import { atom } from "jotai";
import { type LibraryBootstrap, type PublicationSummary } from "../host/bootstrap/types";
import { describeError, isAbortError, nativeRequest } from "../host/requests";
import { beginOperation, endOperation, operationAtom } from "../lib/operations";
import { bootstrapAtom } from "../app/session";
import {
  acquisitionAtom,
  deletionAtom,
  IDLE_DELETION,
  IDLE_IMPORT,
  libraryErrorAtom,
  publicationsAtom,
} from "./state";
import { librarySurfaceAtom } from "./shell-state";

export const deletionOperationAtom = operationAtom();

export const confirmDeletionAtom = atom(null, (get, set, publication: PublicationSummary): void => {
  if (get(librarySurfaceAtom) !== null) return;
  set(libraryErrorAtom, null);
  set(deletionAtom, { status: "confirming", publication });
});

export const cancelDeletionAtom = atom(null, (_get, set): void => {
  set(deletionAtom, IDLE_DELETION);
});

const completeDeletionAtom = atom(null, (get, set, publication: PublicationSummary): void => {
  set(
    publicationsAtom,
    get(publicationsAtom).filter(
      (candidate) => candidate.publicationId !== publication.publicationId,
    ),
  );
  const acquisition = get(acquisitionAtom);
  if (
    acquisition.status === "complete" &&
    acquisition.publication.publicationId === publication.publicationId
  ) {
    set(acquisitionAtom, IDLE_IMPORT);
  }
  set(deletionAtom, IDLE_DELETION);
});

export const deletePublicationAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || bootstrap.mode !== "library") return;
    const operation = beginOperation(get, set, deletionOperationAtom);
    const current = (): boolean => get(deletionOperationAtom).sequence === operation.sequence;
    set(deletionAtom, { status: "deleting", publication });
    set(libraryErrorAtom, null);
    try {
      await requestDeletion(bootstrap, publication, operation.controller.signal);
      if (!current()) return;
      set(completeDeletionAtom, publication);
    } catch (cause) {
      if (!current() || isAbortError(cause)) return;
      set(deletionAtom, { status: "confirming", publication });
      set(libraryErrorAtom, describeError(cause));
    } finally {
      endOperation(get, set, deletionOperationAtom, operation.controller);
    }
  },
);

async function requestDeletion(
  bootstrap: LibraryBootstrap,
  publication: PublicationSummary,
  signal: AbortSignal,
): Promise<void> {
  const response = await nativeRequest(
    `/native/publications/${encodeURIComponent(publication.publicationId)}`,
    { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
    { method: "DELETE", signal },
  );
  if (response.status !== 204 && response.status !== 404) {
    const detail = await response.text();
    throw new Error(
      `Delete failed with HTTP ${response.status}${detail.length > 0 ? `: ${detail}` : ""}`,
    );
  }
}
