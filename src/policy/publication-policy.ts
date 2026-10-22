import { nativeRequest } from "../host/requests";

export interface PublicationPolicyRevisions {
  readonly global: number;
  readonly publication: number;
}

export interface PublicationPolicyState {
  readonly policy: {
    readonly global: {
      readonly activeContent: "automatic" | "ask" | "disabled";
      readonly remoteResources: "allowed" | "ask" | "blocked";
    };
    readonly publication: PublicationPolicyOverrides;
  };
  readonly effective: {
    readonly activeContent: "enabled" | "ask" | "disabled";
    readonly remoteResources: "allowed" | "ask" | "blocked";
  };
  readonly revisions: PublicationPolicyRevisions;
}

export interface PublicationPolicyOverrides {
  readonly activeContent: "enabled" | "disabled" | null;
  readonly remoteResources: "allowed" | "blocked" | null;
}

export interface PublicationPolicyRelevance {
  readonly activeContent: "detected" | "not-detected" | "unknown";
  readonly remoteResources: "detected" | "not-detected" | "unknown";
}

export interface PublicationPolicyReview extends PublicationPolicyState {
  readonly relevance: PublicationPolicyRelevance;
}

export interface PolicyDecisionResponse extends PublicationPolicyState {
  readonly status: "policy-decision-required";
  readonly required: { readonly activeContent: boolean; readonly remoteResources: boolean };
  readonly relevance: PublicationPolicyRelevance;
}

export interface PublicationCapabilityPrompt {
  readonly title: string;
  readonly description: string;
  readonly allowLabel: string;
  readonly denyLabel: string;
}

/** Collect all choices before saving, so cancelling the second prompt saves neither. */
export async function collectPublicationPolicyChoices(
  decision: PolicyDecisionResponse,
  publicationTitle: string,
  requestDecision: (
    prompt: PublicationCapabilityPrompt,
    signal: AbortSignal,
  ) => Promise<boolean | null>,
  signal: AbortSignal,
): Promise<Partial<PublicationPolicyOverrides> | null> {
  const changes: {
    activeContent?: "enabled" | "disabled";
    remoteResources?: "allowed" | "blocked";
  } = {};
  for (const capability of ["activeContent", "remoteResources"] as const) {
    if (signal.aborted) return null;
    if (!decision.required[capability]) continue;
    const prompt: PublicationCapabilityPrompt =
      capability === "activeContent"
        ? {
            title: "Allow active content?",
            description: `"${publicationTitle}" ${
              decision.relevance.activeContent === "detected"
                ? "contains active content. Allowing it lets publication scripts run and use publication website storage."
                : "could not be fully checked for active content. Allowing it lets any publication scripts run and use publication website storage."
            } Network access is a separate permission. Your choice will be remembered for this publication.`,
            allowLabel: "Allow active content",
            denyLabel: "Keep disabled",
          }
        : {
            title: "Allow network access?",
            description: `"${publicationTitle}" ${
              decision.relevance.remoteResources === "detected"
                ? "uses remote resources."
                : "could not be fully checked for remote resources."
            } Allowing access lets resources load and enabled scripts contact internet services, which can observe network metadata and activity. HTTP resources stay blocked. Your choice will be remembered for this publication.`,
            allowLabel: "Allow network",
            denyLabel: "Keep blocked",
          };
    const allowed = await requestDecision(prompt, signal);
    if (signal.aborted || allowed === null) return null;
    if (capability === "activeContent") changes.activeContent = allowed ? "enabled" : "disabled";
    else changes.remoteResources = allowed ? "allowed" : "blocked";
  }
  return changes;
}

interface SavedPublicationPolicy extends PublicationPolicyState {
  readonly status: "saved";
}

interface PublicationPolicyConflict {
  readonly status: "conflict";
  readonly reason: "revision-mismatch" | "revision-exhausted";
  readonly authoritative: PublicationPolicyState;
}

interface ReadPublicationPolicyOptions {
  readonly url: string;
  readonly sessionToken: string;
  readonly signal?: AbortSignal;
  readonly request?: typeof fetch;
}

export async function readPublicationPolicy({
  url,
  sessionToken,
  signal,
  request = fetch,
}: ReadPublicationPolicyOptions): Promise<PublicationPolicyReview> {
  signal?.throwIfAborted();
  const response = await nativeRequest(
    url,
    { sessionToken, trusted: true },
    { method: "GET", signal, fetcher: request },
  );
  signal?.throwIfAborted();
  if (!response.ok) {
    throw new Error(`Policy read failed with HTTP ${response.status}`);
  }
  let result: unknown;
  try {
    result = await response.json();
  } catch {
    signal?.throwIfAborted();
    throw new Error("Policy read returned invalid data.");
  }
  signal?.throwIfAborted();
  if (!isPublicationPolicyReview(result)) {
    throw new Error("Policy read returned invalid data.");
  }
  return result;
}

