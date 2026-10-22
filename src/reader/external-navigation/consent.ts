import { nativeRequest } from "../../host/requests";

/**
 * Where a link leaves the reader for. A web link names an origin; a mail link has none, so it
 * names the one recipient the message would go to.
 */
export type ExternalNavigationPrompt =
  | { readonly kind: "browser"; readonly url: string; readonly origin: string }
  | { readonly kind: "mail"; readonly url: string; readonly address: string };

export function createExternalNavigationConsent({
  readerSessionId,
  sessionToken,
  showPrompt,
  reportError,
  request = fetch,
  now = Date.now,
}: {
  readonly readerSessionId: string;
  readonly sessionToken: string;
  readonly showPrompt: (prompt: ExternalNavigationPrompt | null) => void;
  readonly reportError: (message: string) => void;
  readonly request?: typeof fetch;
  readonly now?: () => number;
}) {
  let operation: AbortController | null = null;
  let pending: ExternalNavigationPrompt | null = null;
  let closed = false;
  let blockedUntil = 0;

  const cancel = (): void => {
    operation?.abort();
    operation = null;
    pending = null;
    blockedUntil = now() + 1_000;
    showPrompt(null);
  };
  const post = (path: string, url: string, signal: AbortSignal): Promise<Response> =>
    nativeRequest(
      path,
      { sessionToken, readerSessionId },
      { method: "POST", json: { url }, signal, fetcher: request },
    );

  return {
    readerSessionId,
    cancel,
    close(): void {
      closed = true;
      cancel();
    },
    async requestOpen(url: string): Promise<void> {
      // One preparation/dialog/dispatch at a time, with no queue or destination replacement.
      if (closed || operation !== null || now() < blockedUntil) return;
      const owner = new AbortController();
      operation = owner;
      const timeout = setTimeout(() => owner.abort(), 5_000);
      try {
        const response = await post("/native/prepare-external-url", url, owner.signal);
        if (!response.ok) throw new Error(`Destination check failed with HTTP ${response.status}`);
        const value: unknown = await response.json();
        owner.signal.throwIfAborted();
        if (closed || operation !== owner) return;
        if (value === null || typeof value !== "object") throw new Error("Invalid destination");
        const destination = value as Record<string, unknown>;
        if (Object.keys(destination).length !== 2 || typeof destination.url !== "string")
          throw new Error("Invalid destination");
        const parsed = new URL(destination.url);
        if (typeof destination.address === "string") {
          if (parsed.protocol !== "mailto:" || !parsed.pathname.includes("@"))
            throw new Error("Invalid destination");
          pending = { kind: "mail", url: destination.url, address: destination.address };
        } else {
          if (
            typeof destination.origin !== "string" ||
            !["http:", "https:"].includes(parsed.protocol) ||
            parsed.username !== "" ||
            parsed.password !== "" ||
            parsed.origin !== destination.origin
          )
            throw new Error("Invalid destination");
          pending = { kind: "browser", url: destination.url, origin: destination.origin };
        }
        showPrompt(pending);
      } catch {
        if (!closed && operation === owner) {
          cancel();
          reportError("External link could not be checked. Nothing was opened.");
        }
      } finally {
        clearTimeout(timeout);
      }
    },
    async decide(prompt: ExternalNavigationPrompt, allowed: boolean): Promise<void> {
      if (closed || pending !== prompt || operation === null) return;
      if (!allowed) {
        cancel();
        return;
      }
      const owner = operation;
      pending = null;
      showPrompt(null);
      const timeout = setTimeout(() => owner.abort(), 5_000);
      try {
        owner.signal.throwIfAborted();
        const response = await post("/native/open-external-url", prompt.url, owner.signal);
        if (!response.ok) throw new Error(`External link failed with HTTP ${response.status}`);
      } catch {
        if (!closed && operation === owner) {
          reportError(
            prompt.kind === "mail"
              ? "The email application could not be opened or its response was not received."
              : "The browser could not be opened or its response was not received.",
          );
        }
      } finally {
        clearTimeout(timeout);
        if (operation === owner) {
          operation = null;
          blockedUntil = now() + 1_000;
        }
      }
    },
  };
}

export type ExternalNavigationConsent = ReturnType<typeof createExternalNavigationConsent>;
