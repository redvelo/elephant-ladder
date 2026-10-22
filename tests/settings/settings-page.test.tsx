// @vitest-environment jsdom
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { SettingsPage } from "../../src/settings/page";
import { bootstrapAtom } from "../../src/app/session";
import { settingsAtom } from "../../src/settings/state";
import { appSettings, libraryBootstrap } from "../support/state";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function attachedPointer(kind: "coarse" | "fine"): void {
  vi.stubGlobal("matchMedia", (media: string) => {
    const list = new EventTarget() as EventTarget & { media: string; matches: boolean };
    list.media = media;
    list.matches =
      kind === "fine" ? media === "(any-hover: hover)" : media === "(any-pointer: coarse)";
    return list;
  });
}

function renderSettings(bootstrap = libraryBootstrap) {
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => new Promise<Response>(() => {})),
  );
  const store = createStore();
  store.set(bootstrapAtom, bootstrap);
  store.set(settingsAtom, appSettings);
  render(
    <Provider store={store}>
      <SettingsPage
        disabled={false}
        portalContainer={document.body}
        onDictionaries={() => {}}
        onNotices={() => {}}
        onShortcuts={() => {}}
      />
    </Provider>,
  );
  return userEvent.setup();
}

test("the index names its pages and keeps the reading fields off it", async () => {
  attachedPointer("fine");
  const user = renderSettings();
  for (const label of ["Reading", "Keyboard shortcuts", "Dictionaries", "Book permissions"]) {
    expect(screen.getByRole("button", { name: new RegExp(label, "u") })).toBeTruthy();
  }
  expect(screen.queryByRole("radiogroup", { name: "Interface colors" })).toBeNull();

  await user.click(screen.getByRole("button", { name: /Reading/u }));
  const reading = await screen.findByRole("dialog", { name: "Reading" });
  expect(within(reading).getByRole("radiogroup", { name: "Interface colors" })).toBeTruthy();
  expect(within(reading).getByRole("radiogroup", { name: "Margins" })).toBeTruthy();
});

test("resetting is confirmed from inside the reading page and leaves it standing", async () => {
  attachedPointer("fine");
  const user = renderSettings();
  const trigger = screen.getByRole("button", { name: /Reading/u });
  await user.click(trigger);
  const reading = await screen.findByRole("dialog", { name: "Reading" });

  await user.click(within(reading).getByRole("button", { name: "Reset…" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Reset reading settings?" });
  await user.click(within(confirm).getByRole("button", { name: "Cancel" }));

  await waitFor(() =>
    expect(screen.queryByRole("alertdialog", { name: "Reset reading settings?" })).toBeNull(),
  );
  expect(screen.getByRole("dialog", { name: "Reading" })).toBeTruthy();
});

test("a touch-only device is not offered the keyboard pages", () => {
  attachedPointer("coarse");
  renderSettings();
  expect(screen.queryByRole("button", { name: /Keyboard shortcuts/u })).toBeNull();
  expect(screen.getByRole("button", { name: /Reading/u })).toBeTruthy();
});

test("an e-ink device gets a Device page carrying what its display reports", async () => {
  attachedPointer("coarse");
  const user = renderSettings({
    ...libraryBootstrap,
    display: { technology: "eink", refreshMode: "regal_plus", speedModeWhileDragging: true },
  });
  await user.click(screen.getByRole("button", { name: /Device/u }));
  const page = await screen.findByRole("dialog", { name: "Device" });
  expect(within(page).getByText("Regal+")).toBeTruthy();
  expect(within(page).getByText(/clears up with a flash/u)).toBeTruthy();
});

test("the permissions page carries the defaults rather than the index", async () => {
  attachedPointer("fine");
  const user = renderSettings();
  await user.click(screen.getByRole("button", { name: /Book permissions/u }));
  const page = await screen.findByRole("dialog", { name: "Book permissions" });
  expect(within(page).getByRole("status")).toBeTruthy();
});
