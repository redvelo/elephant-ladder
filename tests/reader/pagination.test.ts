import { expect, test } from "vitest";
import {
  createLivePaginationCapabilities,
  isPaginationDescriptor,
} from "../../src/reader/capabilities/pagination";

const origin = "http://127.0.0.1:43123";
const available = {
  state: "available" as const,
  responses: {
    mount: {
      baseUrl: `${origin}/`,
      remoteResources: "blocked" as const,
      contentOrigin: "isolated-publication-origin" as const,
    },
    eligibleResourcePaths: ["EPUB/text/chapter.xhtml"],
  },
  foregroundPreparation: {
    eligibleResourcePaths: ["EPUB/text/chapter.xhtml", "EPUB/images/fixed-page.svg"],
  },
  cache: {
    baseUrl: "/native/pagination-cache/",
    handle: "pagination-handle-0000000000000000",
    measurementArtifactDigest: "a".repeat(64),
  },
};

test("pagination descriptor accepts exact live and unavailable shapes", () => {
  expect(isPaginationDescriptor(available, origin)).toBe(true);
  expect(
    isPaginationDescriptor({ state: "unavailable", reason: "capabilities-unavailable" }, origin),
  ).toBe(true);
});

test("pagination descriptor rejects mismatched mounts and unsafe or duplicate paths", () => {
  expect(
    isPaginationDescriptor(
      {
        ...available,
        responses: {
          ...available.responses,
          mount: { ...available.responses.mount, baseUrl: "http://127.0.0.1:43124/" },
        },
      },
      origin,
    ),
  ).toBe(false);
  expect(
    isPaginationDescriptor(
      {
        ...available,
        foregroundPreparation: { eligibleResourcePaths: ["../chapter.xhtml"] },
      },
      origin,
    ),
  ).toBe(false);
  expect(
    isPaginationDescriptor(
      {
        ...available,
        responses: {
          ...available.responses,
          eligibleResourcePaths: ["chapter.xhtml", "chapter.xhtml"],
        },
      },
      origin,
    ),
  ).toBe(false);
});

test("live pagination capability is frozen and closes idempotently", () => {
  const capability = createLivePaginationCapabilities(available);
  expect(Object.isFrozen(capability)).toBe(true);
  expect(Object.isFrozen(capability.responses)).toBe(true);
  expect(Object.isFrozen(capability.responses?.mount)).toBe(true);
  expect(Object.isFrozen(capability.responses?.eligibleResourcePaths)).toBe(true);
  expect(Object.isFrozen(capability.foregroundPreparation)).toBe(true);
  expect(Object.isFrozen(capability.cache)).toBe(true);
  expect(Object.isFrozen(capability.cacheIdentity)).toBe(true);
  expect(Object.isFrozen(capability.foregroundPreparation?.eligibleResourcePaths)).toBe(true);
  expect(capability.foregroundPreparation?.eligibleResourcePaths).toStrictEqual([
    "EPUB/text/chapter.xhtml",
    "EPUB/images/fixed-page.svg",
  ]);
  expect(capability.signal.aborted).toBe(false);
  capability.close();
  capability.close();
  expect(capability.signal.aborted).toBe(true);
});

