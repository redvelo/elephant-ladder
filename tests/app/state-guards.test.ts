// @vitest-environment jsdom
import { createStore, type Atom } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { acquisitionAtom, publicationsAtom } from "../../src/library/state";
import { importPublicationAtom } from "../../src/library/import";
import { openPublicationAtom, returnToLibraryAtom } from "../../src/library/open";
import {
  closePermissionsAtom,
  loadPermissionsAtom,
  permissionsReviewAtom,
} from "../../src/policy/permissions-state";
import { changeReaderPreferencesAtom } from "../../src/reader/preferences";
import { startReadingSessionAtom } from "../../src/reader/session";
import { preferenceErrorAtom } from "../../src/reader/state";
import { bootstrapAtom, readerViewAtom } from "../../src/app/session";
import {
  deferredFetch,
  libraryBootstrap,
  publicationSummary,
  readerPreferences,
  readingBootstrap,
} from "../support/state";

afterEach(() => {
  vi.unstubAllGlobals();
});

function notifications(store: ReturnType<typeof createStore>, atom: Atom<unknown>): () => number {
  let count = 0;
  store.sub(atom, () => {
    count += 1;
  });
  return () => count;
}

test("an identical preferences report neither republishes the view nor writes", () => {
  const fetch = vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json({})));
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, readingBootstrap);
  store.set(startReadingSessionAtom, readingBootstrap);
  const viewNotifications = notifications(store, readerViewAtom);

  for (let report = 0; report < 5; report += 1) {
    store.set(changeReaderPreferencesAtom, structuredClone(readerPreferences));
  }
  expect(viewNotifications()).toBe(0);
  expect(fetch).not.toHaveBeenCalled();

  store.set(changeReaderPreferencesAtom, { ...readerPreferences, fontScale: "125" });
  expect(viewNotifications()).toBe(1);
  expect(fetch).toHaveBeenCalledExactlyOnceWith(
    "/native/preferences",
    expect.objectContaining({ method: "PUT" }),
  );
});

test("a rejected preference save reports the host's reason, not a parse failure", async () => {
  const fetch = vi.fn<typeof globalThis.fetch>(() =>
    Promise.resolve(new Response("invalid preferences", { status: 400 })),
  );
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, readingBootstrap);
  store.set(startReadingSessionAtom, readingBootstrap);

  store.set(changeReaderPreferencesAtom, {
    ...readerPreferences,
    publicationTypeface: "reader-sans",
  });

  // The host answers a rejection in text, so parsing it as JSON used to surface
  // "Unexpected token" in place of the reason.
  await vi.waitFor(() => expect(store.get(preferenceErrorAtom)).not.toBeNull());
  const message = store.get(preferenceErrorAtom);
  expect(message).toContain("HTTP 400");
  expect(message).toContain("invalid preferences");
  expect(message).not.toContain("Unexpected token");
});

test("a superseded open never writes the reader view", async () => {
  const { fetch, requests } = deferredFetch();
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const publication = publicationSummary("1");

  const opening = store.set(openPublicationAtom, publication);
  expect(store.get(readerViewAtom)).toStrictEqual({ status: "opening", publication });
  store.set(returnToLibraryAtom);
  requests[0]!.respond(Response.json({ status: "failed" }, { status: 500 }));
  await opening;

  expect(store.get(readerViewAtom)).toStrictEqual({ status: "library" });
});

test("a superseded import never overwrites the newer one", async () => {
  const { fetch, requests } = deferredFetch();
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const first = store.set(importPublicationAtom, new File(["a"], "first.epub"));
  const second = store.set(importPublicationAtom, new File(["b"], "second.epub"));

  requests[0]!.respond(
    Response.json({ disposition: "created", publication: publicationSummary("1") }),
  );
  await first;
  expect(store.get(acquisitionAtom)).toMatchObject({
    status: "streaming",
    fileName: "second.epub",
  });
  expect(store.get(publicationsAtom)).toStrictEqual([]);

  requests[1]!.respond(
    Response.json({ disposition: "created", publication: publicationSummary("2") }),
  );
  await second;
  expect(store.get(acquisitionAtom)).toMatchObject({
    status: "complete",
    fileName: "second.epub",
  });
  expect(store.get(publicationsAtom)).toStrictEqual([publicationSummary("2")]);
});

test("closing permissions abandons the read and a late response is dropped", async () => {
  const { fetch, requests } = deferredFetch();
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(loadPermissionsAtom, publicationSummary("1").publicationId);
  const signal = requests[0]!.init?.signal;

  store.set(closePermissionsAtom);
  expect(signal?.aborted).toBe(true);
  requests[0]!.respond(new Response("unavailable", { status: 503 }));
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));

  expect(store.get(permissionsReviewAtom)).toStrictEqual({ status: "loading", notice: null });
});
