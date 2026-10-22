import type { PublicationSearchCapabilities } from "epub-navigator/runtime";
import { nativeRequest } from "../../host/requests";

type SearchOutcome = Awaited<ReturnType<PublicationSearchCapabilities["search"]>>;

const unavailableReasons = new Set([
  "index-pending",
  "index-unavailable",
  "scope-unavailable",
  "policy",
  "snapshot-unavailable",
]);

export function createNativeSearchCapabilities(
  readerSessionId: string,
  sessionToken: string,
): PublicationSearchCapabilities {
  const controller = new AbortController();
  let closed = false;

  return {
    scopes: ["publication", "occurrence"],
    signal: controller.signal,
    async search(request, context) {
      const combined = combineSignals(controller.signal, context.signal);
      try {
        const response = await nativeRequest(
          "/native/search",
          { sessionToken },
          { method: "POST", json: { readerSessionId, request }, signal: combined.signal },
        );
        if (response.status === 409) {
          return { status: "unavailable", reason: "snapshot-unavailable", message: null };
        }

        let value: unknown;
        try {
          value = await response.json();
        } catch (error) {
          if (combined.signal.aborted) throw error;
          return { status: "failed", message: null };
        }

        if (!response.ok) {
          return isUnavailableOutcome(value) || isFailedOutcome(value)
            ? value
            : { status: "failed", message: null };
        }
        return isSearchOutcome(value) ? value : { status: "failed", message: null };
      } catch (error) {
        if (combined.signal.aborted) throw error;
        return { status: "failed", message: null };
      } finally {
        combined.cleanup();
      }
    },
    close() {
      if (closed) return;
      closed = true;
      controller.abort();
    },
  };
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

function isSearchOutcome(value: unknown): value is SearchOutcome {
  return isCompletedOutcome(value) || isUnavailableOutcome(value) || isFailedOutcome(value);
}

function isCompletedOutcome(
  value: unknown,
): value is Extract<SearchOutcome, { status: "completed" }> {
  return (
    isExactRecord(value, ["status", "batch"]) &&
    value.status === "completed" &&
    isBatch(value.batch)
  );
}

function isUnavailableOutcome(
  value: unknown,
): value is Extract<SearchOutcome, { status: "unavailable" }> {
  return (
    isExactRecord(value, ["status", "reason", "message"]) &&
    value.status === "unavailable" &&
    typeof value.reason === "string" &&
    unavailableReasons.has(value.reason) &&
    isNullableString(value.message)
  );
}

function isFailedOutcome(value: unknown): value is Extract<SearchOutcome, { status: "failed" }> {
  return (
    isExactRecord(value, ["status", "message"]) &&
    value.status === "failed" &&
    isNullableString(value.message)
  );
}

function isBatch(value: unknown): boolean {
  return (
    isExactRecord(value, [
      "requestId",
      "results",
      "coverage",
      "continuation",
      "totalResultCount",
    ]) &&
    isNonEmptyString(value.requestId) &&
    Array.isArray(value.results) &&
    value.results.every(isResult) &&
    isCoverage(value.coverage) &&
    (value.continuation === null || isNonEmptyString(value.continuation)) &&
    (value.totalResultCount === null || isCount(value.totalResultCount))
  );
}

function isResult(value: unknown): boolean {
  return (
    isExactRecord(value, ["id", "section", "targetJson", "excerpt"]) &&
    isNonEmptyString(value.id) &&
    isSection(value.section) &&
    isNonEmptyString(value.targetJson) &&
    isExcerpt(value.excerpt)
  );
}

function isSection(value: unknown): boolean {
  return (
    isExactRecord(value, ["id", "occurrence", "resource", "label"]) &&
    isNonEmptyString(value.id) &&
    isCount(value.occurrence) &&
    isNonEmptyString(value.resource) &&
    isNullableString(value.label)
  );
}

function isExcerpt(value: unknown): boolean {
  return (
    isExactRecord(value, ["before", "match", "after"]) &&
    typeof value.before === "string" &&
    isNonEmptyString(value.match) &&
    typeof value.after === "string"
  );
}

function isCoverage(value: unknown): boolean {
  if (!isRecord(value)) return false;
  const countsAreValid =
    isCount(value.searchedOccurrences) &&
    isCount(value.totalOccurrences) &&
    value.searchedOccurrences <= value.totalOccurrences;
  if (value.state === "complete") {
    return (
      isExactRecord(value, ["state", "searchedOccurrences", "totalOccurrences"]) && countsAreValid
    );
  }
  return (
    value.state === "partial" &&
    isExactRecord(value, ["state", "searchedOccurrences", "totalOccurrences", "reason"]) &&
    countsAreValid &&
    isNullableString(value.reason)
  );
}

function isCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string";
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return (
    isRecord(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.prototype.hasOwnProperty.call(value, key))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
