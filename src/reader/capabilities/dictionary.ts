import type {
  NavigatorDictionaryEntry,
  NavigatorDictionaryLookup,
  NavigatorDictionarySectionPage,
  NavigatorDictionaryUnavailableReason,
} from "epub-navigator";
import type { PublicationDictionaryCapabilities } from "epub-navigator/runtime";
import { nativeRequest } from "../../host/requests";

export interface DictionaryViewCapability {
  viewId: string;
  viewType: "monolingual" | "bilingual";
  sourceName: string;
  editionLanguage: string | null;
  sourceLanguage: string;
  targetLanguage: string | null;
}

export interface DictionaryCapability {
  revision: string;
  views: DictionaryViewCapability[];
  audioLanguages: string[];
}

type Outcome<T> =
  | ({ readonly status: "completed" } & T)
  | { readonly status: "unavailable"; readonly reason: NavigatorDictionaryUnavailableReason }
  | { readonly status: "failed"; readonly message: string | null };

const unavailableReasons = new Set<string>([
  "snapshot-unavailable",
  "entry-unavailable",
  "audio-unavailable",
]);

export function createNativeDictionaryCapabilities(
  readerSessionId: string,
  sessionToken: string,
  capability: DictionaryCapability,
): PublicationDictionaryCapabilities {
  const controller = new AbortController();
  let closed = false;
  const request = async <T>(
    route: string,
    body: Record<string, unknown>,
    signal: AbortSignal,
    completed: (response: Response) => Promise<T | null>,
  ): Promise<Outcome<T>> => {
    const combined = combineSignals(controller.signal, signal);
    try {
      const response = await nativeRequest(
        `/native/dictionary/${route}`,
        { sessionToken },
        {
          method: "POST",
          json: { readerSessionId, revision: capability.revision, ...body },
          signal: combined.signal,
        },
      );
      if (response.status === 409) return { status: "unavailable", reason: "snapshot-unavailable" };
      if (response.ok) {
        const value = await completed(response);
        return value === null
          ? { status: "failed", message: null }
          : { status: "completed", ...value };
      }
      const value: unknown = await response.json().catch(() => null);
      if (
        isRecord(value) &&
        value.status === "unavailable" &&
        typeof value.reason === "string" &&
        unavailableReasons.has(value.reason)
      ) {
        return {
          status: "unavailable",
          reason: value.reason as NavigatorDictionaryUnavailableReason,
        };
      }
      return {
        status: "failed",
        message: isRecord(value) && typeof value.message === "string" ? value.message : null,
      };
    } catch (error) {
      if (combined.signal.aborted) throw error;
      return { status: "failed", message: null };
    } finally {
      combined.cleanup();
    }
  };
  const json = async (response: Response): Promise<Record<string, unknown> | null> => {
    const value: unknown = await response.json();
    return isRecord(value) && value.status === "completed" ? value : null;
  };

  return {
    views: capability.views,
    signal: controller.signal,
    lookup: (lookup, context) =>
      request(
        "lookup",
        { requestId: lookup.requestId, text: lookup.text, language: lookup.language },
        context.signal,
        async (response) => {
          const value = await json(response);
          return value === null
            ? null
            : {
                // The Navigator coordinator validates every field of provider values.
                lookup: {
                  requestId: value.requestId,
                  query: value.query,
                  views: value.views,
                } as NavigatorDictionaryLookup,
              };
        },
      ),
    entry: (handle, context) =>
      request("entry", { entry: handle }, context.signal, async (response) => {
        const value = await json(response);
        return value === null ? null : { entry: value.entry as NavigatorDictionaryEntry };
      }),
    section: (handle, section, page, context) =>
      request(
        "section",
        { entry: handle, section, offset: page.offset, limit: page.limit },
        context.signal,
        async (response) => {
          const value = await json(response);
          return value === null
            ? null
            : { section: value.section as NavigatorDictionarySectionPage };
        },
      ),
    audio: (handle, context) =>
      request("audio", { audio: handle }, context.signal, async (response) => {
        const type = response.headers.get("Content-Type") ?? "";
        if (!type.startsWith("audio/")) return null;
        return { blob: new Blob([await response.arrayBuffer()], { type }) };
      }),
    close() {
      if (closed) return;
      closed = true;
      controller.abort();
    },
  };
}

export function isDictionaryCapability(value: unknown): value is DictionaryCapability {
  if (!isRecord(value) || !hasKeys(value, ["revision", "views", "audioLanguages"])) {
    return false;
  }
  return (
    typeof value.revision === "string" &&
    /^[0-9a-f]{64}$/.test(value.revision) &&
    Array.isArray(value.views) &&
    value.views.length > 0 &&
    value.views.every(isViewCapability) &&
    Array.isArray(value.audioLanguages) &&
    value.audioLanguages.every((language) => typeof language === "string")
  );
}

function isViewCapability(value: unknown): value is DictionaryViewCapability {
  if (
    !isRecord(value) ||
    !hasKeys(value, [
      "viewId",
      "viewType",
      "sourceName",
      "editionLanguage",
      "sourceLanguage",
      "targetLanguage",
    ]) ||
    typeof value.viewId !== "string" ||
    typeof value.sourceName !== "string" ||
    (value.editionLanguage !== null && typeof value.editionLanguage !== "string") ||
    typeof value.sourceLanguage !== "string"
  ) {
    return false;
  }
  return value.viewType === "monolingual"
    ? value.targetLanguage === null
    : value.viewType === "bilingual" && typeof value.targetLanguage === "string";
}

function combineSignals(
  capabilitySignal: AbortSignal,
  contextSignal: AbortSignal,
): { readonly signal: AbortSignal; readonly cleanup: () => void } {
  if (typeof AbortSignal.any === "function") {
    return { signal: AbortSignal.any([capabilitySignal, contextSignal]), cleanup: () => {} };
  }
  const controller = new AbortController();
  const signals = [capabilitySignal, contextSignal];
  const abort = () => controller.abort();
  if (signals.some((signal) => signal.aborted)) {
    abort();
  } else {
    for (const signal of signals) signal.addEventListener("abort", abort, { once: true });
  }
  return {
    signal: controller.signal,
    cleanup: () => {
      for (const signal of signals) signal.removeEventListener("abort", abort);
    },
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && keys.every((key) => actual.includes(key));
}
