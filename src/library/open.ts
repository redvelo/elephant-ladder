import { atom } from "jotai";
import { hasExactKeys, isRecord } from "../host/guards";
import { isPreferenceRevisions } from "../host/bootstrap/preferences";
import { type LibraryBootstrap, type PublicationSummary } from "../host/bootstrap/types";
import {
  createPerformanceTraceId,
  describeError,
  isAbortError,
  isNavigationResponse,
  nativeRequest,
} from "../host/requests";
import { beginOperation, endOperation, operationAtom, supersedeOperation } from "../lib/operations";
import {
  collectPublicationPolicyChoices,
  savePublicationPolicyWithRetry,
  type PolicyDecisionResponse,
} from "../policy/publication-policy";
import { requestPolicyDecisionAtom } from "../policy/decision-state";
import { bootstrapAtom, LIBRARY_VIEW, readerViewAtom } from "../app/session";
import { libraryErrorAtom } from "./state";
import { librarySurfaceAtom } from "./shell-state";

export const openOperationAtom = operationAtom();

interface OpenPublicationResponse {
  status: "navigate";
  navigationPath: string;
}

interface OpenPreparationResponse {
  status: "preparing";
}

function isOpenPublicationResponse(value: unknown): value is OpenPublicationResponse {
  return isNavigationResponse(value);
}

function isPolicyDecisionResponse(value: unknown): value is PolicyDecisionResponse {
  if (
    !isRecord(value) ||
    !hasExactKeys(
      value,
      new Set(["status", "required", "policy", "effective", "relevance", "revisions"]),
    ) ||
    value.status !== "policy-decision-required" ||
    !isRecord(value.required) ||
    !hasExactKeys(value.required, new Set(["activeContent", "remoteResources"])) ||
    typeof value.required.activeContent !== "boolean" ||
    typeof value.required.remoteResources !== "boolean" ||
    (!value.required.activeContent && !value.required.remoteResources) ||
    !isRecord(value.policy) ||
    !hasExactKeys(value.policy, new Set(["global", "publication"])) ||
    !isRecord(value.policy.global) ||
    !hasExactKeys(value.policy.global, new Set(["activeContent", "remoteResources"])) ||
    !["automatic", "ask", "disabled"].includes(value.policy.global.activeContent as string) ||
    !["allowed", "ask", "blocked"].includes(value.policy.global.remoteResources as string) ||
    !isRecord(value.policy.publication) ||
    !hasExactKeys(value.policy.publication, new Set(["activeContent", "remoteResources"])) ||
    ![null, "enabled", "disabled"].includes(
      value.policy.publication.activeContent as null | string,
    ) ||
    ![null, "allowed", "blocked"].includes(
      value.policy.publication.remoteResources as null | string,
    ) ||
    !isRecord(value.effective) ||
    !hasExactKeys(value.effective, new Set(["activeContent", "remoteResources"])) ||
    !["enabled", "ask", "disabled"].includes(value.effective.activeContent as string) ||
    !["allowed", "ask", "blocked"].includes(value.effective.remoteResources as string) ||
    !isRecord(value.relevance) ||
    !hasExactKeys(value.relevance, new Set(["activeContent", "remoteResources"])) ||
    !["detected", "not-detected", "unknown"].includes(value.relevance.activeContent as string) ||
    !["detected", "not-detected", "unknown"].includes(value.relevance.remoteResources as string) ||
    !isPreferenceRevisions(value.revisions)
  ) {
    return false;
  }
  return true;
}

function isOpenPreparationResponse(value: unknown): value is OpenPreparationResponse {
  return isRecord(value) && value.status === "preparing";
}

