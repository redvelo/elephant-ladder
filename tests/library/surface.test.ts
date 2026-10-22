import { createStore } from "jotai";
import { expect, test } from "vitest";
import { bootstrapAtom, LIBRARY_VIEW, readerViewAtom } from "../../src/app/session";
import {
  showBookDetailsAtom,
  showDictionariesAtom,
  showNoticesAtom,
  showShortcutsAtom,
} from "../../src/library/dispatch";
import { closeLibrarySurfaceAtom, librarySurfaceAtom } from "../../src/library/shell-state";
import { libraryBusyAtom } from "../../src/library/state";
import { libraryBootstrap, publicationSummary } from "../support/state";

function library() {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(readerViewAtom, LIBRARY_VIEW);
  return store;
}

test("opening a surface replaces the one before it", () => {
  const store = library();
  store.set(showShortcutsAtom, null);
  expect(store.get(librarySurfaceAtom)?.kind).toBe("shortcuts");
  store.set(showNoticesAtom, null);
  expect(store.get(librarySurfaceAtom)?.kind).toBe("notices");
  store.set(showDictionariesAtom, null);
  expect(store.get(librarySurfaceAtom)?.kind).toBe("dictionaries");
});

test("an open surface holds the library busy until it closes", () => {
  const store = library();
  expect(store.get(libraryBusyAtom)).toBe(false);
  store.set(showShortcutsAtom, null);
  expect(store.get(libraryBusyAtom)).toBe(true);
  store.set(closeLibrarySurfaceAtom);
  expect(store.get(libraryBusyAtom)).toBe(false);
  expect(store.get(librarySurfaceAtom)).toBeNull();
});

test("book details do not open over another surface", () => {
  const store = library();
  store.set(showNoticesAtom, null);
  store.set(showBookDetailsAtom, publicationSummary("1"), null);
  expect(store.get(librarySurfaceAtom)?.kind).toBe("notices");
});
