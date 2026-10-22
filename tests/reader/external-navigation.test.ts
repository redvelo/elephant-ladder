import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createExternalNavigationConsent,
  type ExternalNavigationPrompt,
} from "../../src/reader/external-navigation/consent";

const destination = { url: "https://example.test/read?q=1#part", origin: "https://example.test" };
const owners: ReturnType<typeof createExternalNavigationConsent>[] = [];

function harness(
  request = vi
    .fn<typeof fetch>()
    .mockImplementation(async (path) =>
      (path instanceof Request ? path.url : String(path)).includes("prepare-")
        ? Response.json(destination)
        : new Response(null, { status: 204 }),
    ),
) {
  const showPrompt = vi.fn<(prompt: ExternalNavigationPrompt | null) => void>();
  const reportError = vi.fn();
  const owner = createExternalNavigationConsent({
    readerSessionId: "session",
    sessionToken: "secret",
    showPrompt,
    reportError,
    request,
  });
  owners.push(owner);
  return {
    owner,
    request,
    showPrompt,
    reportError,
    prompt() {
      const value = showPrompt.mock.calls.at(-1)?.[0];
      if (!value) throw new Error("Expected a pending prompt");
      return value;
    },
  };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  owners.splice(0).forEach((owner) => owner.close());
  vi.useRealTimers();
});

