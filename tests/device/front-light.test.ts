import { createStore } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import {
  frontLightDialogEnabledAtom,
  frontLightDialogErrorAtom,
  frontLightDialogPendingAtom,
  showFrontLightDialogAtom,
} from "../../src/device/front-light";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap } from "../support/state";

afterEach(() => vi.unstubAllGlobals());

test("the front-light dialog is offered only when the platform reports one", () => {
  const store = createStore();
  expect(store.get(frontLightDialogEnabledAtom)).toBe(false);
  store.set(bootstrapAtom, libraryBootstrap);
  expect(store.get(frontLightDialogEnabledAtom)).toBe(false);
  store.set(bootstrapAtom, {
    ...libraryBootstrap,
    nativeActions: { frontLightDialog: true, sideButtons: true, windowFullscreen: false },
  });
  expect(store.get(frontLightDialogEnabledAtom)).toBe(true);
});

test("showing the dialog authorizes the request and reports dispatch only", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(new Response(null, { status: 204 }));
  vi.stubGlobal("fetch", fetch);

  await store.set(showFrontLightDialogAtom);

  expect(fetch).toHaveBeenCalledTimes(1);
  const [path, request] = fetch.mock.calls[0] ?? [];
  expect(path).toBe("/native/display/front-light-dialog");
  expect(request?.method).toBe("POST");
  expect(request?.headers).toHaveProperty(
    "Authorization",
    `Bearer ${libraryBootstrap.sessionToken}`,
  );
  expect(request?.headers).toHaveProperty("Content-Type", "application/json");
  expect(store.get(frontLightDialogErrorAtom)).toBeNull();
  expect(store.get(frontLightDialogPendingAtom)).toBe(false);
});

test("a rejected dispatch surfaces an error and leaves nothing pending", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>().mockResolvedValue(new Response(null, { status: 503 })),
  );

  await store.set(showFrontLightDialogAtom);

  expect(store.get(frontLightDialogErrorAtom)).toContain("503");
  expect(store.get(frontLightDialogPendingAtom)).toBe(false);
});

test("a second request is ignored while one is already in flight", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  let release = (): void => {};
  const blocked = new Promise<Response>((resolve) => {
    release = () => resolve(new Response(null, { status: 204 }));
  });
  const fetch = vi.fn<typeof globalThis.fetch>().mockReturnValue(blocked);
  vi.stubGlobal("fetch", fetch);

  const first = store.set(showFrontLightDialogAtom);
  expect(store.get(frontLightDialogPendingAtom)).toBe(true);
  await store.set(showFrontLightDialogAtom);
  release();
  await first;

  expect(fetch).toHaveBeenCalledTimes(1);
  expect(store.get(frontLightDialogPendingAtom)).toBe(false);
});