test("pagination cache adapter sends only opaque keys and credentials", async () => {
  const requests: Array<{ url: string; init: RequestInit }> = [];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async (input, init = {}) => {
    requests.push({ url: input instanceof Request ? input.url : String(input), init });
    if (init.method === "POST") {
      const { records } = JSON.parse(init.body as string) as { records: string[] };
      return Response.json({ records: records.map((_, index) => (index === 0 ? "AQID" : null)) });
    }
    return new Response(null, { status: 204 });
  };
  try {
    const capability = createLivePaginationCapabilities(available, "reader-token");
    const basis = "b".repeat(64);
    const record = "c".repeat(64);
    const signal = new AbortController().signal;
    expect(await capability.cache?.read(basis, record, { signal, maxBytes: 65_536 })).toStrictEqual(
      new Uint8Array([1, 2, 3]),
    );
    expect(
      await capability.cache?.readMany?.(basis, [record, "d".repeat(64)], {
        signal,
        maxBytes: 65_536,
      }),
    ).toStrictEqual([new Uint8Array([1, 2, 3]), null]);
    await capability.cache?.write(basis, record, new Uint8Array([4]), { signal });
    await capability.cache?.writeMany?.(
      basis,
      [
        { type: "write", record, value: new Uint8Array([4, 5]) },
        { type: "remove", record: "d".repeat(64) },
      ],
      { signal },
    );
    await capability.cache?.remove?.(basis, record, { signal });
    await capability.cache?.removeBasis(basis, { signal });
    // Every operation, per-record or not, travels one of the two bulk routes.
    expect(requests.map(({ init }) => init.method)).toStrictEqual([
      "POST",
      "POST",
      "PUT",
      "PUT",
      "PUT",
      "DELETE",
    ]);
    for (const request of requests) {
      expect(request.url).toBe(`/native/pagination-cache/${basis}`);
    }
    const headers = requests[0]?.init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer reader-token");
    expect(headers["x-elephant-pagination-handle"]).toBe(available.cache.handle);
    expect(headers["content-type"]).toBe("application/json");
    expect(JSON.parse(requests[0]?.init.body as string)).toStrictEqual({ records: [record] });
    expect(JSON.parse(requests[1]?.init.body as string)).toStrictEqual({
      records: [record, "d".repeat(64)],
    });
    expect(JSON.parse(requests[2]?.init.body as string)).toStrictEqual({
      operations: [{ type: "write", record, value: "BA==" }],
    });
    expect(JSON.parse(requests[3]?.init.body as string)).toStrictEqual({
      operations: [
        { type: "write", record, value: "BAU=" },
        { type: "remove", record: "d".repeat(64) },
      ],
    });
    expect(JSON.parse(requests[4]?.init.body as string)).toStrictEqual({
      operations: [{ type: "remove", record }],
    });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("pagination bulk cache adapter rejects malformed responses", async () => {
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async () => Response.json({ records: ["not base64"] });
  try {
    const cache = createLivePaginationCapabilities(available, "reader-token").cache;
    await expect(
      cache?.readMany?.("b".repeat(64), ["c".repeat(64)], {
        signal: new AbortController().signal,
        maxBytes: 65_536,
      }),
    ).rejects.toThrow(/bulk read returned invalid data/);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("pagination cache adapter reports mutations that the host did not apply", async () => {
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response(null, { status: 409 });
  try {
    const cache = createLivePaginationCapabilities(available, "reader-token").cache;
    const signal = new AbortController().signal;
    expect(await cache?.read("b".repeat(64), "c".repeat(64), { signal, maxBytes: 65_536 })).toBe(
      null,
    );
    expect(
      await cache?.readMany?.("b".repeat(64), ["c".repeat(64)], {
        signal,
        maxBytes: 65_536,
      }),
    ).toStrictEqual([null]);
    await expect(
      cache?.write("b".repeat(64), "c".repeat(64), new Uint8Array([1]), { signal }),
    ).rejects.toThrow(/pagination cache bulk mutation failed: 409/);
    await expect(
      cache?.writeMany?.(
        "b".repeat(64),
        [{ type: "write", record: "c".repeat(64), value: new Uint8Array([1]) }],
        { signal },
      ),
    ).rejects.toThrow(/pagination cache bulk mutation failed: 409/);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("foreground preparation remains available without measurement responses", () => {
  const descriptor = {
    state: "available" as const,
    foregroundPreparation: {
      eligibleResourcePaths: ["EPUB/images/fixed-page.svg"],
    },
  };
  expect(isPaginationDescriptor(descriptor, origin)).toBe(true);
  const capability = createLivePaginationCapabilities(descriptor);
  expect(capability.responses).toBe(undefined);
  expect(capability.foregroundPreparation?.eligibleResourcePaths).toStrictEqual([
    "EPUB/images/fixed-page.svg",
  ]);
});
