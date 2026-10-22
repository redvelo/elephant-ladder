// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { Library } from "../../src/library/library";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap, publicationSummary } from "../support/state";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function primaryPointer(kind: "coarse" | "fine"): void {
  vi.stubGlobal("matchMedia", (media: string) => {
    const list = new EventTarget() as EventTarget & { media: string; matches: boolean };
    list.media = media;
    list.matches = media === "(pointer: coarse)" && kind === "coarse";
    return list;
  });
}

test("a touch-primary pointer opens book actions as a drawer", async () => {
  primaryPointer("coarse");
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() =>
      Promise.resolve(Response.json([publicationSummary("1", "Moby-Dick")])),
    ),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );

  await screen.findByRole("heading", { name: "Moby-Dick" });
  await user.click(screen.getByRole("button", { name: "Library" }));
  fireEvent.contextMenu(screen.getByRole("button", { name: "Read Moby-Dick" }));

  const drawer = await screen.findByRole("dialog", { name: "Moby-Dick" });
  expect(screen.queryByRole("menuitem")).toBeNull();
  expect(within(drawer).getByRole("button", { name: "Reflections" })).toBeTruthy();
  expect(within(drawer).getByRole("button", { name: "Permissions…" })).toBeTruthy();

  await user.click(within(drawer).getByRole("button", { name: "Delete from device…" }));
  expect(screen.getByRole("heading", { name: "Delete Moby-Dick?" })).toBeTruthy();
});
