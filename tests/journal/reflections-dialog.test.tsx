// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { ReflectionsDialog } from "../../src/journal/reflections-dialog";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap, publicationSummary } from "../support/state";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

test("a reflection surface dismisses while its journal is still loading", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => new Promise(() => {})),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const onClose = vi.fn();
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <ReflectionsDialog
        publication={publicationSummary("1", "Moby-Dick")}
        container={document.body}
        returnFocus={document.body}
        onClose={onClose}
      />
    </Provider>,
  );

  await user.click(screen.getByRole("button", { name: "Close" }));
  expect(onClose).toHaveBeenCalledTimes(1);

  await user.click(screen.getByRole("button", { name: "Done" }));
  expect(onClose).toHaveBeenCalledTimes(2);
});
