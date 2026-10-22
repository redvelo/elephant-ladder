import { afterEach, expect, test } from "vitest";
import {
  createNativeDictionaryCapabilities,
  isDictionaryCapability,
} from "../../src/reader/capabilities/dictionary";

const capability = {
  revision: "a".repeat(64),
  views: [
    {
      viewId: "fr",
      viewType: "monolingual" as const,
      sourceName: "Wiktionary",
      editionLanguage: "fr",
      sourceLanguage: "fr",
      targetLanguage: null,
    },
  ],
  audioLanguages: ["fr"],
};

const previousFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = previousFetch;
});

function capture(response: () => Response) {
  const calls: { input: string; body: unknown; headers: unknown }[] = [];
  globalThis.fetch = async (input, init = {}) => {
    calls.push({
      input: input instanceof Request ? input.url : String(input),
      body: JSON.parse(init.body as string),
      headers: init.headers,
    });
    return response();
  };
  return calls;
}

const signal = new AbortController().signal;

test("the bootstrap dictionary capability requires source identity", () => {
  expect(isDictionaryCapability(capability)).toBe(true);
  const { sourceName: _sourceName, ...legacy } = capability.views[0]!;
  expect(isDictionaryCapability({ ...capability, views: [legacy] })).toBe(false);
});

test("lookup sends the session, revision, and request and returns the lookup", async () => {
  const calls = capture(() =>
    Response.json({
      status: "completed",
      requestId: "r1",
      revision: capability.revision,
      query: "éléphant",
      views: [{ viewId: "fr", outcome: { type: "no-entry" } }],
    }),
  );
  const dictionary = createNativeDictionaryCapabilities("session", "token", capability);
  const outcome = await dictionary.lookup(
    { requestId: "r1", text: "éléphant,", language: "fr", initiator: "selection" },
    { signal },
  );
  expect(outcome).toStrictEqual({
    status: "completed",
    lookup: {
      requestId: "r1",
      query: "éléphant",
      views: [{ viewId: "fr", outcome: { type: "no-entry" } }],
    },
  });
  expect(calls).toStrictEqual([
    {
      input: "/native/dictionary/lookup",
      headers: { Authorization: "Bearer token", "Content-Type": "application/json" },
      body: {
        readerSessionId: "session",
        revision: capability.revision,
        requestId: "r1",
        text: "éléphant,",
        language: "fr",
      },
    },
  ]);
});

test("entry and section reads map host statuses to provider outcomes", async () => {
  capture(() =>
    Response.json({ status: "unavailable", reason: "snapshot-unavailable" }, { status: 409 }),
  );
  const dictionary = createNativeDictionaryCapabilities("session", "token", capability);
  expect(await dictionary.entry("e0.00.0", { signal })).toStrictEqual({
    status: "unavailable",
    reason: "snapshot-unavailable",
  });

  const calls = capture(() =>
    Response.json({ status: "unavailable", reason: "entry-unavailable" }, { status: 404 }),
  );
  expect(
    await dictionary.section(
      "e0.00.0",
      { type: "sense-examples", sense: 2 },
      { offset: 8, limit: 4 },
      { signal },
    ),
  ).toStrictEqual({ status: "unavailable", reason: "entry-unavailable" });
  expect(calls[0]?.body).toStrictEqual({
    readerSessionId: "session",
    revision: capability.revision,
    entry: "e0.00.0",
    section: { type: "sense-examples", sense: 2 },
    offset: 8,
    limit: 4,
  });

  capture(() => Response.json({ status: "failed", message: "Broken." }, { status: 500 }));
  expect(await dictionary.entry("e0.00.0", { signal })).toStrictEqual({
    status: "failed",
    message: "Broken.",
  });
});

test("audio returns the recording as a typed blob", async () => {
  capture(
    () =>
      new Response(new Uint8Array([79, 103, 103, 83]), {
        headers: { "Content-Type": "audio/ogg; codecs=opus" },
      }),
  );
  const dictionary = createNativeDictionaryCapabilities("session", "token", capability);
  const outcome = await dictionary.audio("a.fr.RmlsZQ", { signal });
  expect(outcome.status).toBe("completed");
  if (outcome.status !== "completed") return;
  expect(outcome.blob.type).toBe("audio/ogg; codecs=opus");
  expect(outcome.blob.size).toBe(4);
});

test("closing aborts in-flight requests", async () => {
  globalThis.fetch = (_input, init = {}) =>
    new Promise((_resolve, reject) =>
      init.signal?.addEventListener("abort", () =>
        reject(new DOMException("aborted", "AbortError")),
      ),
    );
  const dictionary = createNativeDictionaryCapabilities("session", "token", capability);
  const pending = dictionary.entry("e0.00.0", { signal });
  dictionary.close();
  dictionary.close();
  expect(dictionary.signal.aborted).toBe(true);
  await expect(pending).rejects.toThrow("aborted");
});
