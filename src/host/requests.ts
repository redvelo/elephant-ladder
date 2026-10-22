import { isNavigationPath } from "./navigation";
import { createTrustedNavigatorDiagnostics } from "./diagnostics/adapter";
import type { TrustedDiagnosticBatch, TrustedNavigatorDiagnostics } from "./diagnostics/records";
import { isRecord } from "./guards";

export interface NavigationResponse {
  status: "navigate";
  navigationPath: string;
}

/** Who the host should believe a request comes from, and which records it may touch. */
export interface NativeAuthority {
  readonly sessionToken: string;
  readonly libraryDocumentId?: string | undefined;
  readonly readerSessionId?: string | undefined;
  /** Marks the request as issued by the trusted reader document rather than a publication. */
  readonly trusted?: boolean | undefined;
}

export interface NativeRequestInit {
  readonly method?: "GET" | "POST" | "PUT" | "DELETE" | undefined;
  /** Serialized as JSON, which also sets the content type. */
  readonly json?: unknown;
  readonly body?: BodyInit | undefined;
  readonly contentType?: string | undefined;
  readonly headers?: Readonly<Record<string, string>> | undefined;
  readonly signal?: AbortSignal | undefined;
  readonly keepalive?: boolean | undefined;
  /** The fetch to use. Modules that are tested against a stub pass theirs. */
  readonly fetcher?: typeof fetch | undefined;
}

/**
 * One request to the native host, carrying the session token and the authority headers the host
 * validates. Callers keep their own status handling: what a status means belongs to the flow.
 */
export function nativeRequest(
  path: string,
  authority: NativeAuthority,
  init: NativeRequestInit = {},
): Promise<Response> {
  // Not async: an extra microtask between the call and the fetch would reorder a caller that
  // decides something in the response handler while more work is already queued.
  if (!path.startsWith("/native/")) {
    return Promise.reject(new Error(`Refusing to send the session token to ${path}`));
  }
  const headers: Record<string, string> = {
    Authorization: `Bearer ${authority.sessionToken}`,
  };
  if (authority.trusted === true) headers["X-Elephant-Request"] = "trusted-reader";
  if (authority.libraryDocumentId !== undefined) {
    headers["X-Elephant-Library-Document"] = authority.libraryDocumentId;
  }
  if (authority.readerSessionId !== undefined) {
    headers["X-Elephant-Reader-Session"] = authority.readerSessionId;
  }
  if (init.json !== undefined) headers["Content-Type"] = "application/json";
  if (init.contentType !== undefined) headers["Content-Type"] = init.contentType;
  return (init.fetcher ?? fetch)(path, {
    cache: "no-store",
    ...(init.method === undefined ? {} : { method: init.method }),
    headers: { ...headers, ...init.headers },
    ...(init.json === undefined ? {} : { body: JSON.stringify(init.json) }),
    ...(init.body === undefined ? {} : { body: init.body }),
    ...(init.signal === undefined ? {} : { signal: init.signal }),
    ...(init.keepalive === true ? { keepalive: true } : {}),
  });
}

export async function authenticatedPostEmpty(
  path: string,
  token: string,
  body: unknown,
  signal?: AbortSignal,
): Promise<void> {
  const response = await nativeRequest(
    path,
    { sessionToken: token },
    {
      method: "POST",
      json: body,
      ...(signal === undefined ? {} : { signal }),
    },
  );
  if (!response.ok) throw new Error(`${path} failed with HTTP ${response.status}`);
}

export function createTrustedDiagnostics(
  level: "summary" | "trace",
  token: string,
): TrustedNavigatorDiagnostics {
  return createTrustedNavigatorDiagnostics({
    level,
    async deliver(batch: TrustedDiagnosticBatch, signal: AbortSignal) {
      await authenticatedPostEmpty("/native/diagnostics/v1", token, batch, signal);
    },
  });
}

export function isNavigationResponse(value: unknown): value is NavigationResponse {
  return (
    isRecord(value) &&
    Object.keys(value).length === 2 &&
    value.status === "navigate" &&
    isNavigationPath(value.navigationPath)
  );
}

export function isAbortError(value: unknown): boolean {
  return value instanceof DOMException && value.name === "AbortError";
}

export function createPerformanceTraceId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function describeError(value: unknown): string {
  return value instanceof Error ? `${value.name}: ${value.message}` : String(value);
}
