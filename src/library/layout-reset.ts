import { atom } from "jotai";
import { type PublicationSummary } from "../host/bootstrap/types";
import { describeError, nativeRequest } from "../host/requests";
import { bootstrapAtom } from "../app/session";
import { layoutResetAtom } from "./state";

export const dismissLayoutResetAtom = atom(null, (_get, set): void => {
  set(layoutResetAtom, { status: "idle" });
});

/**
 * Drops a book's own layout so it follows the application defaults again. A book that once
 * differed from the defaults otherwise keeps its layout for good.
 */

export const resetPublicationLayoutAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap?.mode !== "library" || get(layoutResetAtom).status === "working") return;
    set(layoutResetAtom, { status: "working", publication });
    try {
      const response = await nativeRequest(
        `/native/publications/${encodeURIComponent(publication.publicationId)}/preferences`,
        { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
        { method: "DELETE" },
      );
      if (!response.ok) throw new Error(`Layout reset failed with HTTP ${response.status}`);
      set(layoutResetAtom, { status: "done", publication });
    } catch (cause) {
      set(layoutResetAtom, { status: "failed", message: describeError(cause) });
    }
  },
);
