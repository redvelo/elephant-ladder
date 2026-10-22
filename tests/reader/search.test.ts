import { expect, test } from "vitest";
import { createNativeSearchCapabilities } from "../../src/reader/capabilities/search";

const request = {
  requestId: "search-1",
  query: "elephant",
  scope: { type: "publication" as const },
  initiator: "query-entry" as const,
  continuation: null,
  limit: 20,
};

const completed = {
  status: "completed" as const,
  batch: {
    requestId: "search-1",
    results: [
      {
        id: "result-1",
        section: {
          id: "section-1",
          occurrence: 0,
          resource: "EPUB/chapter.xhtml",
          label: "Chapter One",
        },
        targetJson: "{}",
        excerpt: { before: "an ", match: "elephant", after: " climbed" },
      },
    ],
    coverage: { state: "complete" as const, searchedOccurrences: 1, totalOccurrences: 1 },
    continuation: null,
    totalResultCount: 1,
  },
};

test("native search sends the session credential and exact request envelope", async () => {
  const previousFetch = globalThis.fetch;
  let captured: { input: string; init: RequestInit } | undefined;
  globalThis.fetch = async (input, init = {}) => {
    captured = { input: input instanceof Request ? input.url : String(input), init };
    return Response.json(completed);
  };
  try {
    const capabilities = createNativeSearchCapabilities("reader-session-1", "reader-token");
    const outcome = await capabilities.search(request, {
      signal: new AbortController().signal,
    });
    expect(outcome).toStrictEqual(completed);
    expect(captured?.input).toBe("/native/search");
    expect(captured?.init.method).toBe("POST");
    expect(captured?.init.cache).toBe("no-store");
    expect(captured?.init.headers).toStrictEqual({
      Authorization: "Bearer reader-token",
      "Content-Type": "application/json",
    });
    expect(JSON.parse(captured?.init.body as string)).toStrictEqual({
      readerSessionId: "reader-session-1",
      request,
    });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("native search handles typed failures and HTTP status fallbacks", async () => {
  const previousFetch = globalThis.fetch;
  const responses = [
    new Response(null, { status: 409 }),
    Response.json(
      { status: "unavailable", reason: "policy", message: "Search is disabled." },
      { status: 403 },
    ),
    Response.json(completed, { status: 500 }),
  ];
  globalThis.fetch = async () => responses.shift()!;
  try {
    const capabilities = createNativeSearchCapabilities("reader-session-1", "reader-token");
    const context = { signal: new AbortController().signal };
    expect(await capabilities.search(request, context)).toStrictEqual({
      status: "unavailable",
      reason: "snapshot-unavailable",
      message: null,
    });
    expect(await capabilities.search(request, context)).toStrictEqual({
      status: "unavailable",
      reason: "policy",
      message: "Search is disabled.",
    });
    expect(await capabilities.search(request, context)).toStrictEqual({
      status: "failed",
      message: null,
    });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("native search combines request and capability cancellation", async () => {
  const previousFetch = globalThis.fetch;
  const seenSignals: AbortSignal[] = [];
  globalThis.fetch = (_input, init = {}) => {
    const signal = init.signal!;
    seenSignals.push(signal);
    return new Promise<Response>((_resolve, reject) => {
      signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
    });
  };
  try {
    const capabilities = createNativeSearchCapabilities("reader-session-1", "reader-token");
    const contextController = new AbortController();
    const contextSearch = capabilities.search(request, { signal: contextController.signal });
    contextController.abort();
    await expect(contextSearch).rejects.toThrow(/aborted/);
    expect(seenSignals[0]?.aborted).toBe(true);

    const capabilitySearch = capabilities.search(request, {
      signal: new AbortController().signal,
    });
    capabilities.close();
    await expect(capabilitySearch).rejects.toThrow(/aborted/);
    expect(seenSignals[1]?.aborted).toBe(true);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("native search rejects malformed outcome shapes at the adapter boundary", async () => {
  const previousFetch = globalThis.fetch;
  const malformed = {
    ...completed,
    batch: {
      ...completed.batch,
      results: [{ ...completed.batch.results[0], hostPath: "/private/book.epub" }],
    },
  };
  globalThis.fetch = async () => Response.json(malformed);
  try {
    const capabilities = createNativeSearchCapabilities("reader-session-1", "reader-token");
    expect(
      await capabilities.search(request, { signal: new AbortController().signal }),
    ).toStrictEqual({ status: "failed", message: null });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("native search close is idempotent and aborts its public signal", () => {
  const capabilities = createNativeSearchCapabilities("reader-session-1", "reader-token");
  let aborts = 0;
  capabilities.signal.addEventListener("abort", () => {
    aborts += 1;
  });
  capabilities.close();
  capabilities.close();
  expect(capabilities.signal.aborted).toBe(true);
  expect(aborts).toBe(1);
});
