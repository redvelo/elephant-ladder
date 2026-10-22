import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { observeNetworkActivity } from "../../src/host/network-activity";

class PageDocument extends EventTarget {
  visibilityState = "visible";
  setVisible(visible: boolean): void {
    this.visibilityState = visible ? "visible" : "hidden";
    this.dispatchEvent(new Event("visibilitychange"));
  }
}

const disposers: (() => void)[] = [];
const fetchMock = vi.fn<typeof fetch>();
const snapshot = (lastObservedAt: number | null = 90_000, available = true) =>
  Response.json({ sessionId: "reader", available, lastObservedAt });

function start(visible = true) {
  const document = new PageDocument();
  document.setVisible(visible);
  const window = new EventTarget();
  const onActivity = vi.fn();
  const stop = observeNetworkActivity("reader", "secret", onActivity, { document, window });
  disposers.push(stop);
  return { document, window, onActivity, stop };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(100_000);
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockImplementation(async () => snapshot());
});
afterEach(() => {
  disposers.splice(0).forEach((stop) => stop());
  vi.useRealTimers();
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});

test("authenticates reads and does not refresh an unchanged observation while polling", async () => {
  const { onActivity } = start();
  await vi.advanceTimersByTimeAsync(0);
  expect(fetchMock).toHaveBeenCalledWith("/native/reader/network-activity", {
    cache: "no-store",
    headers: {
      Authorization: "Bearer secret",
      "X-Elephant-Request": "trusted-reader",
      "X-Elephant-Reader-Session": "reader",
    },
    signal: expect.any(AbortSignal),
  });
  expect(onActivity.mock.calls).toEqual([
    [null],
    [{ sessionId: "reader", lastObservedAt: 90_000 }],
  ]);
  await vi.advanceTimersByTimeAsync(32_000);
  expect(fetchMock).toHaveBeenCalledTimes(17);
  expect(onActivity).toHaveBeenCalledTimes(2);
  fetchMock.mockImplementation(async () => snapshot(Date.now()));
  await vi.advanceTimersByTimeAsync(2_000);
  expect(onActivity).toHaveBeenLastCalledWith({ sessionId: "reader", lastObservedAt: 134_000 });
});

test("unavailable platforms never light the icon and poll slowly", async () => {
  fetchMock.mockImplementation(async () => snapshot(90_000, false));
  const { onActivity } = start();
  await vi.advanceTimersByTimeAsync(29_999);
  expect(onActivity.mock.calls).toEqual([[null]]);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  fetchMock.mockImplementation(async () => snapshot(Date.now()));
  await vi.advanceTimersByTimeAsync(1);
  expect(onActivity).toHaveBeenLastCalledWith({ sessionId: "reader", lastObservedAt: 130_000 });
});

test("polls only while visible, and suspends across pagehide/pageshow", async () => {
  const { document, window, onActivity, stop } = start(false);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(fetchMock).not.toHaveBeenCalled();
  document.setVisible(true);
  await vi.advanceTimersByTimeAsync(0);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  document.setVisible(false);
  expect(onActivity).toHaveBeenLastCalledWith(null);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  document.setVisible(true);
  await vi.advanceTimersByTimeAsync(0);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  window.dispatchEvent(new Event("pagehide"));
  document.setVisible(true);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  window.dispatchEvent(new Event("pageshow"));
  await vi.advanceTimersByTimeAsync(0);
  expect(fetchMock).toHaveBeenCalledTimes(3);
  stop();
  window.dispatchEvent(new Event("pageshow"));
  document.setVisible(true);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(fetchMock).toHaveBeenCalledTimes(3);
  expect(vi.getTimerCount()).toBe(0);
});

test("aborts a hidden request and ignores its late reply after resuming", async () => {
  let finish!: (response: Response) => void;
  fetchMock.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { document, onActivity } = start();
  const signal = fetchMock.mock.calls[0]![1]!.signal!;
  await vi.advanceTimersByTimeAsync(4_000);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  document.setVisible(false);
  expect(signal.aborted).toBe(true);
  document.setVisible(true);
  await vi.advanceTimersByTimeAsync(0);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  finish(snapshot(100_000));
  await vi.advanceTimersByTimeAsync(0);
  expect(onActivity).toHaveBeenLastCalledWith({ sessionId: "reader", lastObservedAt: 90_000 });
});

test.each([401, 403, 409])(
  "stops after HTTP %s without retrying on visibility changes",
  async (status) => {
    fetchMock
      .mockImplementationOnce(async () => snapshot())
      .mockImplementation(async () => new Response(null, { status }));
    const { document, onActivity } = start();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(onActivity).toHaveBeenLastCalledWith(null);
    document.setVisible(false);
    document.setVisible(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  },
);

test.each([
  null,
  { sessionId: "other", available: true, lastObservedAt: 90_000 },
  { sessionId: "reader", available: "yes", lastObservedAt: 90_000 },
  { sessionId: "reader", available: true, lastObservedAt: -1 },
  { sessionId: "reader", available: true, lastObservedAt: 100_001 },
  { sessionId: "reader", available: true, lastObservedAt: 0.5 },
  { sessionId: "reader", available: true, lastObservedAt: 90_000, extra: true },
])("rejects invalid snapshots without inventing activity: %j", async (value) => {
  fetchMock.mockImplementation(async () => Response.json(value));
  const { onActivity } = start();
  await vi.advanceTimersByTimeAsync(3_999);
  expect(onActivity.mock.calls).toEqual([[null]]);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(fetchMock).toHaveBeenCalledTimes(2);
});

test("aborts timed-out requests, backs off, and resets cadence after success", async () => {
  fetchMock.mockImplementationOnce(
    (_url, init) =>
      new Promise((_resolve, reject) => {
        init!.signal!.addEventListener("abort", () =>
          reject(new DOMException("Aborted", "AbortError")),
        );
      }),
  );
  start();
  const signal = fetchMock.mock.calls[0]![1]!.signal!;
  await vi.advanceTimersByTimeAsync(4_999);
  expect(signal.aborted).toBe(false);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(signal.aborted).toBe(true);
  await vi.advanceTimersByTimeAsync(4_000);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(2_000);
  expect(fetchMock).toHaveBeenCalledTimes(3);
});