export const openPublicationAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<void> => {
    if (get(librarySurfaceAtom) !== null) return;
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || bootstrap.mode !== "library") return;
    const operation = beginOperation(get, set, openOperationAtom);
    const { controller } = operation;
    const current = (): boolean => get(openOperationAtom).sequence === operation.sequence;
    const traceId = createPerformanceTraceId();
    set(readerViewAtom, { status: "opening", publication });
    set(libraryErrorAtom, null);
    try {
      let policyHandled = false;
      while (true) {
        const opened = await requestOpen(bootstrap, publication, operation.sequence, traceId, {
          signal: controller.signal,
          current,
          onPreparing: () => set(readerViewAtom, { status: "preparing", publication }),
        });
        if (opened === null) return;
        if (opened.status === "navigate") {
          window.location.replace(opened.navigationPath);
          return;
        }
        if (policyHandled) throw new Error("Publication policy decision remained unresolved.");
        const changes = await collectPublicationPolicyChoices(
          opened.decision,
          publication.title,
          (prompt, signal) => set(requestPolicyDecisionAtom, prompt, signal),
          controller.signal,
        );
        if (!current() || controller.signal.aborted) return;
        if (changes === null) {
          set(readerViewAtom, LIBRARY_VIEW);
          return;
        }
        await savePublicationPolicyWithRetry({
          url: `/native/publications/${encodeURIComponent(publication.publicationId)}/policy`,
          sessionToken: bootstrap.sessionToken,
          libraryDocumentId: bootstrap.libraryDocumentId,
          initial: opened.decision,
          changes,
          signal: controller.signal,
        });
        if (!current()) return;
        policyHandled = true;
      }
    } catch (cause) {
      if (!current() || isAbortError(cause)) return;
      set(readerViewAtom, {
        status: "error",
        operation: "open",
        publication,
        session: null,
        message: describeError(cause),
      });
    } finally {
      endOperation(get, set, openOperationAtom, controller);
    }
  },
);

export const returnToLibraryAtom = atom(null, (get, set): void => {
  supersedeOperation(get, set, openOperationAtom);
  set(readerViewAtom, LIBRARY_VIEW);
});

type OpenOutcome =
  | OpenPublicationResponse
  | { readonly status: "policy-decision"; readonly decision: PolicyDecisionResponse };

async function requestOpen(
  bootstrap: LibraryBootstrap,
  publication: PublicationSummary,
  operation: number,
  traceId: string,
  {
    signal,
    current,
    onPreparing,
  }: {
    readonly signal: AbortSignal;
    readonly current: () => boolean;
    readonly onPreparing: () => void;
  },
): Promise<OpenOutcome | null> {
  while (true) {
    const response = await nativeRequest(
      `/native/publications/${encodeURIComponent(publication.publicationId)}/open`,
      { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
      {
        method: "POST",
        headers: { "X-Elephant-Trace-Id": traceId, "X-Elephant-Open-Operation": String(operation) },
        signal,
      },
    );
    if (!current()) return null;
    const responseBody = await response.text();
    let opened: unknown;
    try {
      opened = JSON.parse(responseBody) as unknown;
    } catch {
      opened = undefined;
    }
    if (opened === undefined) {
      if (!response.ok) {
        throw new Error(
          responseBody.length === 0 ? `Open failed with HTTP ${response.status}` : responseBody,
        );
      }
      throw new Error("Open returned invalid JSON.");
    }
    if (response.status === 202) {
      if (!isOpenPreparationResponse(opened)) {
        throw new Error("Open returned invalid preparation data.");
      }
      // Preparation runs as long as the host reports progress. Transcoding a long audiobook
      // legitimately takes many minutes, and a wall clock here only decides that the user may
      // not have their book; an unreachable or failing host still ends the loop by throwing.
      onPreparing();
      continue;
    }
    if (response.status === 409 && isPolicyDecisionResponse(opened)) {
      return { status: "policy-decision", decision: opened };
    }
    if (!response.ok) {
      const status = isRecord(opened) && typeof opened.status === "string" ? opened.status : null;
      throw new Error(status === null ? `Open failed with HTTP ${response.status}` : status);
    }
    if (!isOpenPublicationResponse(opened)) {
      throw new Error("Open returned invalid publication data.");
    }
    return opened;
  }
}