describe("external navigation consent", () => {
  it("prepares without opening, then opens exactly the displayed normalized destination", async () => {
    const h = harness();
    const authored = "HTTPS://EXAMPLE.test:443/a/../read?q=1#part";
    await h.owner.requestOpen(authored);
    expect(h.prompt()).toEqual({ kind: "browser", ...destination });
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.request).toHaveBeenCalledWith("/native/prepare-external-url", {
      method: "POST",
      cache: "no-store",
      headers: {
        Authorization: "Bearer secret",
        "X-Elephant-Reader-Session": "session",
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ url: authored }),
      signal: expect.any(AbortSignal),
    });
    await h.owner.decide(h.prompt(), true);
    expect(h.request).toHaveBeenCalledTimes(2);
    expect(h.request.mock.calls[1]?.[0]).toBe("/native/open-external-url");
    expect(JSON.parse(h.request.mock.calls[1]?.[1]?.body as string)).toEqual({
      url: destination.url,
    });
    expect(h.showPrompt).toHaveBeenLastCalledWith(null);
    expect(h.reportError).not.toHaveBeenCalled();
  });

  it("has no queue, does not replace destinations, and cools down after cancellation", async () => {
    const h = harness();
    await h.owner.requestOpen(destination.url);
    const first = h.prompt();
    for (let i = 0; i < 100; i++) await h.owner.requestOpen(`https://other.test/${i}`);
    expect(h.prompt()).toBe(first);
    await h.owner.decide(first, false);
    await h.owner.requestOpen(destination.url);
    expect(h.request).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1_000);
    await h.owner.requestOpen(destination.url);
    expect(h.request).toHaveBeenCalledTimes(2);
    expect(h.request.mock.calls.every(([path]) => path === "/native/prepare-external-url")).toBe(
      true,
    );
  });

  it("names the recipient for a mail link, which has no origin to name", async () => {
    const mail = { url: "mailto:someone@example.test?subject=Hi", address: "someone@example.test" };
    const h = harness(
      vi
        .fn<typeof fetch>()
        .mockImplementation(async (path) =>
          (path instanceof Request ? path.url : String(path)).includes("prepare-")
            ? Response.json(mail)
            : new Response(null, { status: 204 }),
        ),
    );
    await h.owner.requestOpen("mailto:someone@example.test?subject=Hi");
    expect(h.prompt()).toEqual({ kind: "mail", ...mail });
    await h.owner.decide(h.prompt(), true);
    expect(h.request).toHaveBeenLastCalledWith(
      "/native/open-external-url",
      expect.objectContaining({ body: JSON.stringify({ url: mail.url }) }),
    );
    expect(h.reportError).not.toHaveBeenCalled();
  });

  it("refuses a mail destination the reader could not have been shown honestly", async () => {
    for (const destination of [
      { url: "mailto:someone@example.test", address: 5 },
      { url: "https://example.test/", address: "someone@example.test" },
      { url: "mailto:nobody", address: "nobody" },
    ]) {
      const h = harness(
        vi
          .fn<typeof fetch>()
          .mockImplementation(async (path) =>
            (path instanceof Request ? path.url : String(path)).includes("prepare-")
              ? Response.json(destination)
              : new Response(null, { status: 204 }),
          ),
      );
      await h.owner.requestOpen("mailto:someone@example.test");
      expect(h.showPrompt).toHaveBeenLastCalledWith(null);
      expect(h.reportError).toHaveBeenCalled();
    }
  });

  it("accepts one approval only and rejects stale or fabricated prompt objects", async () => {
    const h = harness();
    await h.owner.requestOpen(destination.url);
    const first = h.prompt();
    await h.owner.decide({ ...first }, true);
    expect(h.request).toHaveBeenCalledTimes(1);
    await Promise.all([h.owner.decide(first, true), h.owner.decide(first, true)]);
    expect(h.request).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1_000);
    await h.owner.requestOpen(destination.url);
    const second = h.prompt();
    await h.owner.decide(first, false);
    expect(h.prompt()).toBe(second);
  });

  it("does not show late preparation after cancellation or close", async () => {
    let finish!: (response: Response) => void;
    const request = vi.fn<typeof fetch>().mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const h = harness(request);
    const preparing = h.owner.requestOpen(destination.url);
    await h.owner.requestOpen("https://other.test");
    expect(request).toHaveBeenCalledTimes(1);
    h.owner.close();
    expect(request.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    finish(Response.json(destination));
    await preparing;
    expect(h.showPrompt.mock.calls).toEqual([[null]]);
    expect(h.reportError).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(10_000);
    await h.owner.requestOpen(destination.url);
    expect(request).toHaveBeenCalledTimes(1);
  });

  it("cancels pending consent when its reading session closes", async () => {
    const h = harness();
    await h.owner.requestOpen(destination.url);
    const prompt = h.prompt();
    h.owner.close();
    await h.owner.decide(prompt, true);
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.showPrompt).toHaveBeenLastCalledWith(null);
  });

  it("ignores requests while platform dispatch is pending", async () => {
    let finish!: (response: Response) => void;
    const h = harness(
      vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(Response.json(destination))
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              finish = resolve;
            }),
        ),
    );
    await h.owner.requestOpen(destination.url);
    const opening = h.owner.decide(h.prompt(), true);
    await h.owner.requestOpen("https://other.test/");
    expect(h.request).toHaveBeenCalledTimes(2);
    finish(new Response(null, { status: 204 }));
    await opening;
    await h.owner.requestOpen(destination.url);
    expect(h.request).toHaveBeenCalledTimes(2);
  });

  it.each([
    null,
    { url: "javascript:alert(1)", origin: "null" },
    { url: "https://user:secret@example.test/", origin: "https://example.test" },
    { ...destination, origin: "https://other.test" },
    { ...destination, extra: true },
  ])("rejects invalid preparation without opening: %j", async (value) => {
    const h = harness(vi.fn<typeof fetch>().mockResolvedValue(Response.json(value)));
    await h.owner.requestOpen(destination.url);
    expect(h.request).toHaveBeenCalledTimes(1);
    expect(h.showPrompt).toHaveBeenLastCalledWith(null);
    expect(h.reportError).toHaveBeenCalledOnce();
  });

  it("times out preparation without leaving a stuck dialog owner", async () => {
    const h = harness(
      vi.fn<typeof fetch>().mockImplementation(
        (_url, init) =>
          new Promise((_resolve, reject) => {
            init?.signal?.addEventListener("abort", () =>
              reject(new DOMException("Aborted", "AbortError")),
            );
          }),
      ),
    );
    const pending = h.owner.requestOpen(destination.url);
    await vi.advanceTimersByTimeAsync(5_000);
    await pending;
    expect(h.reportError).toHaveBeenCalledOnce();
    expect(h.showPrompt).toHaveBeenLastCalledWith(null);
    h.request.mockResolvedValueOnce(Response.json(destination));
    await vi.advanceTimersByTimeAsync(1_000);
    await h.owner.requestOpen(destination.url);
    expect(h.prompt()).toEqual({ kind: "browser", ...destination });
  });

  it("reports failed dispatch without reopening or automatically retrying", async () => {
    const h = harness(
      vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(Response.json(destination))
        .mockResolvedValueOnce(new Response(null, { status: 503 })),
    );
    const opening = h.owner.requestOpen(destination.url);
    await opening;
    await h.owner.decide(h.prompt(), true);
    expect(h.reportError).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(h.request).toHaveBeenCalledTimes(2);
  });
});
