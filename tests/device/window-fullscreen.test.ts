import { createStore } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import {
  setWindowFullscreenAtom,
  syncWindowFullscreenAtom,
  windowFullscreenAtom,
  windowFullscreenAvailableAtom,
} from "../../src/device/window-fullscreen";
import { bootstrapAtom } from "../../src/app/session";
import { readerNoticeAtom } from "../../src/reader/state";
import { libraryBootstrap } from "../support/state";

afterEach(() => vi.unstubAllGlobals());

const hostWithWindow = {
  ...libraryBootstrap,
  nativeActions: { ...libraryBootstrap.nativeActions, windowFullscreen: true },
};

test("window fullscreen is offered only when the host reports a window", () => {
  const store = createStore();
  expect(store.get(windowFullscreenAvailableAtom)).toBe(false);
  store.set(bootstrapAtom, libraryBootstrap);
  expect(store.get(windowFullscreenAvailableAtom)).toBe(false);
  store.set(bootstrapAtom, hostWithWindow);
  expect(store.get(windowFullscreenAvailableAtom)).toBe(true);
});

test("setting fullscreen authorizes the request and records the asked-for state", async () => {
  const store = createStore();
  store.set(bootstrapAtom, hostWithWindow);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(new Response(null, { status: 204 }));
  vi.stubGlobal("fetch", fetch);

  await store.set(setWindowFullscreenAtom, true);

  expect(fetch).toHaveBeenCalledTimes(1);
  const [path, request] = fetch.mock.calls[0] ?? [];
  expect(path).toBe("/native/window/fullscreen");
  expect(request?.method).toBe("POST");
  expect(request?.body).toBe(JSON.stringify({ fullscreen: true }));
  expect(request?.headers).toHaveProperty(
    "Authorization",
    `Bearer ${libraryBootstrap.sessionToken}`,
  );
  expect(store.get(windowFullscreenAtom)).toBe(true);
  expect(store.get(readerNoticeAtom)).toBeNull();
});

test("syncing reads the window's state back from the host", async () => {
  const store = createStore();
  store.set(bootstrapAtom, hostWithWindow);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(Response.json({ fullscreen: true }));
  vi.stubGlobal("fetch", fetch);

  await store.set(syncWindowFullscreenAtom);

  const [path, request] = fetch.mock.calls[0] ?? [];
  expect(path).toBe("/native/window/fullscreen");
  expect(request?.method).toBeUndefined();
  expect(store.get(windowFullscreenAtom)).toBe(true);
});

test("a host without a window is never asked", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi.fn<typeof globalThis.fetch>();
  vi.stubGlobal("fetch", fetch);

  await store.set(setWindowFullscreenAtom, true);
  await store.set(syncWindowFullscreenAtom);

  expect(fetch).not.toHaveBeenCalled();
  expect(store.get(windowFullscreenAtom)).toBe(false);
});

test("a rejected change leaves the state alone and tells the reader", async () => {
  const store = createStore();
  store.set(bootstrapAtom, hostWithWindow);
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>().mockResolvedValue(new Response(null, { status: 503 })),
  );

  await store.set(setWindowFullscreenAtom, true);

  expect(store.get(windowFullscreenAtom)).toBe(false);
  expect(store.get(readerNoticeAtom)).toBe("Fullscreen could not be changed.");
});

test("a malformed state answer changes nothing", async () => {
  const store = createStore();
  store.set(bootstrapAtom, hostWithWindow);
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>().mockResolvedValue(Response.json({ fullscreen: "true" })),
  );

  await store.set(syncWindowFullscreenAtom);

  expect(store.get(windowFullscreenAtom)).toBe(false);
});
