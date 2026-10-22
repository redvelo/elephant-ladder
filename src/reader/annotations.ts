import type {
  NavigatorAnnotationAcknowledgment,
  NavigatorAnnotationMutationRequest,
  NavigatorAnnotationProjection,
  NavigatorAnnotationRecord,
} from "epub-navigator/react";
import { atom } from "jotai";
import { isAnnotationMutationAccepted, isAnnotationMutationConflict } from "../annotations/guards";
import { annotationFromRecord, annotationLink, annotationMarkdown } from "../annotations/markdown";
import { isAbortError, nativeRequest } from "../host/requests";
import { bootstrapAtom, readerViewAtom } from "../app/session";
import { readingSessionIdAtom } from "./state";

const replaceAnnotationsAtom = atom(
  null,
  (get, set, sessionId: string, annotations: NavigatorAnnotationProjection): void => {
    const view = get(readerViewAtom);
    if (view.status === "reading" && view.session.id === sessionId) {
      set(readerViewAtom, { ...view, annotations });
    }
  },
);

export const mutateAnnotationsAtom = atom(
  null,
  async (
    get,
    set,
    request: NavigatorAnnotationMutationRequest,
    context: { readonly signal: AbortSignal },
  ): Promise<NavigatorAnnotationAcknowledgment> => {
    const bootstrap = get(bootstrapAtom);
    const sessionId = get(readingSessionIdAtom);
    if (bootstrap === null || sessionId === null) {
      return { status: "rejected", requestId: request.requestId, reason: "reader-unavailable" };
    }
    try {
      const response = await nativeRequest(
        "/native/annotations/mutations",
        { sessionToken: bootstrap.sessionToken },
        {
          method: "POST",
          json: { readerSessionId: sessionId, mutation: request },
          signal: context.signal,
        },
      );
      const value: unknown = await response.json();
      if (get(readingSessionIdAtom) !== sessionId) {
        return { status: "rejected", requestId: request.requestId, reason: "reader-replaced" };
      }
      if (response.ok && isAnnotationMutationAccepted(value, request.requestId)) {
        set(replaceAnnotationsAtom, sessionId, value.projection);
        return {
          status: "accepted",
          requestId: value.requestId,
          recordId: value.recordId,
          projectionRevision: value.projection.revision,
        };
      }
      if (response.status === 409 && isAnnotationMutationConflict(value, request.requestId)) {
        if (value.authoritative !== null) {
          set(replaceAnnotationsAtom, sessionId, value.authoritative.projection);
        }
        return { status: "rejected", requestId: request.requestId, reason: "state-conflict" };
      }
      return { status: "rejected", requestId: request.requestId, reason: "save-failed" };
    } catch (cause) {
      if (isAbortError(cause) || context.signal.aborted) throw cause;
      return { status: "rejected", requestId: request.requestId, reason: "save-failed" };
    }
  },
);

export const copyAnnotationAtom = atom(
  null,
  (get, _set, record: NavigatorAnnotationRecord, format: "markdown" | "link"): void => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap?.mode !== "reading") return;
    const annotation = annotationFromRecord(record);
    void navigator.clipboard.writeText(
      format === "markdown"
        ? annotationMarkdown(bootstrap.annotationIdentifiers, annotation)
        : annotationLink(bootstrap.annotationIdentifiers, annotation.target),
    );
  },
);
