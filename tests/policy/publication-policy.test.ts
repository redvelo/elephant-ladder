import { expect, test, vi } from "vitest";
import {
  collectPublicationPolicyChoices,
  readPublicationPolicy,
  savePublicationPolicyWithRetry,
  type PolicyDecisionResponse,
  type PublicationCapabilityPrompt,
  type PublicationPolicyReview,
  type PublicationPolicyState,
} from "../../src/policy/publication-policy";

const initial: PublicationPolicyState = {
  policy: {
    global: { activeContent: "automatic", remoteResources: "ask" },
    publication: { activeContent: "disabled", remoteResources: null },
  },
  effective: { activeContent: "disabled", remoteResources: "ask" },
  revisions: { global: 2, publication: 3 },
};

const review: PublicationPolicyReview = {
  ...initial,
  relevance: { activeContent: "detected", remoteResources: "not-detected" },
};

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

test("policy read authenticates, bypasses cache, and forwards the signal", async () => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(response(review));
  const signal = new AbortController().signal;
  await expect(
    readPublicationPolicy({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      signal,
      request,
    }),
  ).resolves.toStrictEqual(review);
  expect(request).toHaveBeenCalledExactlyOnceWith("/native/publications/book/policy", {
    method: "GET",
    cache: "no-store",
    headers: { Authorization: "Bearer secret", "X-Elephant-Request": "trusted-reader" },
    signal,
  });
});

test.each([
  null,
  [],
  initial,
  { ...review, extra: true },
  { ...review, status: "saved" },
  { status: "conflict", reason: "revision-mismatch", authoritative: review },
  { policy: initial.policy, revisions: initial.revisions, relevance: review.relevance },
  { ...review, policy: { ...initial.policy, extra: true } },
  ...(["global", "publication"] as const).flatMap((scope) =>
    ["activeContent", "remoteResources"].map((field) => ({
      ...review,
      policy: {
        ...initial.policy,
        [scope]: { ...initial.policy[scope], [field]: "unknown" },
      },
    })),
  ),
  ...["activeContent", "remoteResources"].map((field) => ({
    ...review,
    effective: { ...initial.effective, [field]: "unknown" },
  })),
  ...["global", "publication"].flatMap((field) =>
    [-1, 1.5, 0x1_0000_0000, "3", null].map((value) => ({
      ...review,
      revisions: { ...initial.revisions, [field]: value },
    })),
  ),
  { ...review, revisions: { global: 0 } },
  { ...review, revisions: { ...initial.revisions, extra: 0 } },
  ...[
    null,
    [],
    {},
    { activeContent: "detected" },
    { remoteResources: "unknown" },
    { activeContent: "enabled", remoteResources: "not-detected" },
    { activeContent: "detected", remoteResources: "allowed" },
    { activeContent: null, remoteResources: "unknown" },
    { activeContent: "unknown", remoteResources: 0 },
    { ...review.relevance, extra: true },
  ].map((relevance) => ({ ...review, relevance })),
])("policy read rejects malformed state %#", async (body) => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(response(body));
  await expect(
    readPublicationPolicy({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      request,
    }),
  ).rejects.toThrow("Policy read returned invalid data.");
});

test("policy read rejects invalid JSON", async () => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(new Response("not JSON"));
  await expect(
    readPublicationPolicy({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      request,
    }),
  ).rejects.toThrow("Policy read returned invalid data.");
});

test.each([400, 401, 403, 404, 409])(
  "policy read rejects HTTP %s without retry",
  async (status) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response(review, status));
    await expect(
      readPublicationPolicy({
        url: "/native/publications/book/policy",
        sessionToken: "secret",
        request,
      }),
    ).rejects.toThrow(`Policy read failed with HTTP ${status}`);
    expect(request).toHaveBeenCalledTimes(1);
  },
);