interface SavePublicationPolicyOptions {
  readonly url: string;
  readonly sessionToken: string;
  readonly libraryDocumentId: string;
  readonly initial: PublicationPolicyState;
  readonly changes: Partial<PublicationPolicyOverrides>;
  readonly signal?: AbortSignal;
  readonly request?: typeof fetch;
}

export async function savePublicationPolicyWithRetry({
  url,
  sessionToken,
  libraryDocumentId,
  initial,
  changes,
  signal,
  request = fetch,
}: SavePublicationPolicyOptions): Promise<SavedPublicationPolicy> {
  let authoritative = initial;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    signal?.throwIfAborted();
    const response = await nativeRequest(
      url,
      { sessionToken, libraryDocumentId },
      {
        method: "PUT",
        json: {
          baseRevisions: authoritative.revisions,
          policy: { ...authoritative.policy.publication, ...changes },
        },
        signal,
        fetcher: request,
      },
    );
    signal?.throwIfAborted();
    if (!response.ok && response.status !== 409) {
      throw new Error(`Policy update failed with HTTP ${response.status}`);
    }

    let result: unknown;
    try {
      result = await response.json();
    } catch {
      signal?.throwIfAborted();
      throw new Error("Policy update returned invalid data.");
    }
    signal?.throwIfAborted();
    if (response.ok) {
      if (!isSavedPublicationPolicy(result)) {
        throw new Error("Policy update returned invalid data.");
      }
      return result;
    }
    if (!isPublicationPolicyConflict(result)) {
      throw new Error("Policy update returned invalid conflict data.");
    }
    if (result.reason !== "revision-mismatch") {
      throw new Error("Policy update failed because its revision is exhausted.");
    }
    if (attempt !== 0) {
      throw new Error("Policy update conflicted again after retry.");
    }
    authoritative = result.authoritative;
  }
  throw new Error("Policy update retry limit was reached.");
}

function isPublicationPolicyReview(value: unknown): value is PublicationPolicyReview {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["policy", "effective", "revisions", "relevance"]) &&
    isPublicationPolicyState(value) &&
    isRecord(value.relevance) &&
    hasExactKeys(value.relevance, ["activeContent", "remoteResources"]) &&
    ["detected", "not-detected", "unknown"].includes(value.relevance.activeContent as string) &&
    ["detected", "not-detected", "unknown"].includes(value.relevance.remoteResources as string)
  );
}

function isSavedPublicationPolicy(value: unknown): value is SavedPublicationPolicy {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["status", "policy", "effective", "revisions"]) &&
    value.status === "saved" &&
    isPublicationPolicyState(value)
  );
}

function isPublicationPolicyConflict(value: unknown): value is PublicationPolicyConflict {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["status", "reason", "authoritative"]) &&
    value.status === "conflict" &&
    (value.reason === "revision-mismatch" || value.reason === "revision-exhausted") &&
    isRecord(value.authoritative) &&
    isPublicationPolicyState(value.authoritative)
  );
}

function isPublicationPolicyState(
  value: Record<string, unknown>,
): value is Record<string, unknown> & PublicationPolicyState {
  return (
    isPolicy(value.policy) && isEffectivePolicy(value.effective) && isRevisions(value.revisions)
  );
}

function isPolicy(value: unknown): value is PublicationPolicyState["policy"] {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["global", "publication"]) &&
    isRecord(value.global) &&
    hasExactKeys(value.global, ["activeContent", "remoteResources"]) &&
    ["automatic", "ask", "disabled"].includes(value.global.activeContent as string) &&
    ["allowed", "ask", "blocked"].includes(value.global.remoteResources as string) &&
    isPublicationOverrides(value.publication)
  );
}

function isPublicationOverrides(value: unknown): value is PublicationPolicyOverrides {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["activeContent", "remoteResources"]) &&
    [null, "enabled", "disabled"].includes(value.activeContent as null | string) &&
    [null, "allowed", "blocked"].includes(value.remoteResources as null | string)
  );
}

function isEffectivePolicy(value: unknown): value is PublicationPolicyState["effective"] {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["activeContent", "remoteResources"]) &&
    ["enabled", "ask", "disabled"].includes(value.activeContent as string) &&
    ["allowed", "ask", "blocked"].includes(value.remoteResources as string)
  );
}

function isRevisions(value: unknown): value is PublicationPolicyRevisions {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["global", "publication"]) &&
    isRevision(value.global) &&
    isRevision(value.publication)
  );
}

function isRevision(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= 0xffff_ffff;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).length === keys.length && keys.every((key) => key in value);
}
