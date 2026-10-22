import type { NavigatorNarrationStatus } from "epub-navigator/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { createNarrationPlayback } from "../../src/reader/narration-playback";

const fetchMock = vi.fn<typeof fetch>();
const pendingCommands: ((response: Response) => void)[] = [];

function narration(
  state: NavigatorNarrationStatus["state"],
  elapsedMs = 0,
  section: string | null = "Chapter 1. Loomings.",
): NavigatorNarrationStatus {
  return { state, section, timing: { elapsedMs, totalMs: 860_500 }, rate: "1" };
}

function reports(): unknown[] {
  return fetchMock.mock.calls
    .filter(([path]) => path === "/native/reader/narration")
    .map(([, init]) => JSON.parse(init?.body as string));
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(100_000);
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockImplementation(async (path) => {
    if (path === "/native/reader/narration") return new Response(null, { status: 204 });
    return new Promise<Response>((resolve) => pendingCommands.push(resolve));
  });
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  fetchMock.mockReset();
  pendingCommands.splice(0);
});

test("reports section, timing, and rate only when native controls would show a change", () => {
  const playback = createNarrationPlayback("reader", "secret", {
    play: vi.fn(),
    pause: vi.fn(),
    next: vi.fn(),
    previous: vi.fn(),
  });
  playback.report(narration("idle"), "Moby-Dick");
  playback.report(narration("loading", 1_000), "Moby-Dick");
  vi.setSystemTime(110_000);
  playback.report(narration("playing", 11_000), "Moby-Dick");
  playback.report(narration("playing", 11_000, "Chapter 2. The Carpet-Bag."), "Moby-Dick");
  vi.setSystemTime(111_000);
  playback.report(narration("playing", 40_000, "Chapter 2. The Carpet-Bag."), "Moby-Dick");
  playback.report(narration("paused", 40_500, "Chapter 2. The Carpet-Bag."), "Moby-Dick");
  playback.report(narration("ended"), "Moby-Dick");
  expect(reports()).toEqual([
    {
      state: "playing",
      title: "Moby-Dick",
      section: "Chapter 1. Loomings.",
      positionMs: 1_000,
      durationMs: 860_500,
      rate: 1,
    },
    {
      state: "playing",
      title: "Moby-Dick",
      section: "Chapter 2. The Carpet-Bag.",
      positionMs: 11_000,
      durationMs: 860_500,
      rate: 1,
    },
    {
      state: "playing",
      title: "Moby-Dick",
      section: "Chapter 2. The Carpet-Bag.",
      positionMs: 40_000,
      durationMs: 860_500,
      rate: 1,
    },
    {
      state: "paused",
      title: "Moby-Dick",
      section: "Chapter 2. The Carpet-Bag.",
      positionMs: 40_500,
      durationMs: 860_500,
      rate: 1,
    },
    { state: "stopped", title: "", section: null, positionMs: null, durationMs: null, rate: 1 },
  ]);
  expect(fetchMock).toHaveBeenCalledWith(
    "/native/reader/narration",
    expect.objectContaining({
      method: "POST",
      headers: {
        Authorization: "Bearer secret",
        "X-Elephant-Request": "trusted-reader",
        "X-Elephant-Reader-Session": "reader",
        "Content-Type": "application/json",
      },
    }),
  );
  playback.close();
});

test("delivers native media commands to the reader while narration is active", async () => {
  const play = vi.fn();
  const pause = vi.fn();
  const next = vi.fn();
  const previous = vi.fn();
  const playback = createNarrationPlayback("reader", "secret", { play, pause, next, previous });
  playback.report(narration("playing"), "Moby-Dick");
  await vi.waitFor(() => expect(pendingCommands).toHaveLength(1));
  pendingCommands.shift()!(Response.json({ commands: ["pause", "play"] }));
  await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(1));
  expect(pause).toHaveBeenCalledTimes(1);
  await vi.waitFor(() => expect(pendingCommands).toHaveLength(1));
  pendingCommands.shift()!(Response.json({ commands: ["next", "previous", "next"] }));
  await vi.waitFor(() => expect(previous).toHaveBeenCalledTimes(1));
  expect(next).toHaveBeenCalledTimes(2);
  await vi.waitFor(() => expect(pendingCommands).toHaveLength(1));
  playback.close();
  expect(reports().at(-1)).toEqual({
    state: "stopped",
    title: "",
    section: null,
    positionMs: null,
    durationMs: null,
    rate: 1,
  });
});

test("stops integrating when the platform has no native narration playback", async () => {
  fetchMock.mockImplementation(async () => new Response(null, { status: 501 }));
  const playback = createNarrationPlayback("reader", "secret", {
    play: vi.fn(),
    pause: vi.fn(),
    next: vi.fn(),
    previous: vi.fn(),
  });
  playback.report(narration("playing"), "Moby-Dick");
  // A macrotask drains every pending microtask, so the assertion holds however many hops the
  // refusal takes to arrive rather than pinning the current number.
  await new Promise((resolve) => setTimeout(resolve, 0));
  playback.report(narration("paused"), "Moby-Dick");
  playback.close();
  expect(reports()).toHaveLength(1);
});

test("a refused report cancels the command poll it left waiting", async () => {
  const signals: AbortSignal[] = [];
  fetchMock.mockImplementation(async (path, init) => {
    if (path === "/native/reader/narration") return new Response(null, { status: 501 });
    if (init?.signal) signals.push(init.signal);
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener(
        "abort",
        () => reject(new DOMException("aborted", "AbortError")),
        { once: true },
      );
    });
  });
  const playback = createNarrationPlayback("reader", "secret", {
    play: vi.fn(),
    pause: vi.fn(),
    next: vi.fn(),
    previous: vi.fn(),
  });
  playback.report(narration("playing"), "Moby-Dick");
  await vi.waitFor(() => expect(signals).toHaveLength(1));
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(signals[0]?.aborted).toBe(true);
  playback.close();
});
