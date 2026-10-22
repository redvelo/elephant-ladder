// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { Library } from "../../src/library/library";
import { LibraryNotices } from "../../src/library/notices";
import { bootstrapAtom, readerViewAtom } from "../../src/app/session";
import { destinationAtom } from "../../src/app/destination";
import { libraryBootstrap, publicationSummary } from "../support/state";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function renderLibrary(answer: () => Promise<Response>) {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  vi.stubGlobal("fetch", vi.fn<typeof globalThis.fetch>(answer));
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  return store;
}

test("an unanswered library says nothing rather than flashing a message", () => {
  renderLibrary(() => new Promise<Response>(() => {}));
  expect(screen.queryByText(/Opening your library/u)).toBeNull();
  expect(screen.queryByText("No books yet.")).toBeNull();
  // The shell itself is still there, so the destinations stay reachable while it loads.
  expect(screen.getByRole("navigation", { name: "Main navigation" })).toBeTruthy();
});

test("an answered empty library states that it is empty", async () => {
  renderLibrary(() => Promise.resolve(Response.json([])));
  expect(await screen.findByText("Your next chapter starts here.")).toBeTruthy();
});

test("opening a book is silent, because it ends in a navigation", () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(destinationAtom, "library");
  store.set(readerViewAtom, { status: "opening", publication: publicationSummary("1") });
  render(
    <Provider store={store}>
      <LibraryNotices pace="standard" />
    </Provider>,
  );
  expect(screen.queryByText(/Opening publication/u)).toBeNull();
  expect(screen.queryByText(/Preparing publication/u)).toBeNull();
});

test("preparing a book is reported, because it can take minutes", () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(destinationAtom, "library");
  store.set(readerViewAtom, { status: "preparing", publication: publicationSummary("1") });
  render(
    <Provider store={store}>
      <LibraryNotices pace="standard" />
    </Provider>,
  );
  expect(screen.getByText("Preparing publication…")).toBeTruthy();
});
