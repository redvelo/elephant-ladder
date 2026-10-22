// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { DictionariesDialog } from "../../src/dictionaries/dialog";
import {
  dictionariesDialogOperationAtom,
  dictionariesViewAtom,
} from "../../src/dictionaries/state";
import type {
  CatalogPackSummary,
  DictionaryInstallation,
  DictionaryOperation,
} from "../../src/dictionaries/model";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap } from "../support/state";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const downloading: DictionaryOperation = {
  operationId: "a".repeat(32),
  corpusLanguage: "en",
  audio: "streamed",
  phase: "downloading",
  bytesCompleted: 4 * 1024 * 1024,
  bytesTotal: 16 * 1024 * 1024,
  failure: null,
};

const corpus: CatalogPackSummary = {
  packType: "corpus",
  packId: "wiktionary-fr-fr",
  packRevision: "b".repeat(64),
  corpusLanguage: "fr",
  edition: "frwiktionary",
  dumpDate: "2026-09-01",
  monolingual: true,
  bilingualTargets: ["en"],
  licenses: ["CC-BY-SA-4.0"],
  installedBytes: 2_598_395_904,
  installed: true,
};

const streamed: CatalogPackSummary = {
  packType: "audio",
  packId: "wiktionary-fr-audio",
  packRevision: "c".repeat(64),
  corpusLanguage: "fr",
  delivery: "remote",
  recordings: 383_287,
  installedBytes: 67_108_864,
  installed: false,
};

const stored: CatalogPackSummary = {
  ...streamed,
  packRevision: "d".repeat(64),
  delivery: "bundled",
  installedBytes: 1_953_497_088,
};

function renderDialog(operation: DictionaryOperation | null) {
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => new Promise<Response>(() => {})),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  render(
    <Provider store={store}>
      <DictionariesDialog portalContainer={document.body} returnFocus={null} onClose={() => {}} />
    </Provider>,
  );
  // Opening the dialog clears both for the listing it starts, so the state under test lands after.
  act(() => {
    store.set(dictionariesViewAtom, { status: "ready", packs: [], installations: [] });
    store.set(dictionariesDialogOperationAtom, operation);
  });
}

/** Renders the dialog with a listing already in hand, as a load would leave it. */
function renderListing(view: {
  readonly packs?: readonly CatalogPackSummary[];
  readonly installations?: readonly DictionaryInstallation[];
  readonly offline?: boolean;
}) {
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => new Promise<Response>(() => {})),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  render(
    <Provider store={store}>
      <DictionariesDialog portalContainer={document.body} returnFocus={null} onClose={() => {}} />
    </Provider>,
  );
  act(() => {
    store.set(
      dictionariesViewAtom,
      view.offline === true
        ? { status: "offline", installations: [...(view.installations ?? [])] }
        : {
            status: "ready",
            packs: [...(view.packs ?? [])],
            installations: [...(view.installations ?? [])],
          },
    );
  });
}

test("an unreachable catalog still lists the dictionaries the device holds", () => {
  renderListing({
    offline: true,
    installations: [
      {
        packType: "corpus",
        packId: corpus.packId,
        packRevision: corpus.packRevision,
        corpusLanguage: "fr",
        installedBytes: corpus.installedBytes,
        delivery: null,
      },
      {
        packType: "audio",
        packId: streamed.packId,
        packRevision: streamed.packRevision,
        corpusLanguage: "fr",
        installedBytes: 67_108_864,
        delivery: "remote",
      },
    ],
  });
  expect(screen.getByRole("status").textContent).toContain("could not be reached");
  expect(screen.getByText("French")).toBeTruthy();
  expect(screen.getByText("2.5 GiB")).toBeTruthy();
  expect(
    screen.getByText("Pronunciations play from Wikimedia Commons, which needs a network"),
  ).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
  // Installing or changing recordings both need the catalog.
  expect(screen.queryByRole("button", { name: /recordings/u })).toBeNull();
});

test("a language whose pronunciations are missing is finished rather than chosen again", () => {
  renderListing({ packs: [corpus, streamed, stored] });
  expect(screen.getByRole("button", { name: "Finish installing" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
  expect(screen.getByText(/pronunciations · not installed yet/u)).toBeTruthy();
  // Streaming comes with the language, so it is never something to ask for.
  expect(screen.queryByRole("button", { name: "Stream recordings" })).toBeNull();
});

test("a language installed as offered is only offered where its recordings live", () => {
  renderListing({ packs: [corpus, { ...streamed, installed: true }, stored] });
  expect(screen.queryByRole("button", { name: "Finish installing" })).toBeNull();
  expect(screen.getByRole("button", { name: "Store recordings" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
});

test("an install in flight reports its progress on the button that leaves it running", () => {
  renderDialog(downloading);
  const button = screen.getByRole("button", { name: /Continue in background/u });
  expect(button.textContent).toContain("Downloading");
  expect(button.textContent).toContain("4.0 MiB of 16 MiB");
  // The dialog paginates its body, so progress must not be left on a page of it.
  expect(screen.queryByText(/Downloading/u, { selector: ".acquisition-status" })).toBeNull();
});

test("a phase with no byte count still names itself on the button", () => {
  renderDialog({ ...downloading, phase: "verifying" });
  expect(screen.getByRole("button", { name: /Continue in background/u }).textContent).toContain(
    "Verifying",
  );
});

test("a finished install drops the button and states the outcome beside the way out", () => {
  renderDialog({ ...downloading, phase: "completed" });
  expect(screen.queryByRole("button", { name: /Continue in background/u })).toBeNull();
  expect(screen.getByRole("status").textContent).toBe("Installed");
});

test("a failure is raised as an alert rather than a phase", () => {
  renderDialog({ ...downloading, phase: "failed", failure: "The download stopped." });
  expect(screen.getByRole("alert").textContent).toBe("The download stopped.");
  expect(screen.queryByRole("button", { name: /Continue in background/u })).toBeNull();
});