test.each(["before", "request", "json", "invalid-json"])(
  "policy read respects abort during %s even if transport ignores it",
  async (stage) => {
    const controller = new AbortController();
    const reason = new DOMException("Cancelled policy read", "AbortError");
    const result = response(review);
    const json = vi.spyOn(result, "json").mockImplementation(async () => {
      if (stage === "json" || stage === "invalid-json") controller.abort(reason);
      if (stage === "invalid-json") throw new SyntaxError("Invalid JSON");
      return review;
    });
    const request = vi.fn<typeof fetch>().mockImplementation(async () => {
      if (stage === "request") controller.abort(reason);
      return result;
    });
    if (stage === "before") controller.abort(reason);
    await expect(
      readPublicationPolicy({
        url: "/native/publications/book/policy",
        sessionToken: "secret",
        request,
        signal: controller.signal,
      }),
    ).rejects.toBe(reason);
    expect(request).toHaveBeenCalledTimes(stage === "before" ? 0 : 1);
    expect(json).toHaveBeenCalledTimes(stage === "json" || stage === "invalid-json" ? 1 : 0);
  },
);

test.each(
  (["detected", "not-detected", "unknown"] as const).flatMap((activeContent) =>
    (["detected", "not-detected", "unknown"] as const).map((remoteResources) => ({
      activeContent,
      remoteResources,
    })),
  ),
)(
  "policy read preserves independent assessment $activeContent/$remoteResources",
  async (relevance) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(response({ ...review, relevance }));
    const result = await readPublicationPolicy({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      request,
    });
    expect(result.relevance).toEqual(relevance);
    expect(result.policy).toEqual(initial.policy);
  },
);

test.each([false, true])("reset saves both null overrides (CAS retry=%s)", async (conflict) => {
  const overrides = { activeContent: "enabled", remoteResources: "allowed" } as const;
  const before = { ...initial, policy: { ...initial.policy, publication: overrides } };
  const authoritative = {
    ...before,
    revisions: { global: 4, publication: 5 },
    policy: {
      ...before.policy,
      publication: { activeContent: "disabled", remoteResources: "blocked" },
    },
  } as const;
  const changes = { activeContent: null, remoteResources: null };
  const saved = {
    ...(conflict ? authoritative : before),
    status: "saved",
    policy: { ...before.policy, publication: changes },
    effective: { activeContent: "enabled", remoteResources: "ask" },
    revisions: conflict ? { global: 4, publication: 6 } : { global: 2, publication: 4 },
  };
  const request = vi.fn<typeof fetch>();
  if (conflict)
    request.mockResolvedValueOnce(
      response({ status: "conflict", reason: "revision-mismatch", authoritative }, 409),
    );
  request.mockResolvedValueOnce(response(saved));
  await expect(
    savePublicationPolicyWithRetry({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      libraryDocumentId: "library",
      initial: before,
      changes,
      request,
    }),
  ).resolves.toStrictEqual(saved);
  expect(request).toHaveBeenCalledTimes(conflict ? 2 : 1);
  for (const [index, [, options]] of request.mock.calls.entries()) {
    expect(JSON.parse(options?.body as string)).toStrictEqual({
      baseRevisions: index === 0 ? before.revisions : authoritative.revisions,
      policy: changes,
    });
    expect(options?.headers).toMatchObject({
      Authorization: "Bearer secret",
      "X-Elephant-Library-Document": "library",
    });
  }
});

test("policy save retries one revision mismatch using authoritative state", async () => {
  const authoritative: PublicationPolicyState = {
    policy: {
      global: { activeContent: "disabled", remoteResources: "ask" },
      publication: { activeContent: null, remoteResources: null },
    },
    effective: { activeContent: "disabled", remoteResources: "ask" },
    revisions: { global: 4, publication: 5 },
  };
  const saved = {
    ...authoritative,
    status: "saved",
    policy: {
      ...authoritative.policy,
      publication: { activeContent: null, remoteResources: "allowed" },
    },
    effective: { activeContent: "disabled", remoteResources: "allowed" },
    revisions: { global: 4, publication: 6 },
  } as const;
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(
      response({ status: "conflict", reason: "revision-mismatch", authoritative }, 409),
    )
    .mockResolvedValueOnce(response(saved));

  await expect(save(request, "allowed")).resolves.toStrictEqual(saved);
  expect(request).toHaveBeenCalledTimes(2);
  expect(JSON.parse(request.mock.calls[0]?.[1]?.body as string)).toStrictEqual({
    baseRevisions: initial.revisions,
    policy: { activeContent: "disabled", remoteResources: "allowed" },
  });
  expect(JSON.parse(request.mock.calls[1]?.[1]?.body as string)).toStrictEqual({
    baseRevisions: authoritative.revisions,
    policy: { activeContent: null, remoteResources: "allowed" },
  });
});

