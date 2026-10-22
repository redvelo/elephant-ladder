import { createStore } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import {
  appPolicyAtom,
  appPolicyErrorAtom,
  loadAppPolicyAtom,
  saveAppPolicyAtom,
} from "../../src/policy/app-policy";
import { layoutResetAtom } from "../../src/library/state";
import { resetPublicationLayoutAtom } from "../../src/library/layout-reset";
import { bootstrapAtom } from "../../src/app/session";
import { libraryBootstrap, publicationSummary } from "../support/state";

const book = publicationSummary("e");

afterEach(() => vi.unstubAllGlobals());

const stored = (activeContent: string, remoteResources: string, revision: number) => ({
  policy: { global: { activeContent, remoteResources }, publication: null },
  effective: { activeContent: "disabled", remoteResources: "blocked" },
  revisions: { global: revision, publication: 0 },
});

test("the application's permission defaults load and save against their revision", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(stored("ask", "ask", 2)))
    .mockResolvedValueOnce(Response.json({ status: "saved", ...stored("disabled", "ask", 3) }));
  vi.stubGlobal("fetch", fetch);

  store.set(loadAppPolicyAtom);
  await vi.waitFor(() =>
    expect(store.get(appPolicyAtom)).toEqual({
      status: "ready",
      policy: { activeContent: "ask", remoteResources: "ask" },
      revision: 2,
    }),
  );

  await store.set(saveAppPolicyAtom, { activeContent: "disabled" });

  expect(fetch.mock.calls[1]?.[0]).toBe("/native/publication-policy");
  expect(JSON.parse(fetch.mock.calls[1]?.[1]?.body as string)).toEqual({
    baseRevision: 2,
    policy: { activeContent: "disabled", remoteResources: "ask" },
  });
  expect(store.get(appPolicyAtom)).toEqual({
    status: "ready",
    policy: { activeContent: "disabled", remoteResources: "ask" },
    revision: 3,
  });
});

test("a refused permission save keeps the values the host still holds", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(stored("ask", "ask", 2)))
    .mockResolvedValueOnce(
      Response.json(
        { status: "conflict", reason: "publication is open", authoritative: null },
        { status: 409 },
      ),
    );
  vi.stubGlobal("fetch", fetch);
  store.set(loadAppPolicyAtom);
  await vi.waitFor(() => expect(store.get(appPolicyAtom).status).toBe("ready"));

  await store.set(saveAppPolicyAtom, { remoteResources: "allowed" });

  expect(store.get(appPolicyAtom)).toEqual({
    status: "ready",
    policy: { activeContent: "ask", remoteResources: "ask" },
    revision: 2,
  });
  expect(store.get(appPolicyErrorAtom)).toContain("Close the open book");
});

test("a conflicting permission save adopts the revision the host reports", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(Response.json(stored("ask", "ask", 2)))
    .mockResolvedValueOnce(
      Response.json(
        {
          status: "conflict",
          reason: "revision-mismatch",
          authoritative: stored("automatic", "blocked", 7),
        },
        { status: 409 },
      ),
    );
  vi.stubGlobal("fetch", fetch);
  store.set(loadAppPolicyAtom);
  await vi.waitFor(() => expect(store.get(appPolicyAtom).status).toBe("ready"));

  await store.set(saveAppPolicyAtom, { activeContent: "disabled" });

  expect(store.get(appPolicyAtom)).toEqual({
    status: "ready",
    policy: { activeContent: "automatic", remoteResources: "blocked" },
    revision: 7,
  });
  expect(store.get(appPolicyErrorAtom)).toContain("changed elsewhere");
});

test("a book's own layout is dropped so it follows the application defaults again", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(new Response(null, { status: 204 }));
  vi.stubGlobal("fetch", fetch);

  await store.set(resetPublicationLayoutAtom, book);

  const [url, request] = fetch.mock.calls[0] ?? [];
  expect(url).toBe(`/native/publications/${book.publicationId}/preferences`);
  expect(request?.method).toBe("DELETE");
  expect(request?.headers).toHaveProperty(
    "X-Elephant-Library-Document",
    libraryBootstrap.libraryDocumentId,
  );
  expect(store.get(layoutResetAtom)).toEqual({ status: "done", publication: book });
});

test("a refused layout reset is reported rather than claimed", async () => {
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  vi.stubGlobal(
    "fetch",
    vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValue(new Response("publication is open", { status: 409 })),
  );

  await store.set(resetPublicationLayoutAtom, book);

  expect(store.get(layoutResetAtom)).toMatchObject({ status: "failed" });
});
