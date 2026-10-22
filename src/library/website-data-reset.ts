import { atom } from "jotai";
import { type LibraryBootstrap, type PublicationSummary } from "../host/bootstrap/types";
import { describeError, isAbortError, nativeRequest } from "../host/requests";
import { isWebsiteDataResetResponse, type WebsiteDataResetResponse } from "../host/website-data";
import { beginOperation, endOperation, operationAtom } from "../lib/operations";
import { bootstrapAtom } from "../app/session";
import { librarySurfaceAtom } from "./shell-state";
import { IDLE_RESET, libraryErrorAtom, websiteDataResetAtom } from "./state";

export const websiteDataResetOperationAtom = operationAtom();

export const confirmWebsiteDataResetAtom = atom(
  null,
  (get, set, publication: PublicationSummary): void => {
    if (get(librarySurfaceAtom) !== null) return;
    set(libraryErrorAtom, null);
    set(websiteDataResetAtom, { status: "confirming", publication });
  },
);

export const cancelWebsiteDataResetAtom = atom(null, (_get, set): void => {
  set(websiteDataResetAtom, IDLE_RESET);
});

export const dismissWebsiteDataResetAtom = atom(null, (get, set): void => {
  if (get(websiteDataResetAtom).status === "complete") set(websiteDataResetAtom, IDLE_RESET);
});

export const resetWebsiteDataAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<boolean> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || bootstrap.mode !== "library") return false;
    const operation = beginOperation(get, set, websiteDataResetOperationAtom);
    const current = (): boolean =>
      get(websiteDataResetOperationAtom).sequence === operation.sequence;
    set(websiteDataResetAtom, { status: "resetting", publication });
    set(libraryErrorAtom, null);
    try {
      const result = await requestWebsiteDataReset(
        bootstrap,
        publication,
        operation.controller.signal,
      );
      if (!current()) return false;
      set(websiteDataResetAtom, { status: "complete", publication, result });
      return true;
    } catch (cause) {
      if (!current() || isAbortError(cause)) return false;
      set(websiteDataResetAtom, { status: "confirming", publication });
      set(libraryErrorAtom, describeError(cause));
      return false;
    } finally {
      endOperation(get, set, websiteDataResetOperationAtom, operation.controller);
    }
  },
);

async function requestWebsiteDataReset(
  bootstrap: LibraryBootstrap,
  publication: PublicationSummary,
  signal: AbortSignal,
): Promise<WebsiteDataResetResponse> {
  const response = await nativeRequest(
    `/native/publications/${encodeURIComponent(publication.publicationId)}/website-data/reset`,
    { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
    { method: "POST", signal },
  );
  const body = await response.text();
  let value: unknown;
  try {
    value = JSON.parse(body) as unknown;
  } catch {
    if (!response.ok) {
      throw new Error(
        `Website-data reset failed with HTTP ${response.status}${body.length > 0 ? `: ${body}` : ""}`,
      );
    }
    throw new Error("Website-data reset returned invalid JSON.");
  }
  if (!response.ok) {
    throw new Error(`Website-data reset failed with HTTP ${response.status}`);
  }
  if (!isWebsiteDataResetResponse(value)) {
    throw new Error("Website-data reset returned invalid data.");
  }
  return value;
}
