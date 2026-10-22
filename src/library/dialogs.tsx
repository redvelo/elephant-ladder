import { Button } from "epub-navigator/ui";
import { useAtomValue, useSetAtom } from "jotai";
import {
  annotationActivityAtom,
  dismissAnnotationActivityAtom,
  importAnnotationsAtom,
} from "../annotations/dispatch";
import { DictionariesDialog } from "../dictionaries/dialog";
import { ReflectionsDialog } from "../journal/reflections-dialog";
import { PublicationSiteDataDialog } from "../policy/site-data-dialog";
import { NoticesDialog } from "../settings/notices-dialog";
import { AppDialog } from "../ui/dialog";
import { bootstrapAtom, readerViewAtom } from "../app/session";
import { cancelDeletionAtom, deletePublicationAtom } from "./deletion";
import { closeLibrarySurfaceAtom, librarySurfaceAtom, returnFocusAtom } from "./shell-state";
import { deletionAtom, websiteDataResetAtom } from "./state";
import {
  cancelWebsiteDataResetAtom,
  confirmWebsiteDataResetAtom,
  resetWebsiteDataAtom,
} from "./website-data-reset";

/** Every decision and detail surface the library can raise, over whichever page is showing. */
export function LibraryDialogs({
  portalContainer,
}: {
  readonly portalContainer: HTMLElement | null;
}) {
  const bootstrap = useAtomValue(bootstrapAtom);
  const view = useAtomValue(readerViewAtom);
  const returnFocus = useAtomValue(returnFocusAtom);
  const surface = useAtomValue(librarySurfaceAtom);
  const closeSurface = useSetAtom(closeLibrarySurfaceAtom);
  const annotationActivity = useAtomValue(annotationActivityAtom);
  const deletion = useAtomValue(deletionAtom);
  const websiteDataReset = useAtomValue(websiteDataResetAtom);
  const dismissAnnotationActivity = useSetAtom(dismissAnnotationActivityAtom);
  const importAnnotations = useSetAtom(importAnnotationsAtom);
  const cancelDeletion = useSetAtom(cancelDeletionAtom);
  const deletePublication = useSetAtom(deletePublicationAtom);
  const cancelWebsiteDataReset = useSetAtom(cancelWebsiteDataResetAtom);
  const confirmWebsiteDataReset = useSetAtom(confirmWebsiteDataResetAtom);
  const resetWebsiteData = useSetAtom(resetWebsiteDataAtom);
  if (bootstrap?.mode !== "library") return null;
  return (
    <>
      {surface?.kind === "reflections" ? (
        <ReflectionsDialog
          key={surface.publication.publicationId}
          publication={surface.publication}
          container={portalContainer}
          returnFocus={surface.returnFocus}
          onClose={() => closeSurface()}
        />
      ) : null}
      {surface?.kind === "book-details" && view.status === "library" ? (
        <PublicationSiteDataDialog
          key={`${bootstrap.libraryDocumentId}:${surface.publication.publicationId}`}
          publication={surface.publication}
          portalContainer={portalContainer}
          returnFocus={surface.returnFocus}
          onClose={() => closeSurface()}
          onResetWebsiteData={() => {
            const { publication } = surface;
            closeSurface();
            confirmWebsiteDataReset(publication);
          }}
        />
      ) : null}
      {surface?.kind === "dictionaries" && view.status === "library" ? (
        <DictionariesDialog
          key={bootstrap.libraryDocumentId}
          portalContainer={portalContainer}
          returnFocus={surface.returnFocus}
          onClose={() => closeSurface()}
        />
      ) : null}
      {surface?.kind === "notices" && view.status === "library" ? (
        <NoticesDialog
          container={portalContainer}
          returnFocus={surface.returnFocus}
          onClose={() => closeSurface()}
        />
      ) : null}
      {annotationActivity.status === "confirm-mismatch" ? (
        <AppDialog
          kind="decision"
          title="Import from a different book?"
          description="This file was exported from a different book or edition. Annotations whose chapters are not in this book are skipped."
          container={portalContainer}
          returnFocus={returnFocus}
          onClose={() => dismissAnnotationActivity()}
          actions={
            <>
              <Button variant="outline" onClick={() => dismissAnnotationActivity()}>
                Cancel
              </Button>
              <Button
                onClick={() =>
                  void importAnnotations(
                    annotationActivity.publication,
                    annotationActivity.contents,
                    true,
                  )
                }
              >
                Import anyway
              </Button>
            </>
          }
        />
      ) : null}
      {deletion.status !== "idle" ? (
        <AppDialog
          kind="decision"
          title={`Delete ${deletion.publication.title}?`}
          description="The book, reading position, annotations, and cached pages will be permanently removed."
          container={portalContainer}
          returnFocus={returnFocus}
          onClose={() => {
            if (deletion.status === "confirming") cancelDeletion();
          }}
          actions={
            <>
              <Button
                variant="outline"
                disabled={deletion.status === "deleting"}
                onClick={() => cancelDeletion()}
              >
                Cancel
              </Button>
              <Button
                disabled={deletion.status === "deleting"}
                onClick={() => void deletePublication(deletion.publication)}
              >
                {deletion.status === "deleting" ? "Deleting…" : "Delete permanently"}
              </Button>
            </>
          }
        />
      ) : null}
      {websiteDataReset.status === "confirming" || websiteDataReset.status === "resetting" ? (
        <AppDialog
          kind="decision"
          title={`Reset browser data for ${websiteDataReset.publication.title}?`}
          description="The book will start with a new storage identity. Data it stored before becomes inaccessible and is queued for removal. The book, reading position, annotations, and preferences remain."
          container={portalContainer}
          returnFocus={returnFocus}
          onClose={() => {
            if (websiteDataReset.status === "confirming") cancelWebsiteDataReset();
          }}
          actions={
            <>
              <Button
                variant="outline"
                disabled={websiteDataReset.status === "resetting"}
                onClick={() => cancelWebsiteDataReset()}
              >
                Cancel
              </Button>
              <Button
                disabled={websiteDataReset.status === "resetting"}
                data-qualification-action="confirm-website-data-reset"
                onClick={() => void resetWebsiteData(websiteDataReset.publication)}
              >
                {websiteDataReset.status === "resetting" ? "Resetting…" : "Reset browser data"}
              </Button>
            </>
          }
        />
      ) : null}
    </>
  );
}