test("policy save does not retry exhausted, malformed, or repeated conflicts", async () => {
  const exhausted = vi
    .fn<typeof fetch>()
    .mockResolvedValue(
      response({ status: "conflict", reason: "revision-exhausted", authoritative: initial }, 409),
    );
  await expect(save(exhausted, "blocked")).rejects.toThrow("revision is exhausted");
  expect(exhausted).toHaveBeenCalledTimes(1);

  const malformed = vi.fn<typeof fetch>().mockResolvedValue(response({ status: "conflict" }, 409));
  await expect(save(malformed, "blocked")).rejects.toThrow("invalid conflict data");
  expect(malformed).toHaveBeenCalledTimes(1);

  const repeated = vi
    .fn<typeof fetch>()
    .mockImplementation(() =>
      Promise.resolve(
        response({ status: "conflict", reason: "revision-mismatch", authoritative: initial }, 409),
      ),
    );
  await expect(save(repeated, "blocked")).rejects.toThrow("conflicted again after retry");
  expect(repeated).toHaveBeenCalledTimes(2);
});

function save(request: typeof fetch, remoteResources: "allowed" | "blocked") {
  return savePublicationPolicyWithRetry({
    url: "/native/publications/book/policy",
    sessionToken: "secret",
    libraryDocumentId: "library",
    initial,
    changes: { remoteResources },
    request,
  });
}

const decision: PolicyDecisionResponse = {
  ...initial,
  policy: {
    global: { activeContent: "ask", remoteResources: "ask" },
    publication: { activeContent: null, remoteResources: null },
  },
  effective: { activeContent: "ask", remoteResources: "ask" },
  status: "policy-decision-required",
  required: { activeContent: true, remoteResources: true },
  relevance: { activeContent: "detected", remoteResources: "unknown" },
};

test.each([
  [true, true, "enabled", "allowed"],
  [true, false, "enabled", "blocked"],
  [false, true, "disabled", "allowed"],
  [false, false, "disabled", "blocked"],
] as const)(
  "collects independent active=%s and network=%s choices",
  async (active, network, activeContent, remoteResources) => {
    const prompt = vi
      .fn<(value: PublicationCapabilityPrompt, signal: AbortSignal) => Promise<boolean | null>>()
      .mockResolvedValueOnce(active)
      .mockResolvedValueOnce(network);
    const signal = new AbortController().signal;
    await expect(
      collectPublicationPolicyChoices(decision, "A book", prompt, signal),
    ).resolves.toEqual({ activeContent, remoteResources });
    expect(prompt.mock.calls.map(([value]) => value.title)).toEqual([
      "Allow active content?",
      "Allow network access?",
    ]);
    expect(prompt.mock.calls[0]?.[0]).toMatchObject({
      description: expect.stringContaining('"A book" contains active content.'),
      denyLabel: "Keep disabled",
    });
    expect(prompt.mock.calls[1]?.[0]).toMatchObject({
      description: expect.stringContaining("could not be fully checked for remote resources"),
      denyLabel: "Keep blocked",
    });
    expect(prompt.mock.calls.every(([, suppliedSignal]) => suppliedSignal === signal)).toBe(true);
  },
);

test.each(["activeContent", "remoteResources"] as const)(
  "asks only for required %s",
  async (capability) => {
    const prompt = vi.fn().mockResolvedValue(false);
    const required = {
      activeContent: capability === "activeContent",
      remoteResources: capability === "remoteResources",
    };
    const result = await collectPublicationPolicyChoices(
      {
        ...decision,
        required,
        relevance: { activeContent: "unknown", remoteResources: "detected" },
      },
      "A book",
      prompt,
      new AbortController().signal,
    );
    expect(result).toEqual({
      [capability]: capability === "activeContent" ? "disabled" : "blocked",
    });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(prompt.mock.calls[0]?.[0].description).toContain(
      capability === "activeContent"
        ? "could not be fully checked for active content"
        : "uses remote resources",
    );
  },
);

