// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import { createNarrationPlayback } from "../../src/reader/narration-playback";
import { observeNetworkActivity } from "../../src/host/network-activity";
import { ReaderSection } from "../../src/reader/reader";
import { startReadingSessionAtom } from "../../src/reader/session";
import { bootstrapAtom, readerViewAtom } from "../../src/app/session";
import { readingBootstrap } from "../support/state";

vi.mock("epub-navigator/react", () => ({ NavigatorReader: () => null }));
vi.mock("../../src/host/network-activity", () => ({
  observeNetworkActivity: vi.fn(() => vi.fn()),
}));
vi.mock("../../src/reader/narration-playback", () => ({
  createNarrationPlayback: vi.fn(() => ({ report: vi.fn(), close: vi.fn() })),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

test("reader services start once per session however often the reader view changes", () => {
  const store = createStore();
  store.set(bootstrapAtom, readingBootstrap);
  store.set(startReadingSessionAtom, readingBootstrap);
  const { unmount } = render(
    <Provider store={store}>
      <ReaderSection />
    </Provider>,
  );

  for (let revision = 1; revision <= 5; revision += 1) {
    act(() => {
      const view = store.get(readerViewAtom);
      if (view.status === "reading") {
        store.set(readerViewAtom, { ...view, annotations: { revision, records: [] } });
      }
    });
  }
  expect(observeNetworkActivity).toHaveBeenCalledTimes(1);
  expect(createNarrationPlayback).toHaveBeenCalledTimes(1);

  const stopNetworkActivity = vi.mocked(observeNetworkActivity).mock.results[0]?.value as
    | (() => void)
    | undefined;
  unmount();
  expect(stopNetworkActivity).toHaveBeenCalledTimes(1);
});
