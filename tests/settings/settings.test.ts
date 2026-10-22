import { createStore } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import {
  loadSettingsAtom,
  resetSettingsAtom,
  saveSettingsAtom,
  settingsAtom,
  settingsErrorAtom,
} from "../../src/settings/state";
import { loadStorageUsageAtom, storageUsageAtom } from "../../src/settings/storage-usage";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap } from "../support/state";

afterEach(() => vi.unstubAllGlobals());

test("settings persist through native storage and revision conflicts preserve the server values", async () => {
  vi.stubGlobal("window", { dispatchEvent: vi.fn() });
  vi.stubGlobal("document", { querySelector: () => null });
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const initial = {
    revision: 3,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: false },
  };
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(Response.json(initial));
  vi.stubGlobal("fetch", fetch);
  store.set(loadSettingsAtom);
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(initial));
  const saved = {
    ...initial,
    revision: 4,
    preferences: { ...initial.preferences, fontScale: "125" as const },
  };
  fetch.mockResolvedValueOnce(Response.json(saved));
  store.set(saveSettingsAtom, { preferences: saved.preferences });
  // The edit shows immediately; the authoritative revision arrives behind it.
  expect(store.get(settingsAtom)?.preferences.fontScale).toBe("125");
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(saved));
  const request = fetch.mock.calls[1]?.[1];
  expect(request?.headers).toHaveProperty(
    "X-Elephant-Library-Document",
    libraryBootstrap.libraryDocumentId,
  );
  expect(JSON.parse(request?.body as string)).toEqual({
    ...initial,
    preferences: saved.preferences,
  });
  const conflicting = { ...saved, revision: 5 };
  fetch.mockResolvedValueOnce(Response.json(conflicting, { status: 409 }));
  store.set(saveSettingsAtom, { preferences: initial.preferences });
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(conflicting));
  expect(store.get(settingsErrorAtom)).toContain("changed on another screen");
});

test("edits made while a save is in flight are coalesced instead of dropped", async () => {
  vi.stubGlobal("window", { dispatchEvent: vi.fn() });
  vi.stubGlobal("document", { querySelector: () => null });
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const initial = {
    revision: 1,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: false },
  };
  const steps = [
    { ...initial.preferences, fontScale: "110" as const },
    { ...initial.preferences, fontScale: "125" as const },
  ] as const;
  let release = (): void => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(initial))
    .mockImplementationOnce(async () => {
      await held;
      return Response.json({ ...initial, revision: 2, preferences: steps[0] });
    })
    .mockImplementationOnce(() =>
      Promise.resolve(Response.json({ ...initial, revision: 3, preferences: steps[1] })),
    );
  vi.stubGlobal("fetch", fetch);
  store.set(loadSettingsAtom);
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(initial));

  store.set(saveSettingsAtom, { preferences: steps[0] });
  store.set(saveSettingsAtom, { preferences: steps[1] });
  // The second press is kept locally while the first write is still outstanding.
  expect(store.get(settingsAtom)?.preferences.fontScale).toBe("125");
  release();

  await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
  await vi.waitFor(() => expect(store.get(settingsAtom)?.revision).toBe(3));
  expect(store.get(settingsAtom)?.preferences.fontScale).toBe("125");
  // The queued write carries the revision the first write returned.
  expect(JSON.parse(fetch.mock.calls[2]?.[1]?.body as string)).toMatchObject({
    revision: 2,
    preferences: steps[1],
  });
  expect(store.get(settingsErrorAtom)).toBeNull();
});

test("a reset returns the reading preferences the host ships with", async () => {
  vi.stubGlobal("window", { dispatchEvent: vi.fn() });
  vi.stubGlobal("document", { querySelector: () => null });
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const initial = {
    revision: 4,
    preferences: { ...libraryBootstrap.preferences, fontScale: "200" as const },
    device: { sideButtonsTurnPages: true },
  };
  const reset = {
    revision: 5,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: true },
  };
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(initial))
    .mockResolvedValueOnce(Response.json(reset));
  vi.stubGlobal("fetch", fetch);
  store.set(loadSettingsAtom);
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(initial));

  await store.set(resetSettingsAtom);

  expect(fetch.mock.calls[1]?.[0]).toBe("/native/app-settings/reset");
  expect(JSON.parse(fetch.mock.calls[1]?.[1]?.body as string)).toEqual({ revision: 4 });
  expect(store.get(settingsAtom)).toEqual(reset);
  expect(store.get(settingsErrorAtom)).toBeNull();
});

test("the device's page buttons are a saved choice, sent with the settings that hold it", async () => {
  vi.stubGlobal("window", { dispatchEvent: vi.fn() });
  vi.stubGlobal("document", { querySelector: () => null });
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const initial = {
    revision: 1,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: false },
  };
  const saved = {
    revision: 2,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: true },
  };
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(initial))
    .mockResolvedValueOnce(Response.json(saved));
  vi.stubGlobal("fetch", fetch);
  store.set(loadSettingsAtom);
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(initial));

  store.set(saveSettingsAtom, { device: { sideButtonsTurnPages: true } });

  expect(JSON.parse(fetch.mock.calls[1]?.[1]?.body as string)).toStrictEqual({
    revision: 1,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: true },
  });
  expect(store.get(settingsAtom)?.device.sideButtonsTurnPages).toBe(true);
  await vi.waitFor(() => expect(store.get(settingsAtom)).toEqual(saved));
});

test("storage usage is measured on request and a failure stays reportable", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json({ booksBytes: 4096, dictionariesBytes: 2048 }))
    .mockResolvedValueOnce(new Response("nope", { status: 500 }));
  vi.stubGlobal("fetch", fetch);

  await store.set(loadStorageUsageAtom);
  expect(store.get(storageUsageAtom)).toStrictEqual({
    status: "ready",
    usage: { booksBytes: 4096, dictionariesBytes: 2048 },
  });
  expect(fetch.mock.calls[0]?.[0]).toBe("/native/storage/usage");

  await store.set(loadStorageUsageAtom);
  expect(store.get(storageUsageAtom)).toMatchObject({ status: "failed" });
});

test("storage usage that is not a set of byte counts is refused", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() =>
      Promise.resolve(Response.json({ booksBytes: -1, dictionariesBytes: 0 })),
    ),
  );

  await store.set(loadStorageUsageAtom);
  expect(store.get(storageUsageAtom)).toMatchObject({ status: "failed" });
});