test.each([0, 1])("cancelling prompt %s discards all pending choices", async (index) => {
  const prompt = vi.fn().mockResolvedValue(null);
  if (index === 1) prompt.mockResolvedValueOnce(true);
  await expect(
    collectPublicationPolicyChoices(decision, "A book", prompt, new AbortController().signal),
  ).resolves.toBeNull();
  expect(prompt).toHaveBeenCalledTimes(index + 1);
});

test("aborting never advances to the next question or returns pending choices", async () => {
  const controller = new AbortController();
  const prompt = vi.fn().mockImplementation(async () => {
    controller.abort();
    return true;
  });
  await expect(
    collectPublicationPolicyChoices(decision, "A book", prompt, controller.signal),
  ).resolves.toBeNull();
  expect(prompt).toHaveBeenCalledTimes(1);
  prompt.mockClear();
  await expect(
    collectPublicationPolicyChoices(decision, "A book", prompt, controller.signal),
  ).resolves.toBeNull();
  expect(prompt).not.toHaveBeenCalled();
});

test("active-only save preserves authoritative network override on conflict", async () => {
  const authoritative = {
    ...initial,
    policy: { ...initial.policy, publication: { activeContent: null, remoteResources: "blocked" } },
    revisions: { global: 3, publication: 4 },
  } as const;
  const saved = {
    ...authoritative,
    status: "saved",
    policy: {
      ...authoritative.policy,
      publication: { activeContent: "enabled", remoteResources: "blocked" },
    },
    effective: { activeContent: "enabled", remoteResources: "blocked" },
    revisions: { global: 3, publication: 5 },
  } as const;
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(
      response({ status: "conflict", reason: "revision-mismatch", authoritative }, 409),
    )
    .mockResolvedValueOnce(response(saved));
  await expect(
    savePublicationPolicyWithRetry({
      url: "/native/publications/book/policy",
      sessionToken: "secret",
      libraryDocumentId: "library",
      initial,
      changes: { activeContent: "enabled" },
      request,
    }),
  ).resolves.toEqual(saved);
  expect(JSON.parse(request.mock.calls[1]?.[1]?.body as string)).toEqual({
    baseRevisions: authoritative.revisions,
    policy: { activeContent: "enabled", remoteResources: "blocked" },
  });
});

test("both choices are saved in one revisioned update", async () => {
  const changes = { activeContent: "disabled", remoteResources: "allowed" } as const;
  const saved = {
    ...initial,
    status: "saved",
    policy: { ...initial.policy, publication: changes },
    effective: changes,
  };
  const request = vi.fn<typeof fetch>().mockResolvedValue(response(saved));
  await savePublicationPolicyWithRetry({
    url: "/native/publications/book/policy",
    sessionToken: "secret",
    libraryDocumentId: "library",
    initial,
    changes,
    request,
  });
  expect(request).toHaveBeenCalledTimes(1);
  expect(JSON.parse(request.mock.calls[0]?.[1]?.body as string)).toEqual({
    baseRevisions: initial.revisions,
    policy: changes,
  });
});

test("aborted policy saves neither start nor retry", async () => {
  const controller = new AbortController();
  const request = vi.fn<typeof fetch>().mockImplementation(async () => {
    controller.abort();
    return response({ ...initial, status: "conflict", reason: "revision-mismatch" }, 409);
  });
  const options = {
    url: "/native/publications/book/policy",
    sessionToken: "secret",
    libraryDocumentId: "library",
    initial,
    changes: { activeContent: "enabled" } as const,
    request,
    signal: controller.signal,
  };
  await expect(savePublicationPolicyWithRetry(options)).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(request).toHaveBeenCalledTimes(1);
  await expect(savePublicationPolicyWithRetry(options)).rejects.toMatchObject({
    name: "AbortError",
  });
  expect(request).toHaveBeenCalledTimes(1);
});
