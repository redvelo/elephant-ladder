import { Button } from "epub-navigator/ui";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { annotationActivityAtom, dismissAnnotationActivityAtom } from "../annotations/dispatch";
import { isTerminalPhase } from "../dictionaries/model";
import { dictionaryOperationAtom } from "../dictionaries/state";
import { formatBytes } from "../lib/format";
import { AppNotice } from "../ui/notice";
import { destinationAtom } from "../app/destination";
import { cancelImportAtom, dismissImportAtom } from "./import";
import { dismissLayoutResetAtom } from "./layout-reset";
import {
  acquisitionAtom,
  layoutResetAtom,
  libraryErrorAtom,
  libraryOpeningAtom,
  websiteDataResetAtom,
  type ImportState,
} from "./state";
import { dismissWebsiteDataResetAtom } from "./website-data-reset";

/** What the library is doing or has just done, reported where it does not take the page. */
export function LibraryNotices({ pace }: { readonly pace: "standard" | "minimal" }) {
  const destination = useAtomValue(destinationAtom);
  const acquisition = useAtomValue(acquisitionAtom);
  const annotationActivity = useAtomValue(annotationActivityAtom);
  const dictionaryOperation = useAtomValue(dictionaryOperationAtom);
  const [libraryError, setLibraryError] = useAtom(libraryErrorAtom);
  const websiteDataReset = useAtomValue(websiteDataResetAtom);
  const layoutReset = useAtomValue(layoutResetAtom);
  const libraryOpening = useAtomValue(libraryOpeningAtom);
  const cancelImport = useSetAtom(cancelImportAtom);
  const dismissImport = useSetAtom(dismissImportAtom);
  const dismissAnnotationActivity = useSetAtom(dismissAnnotationActivityAtom);
  const dismissWebsiteDataReset = useSetAtom(dismissWebsiteDataResetAtom);
  const dismissLayoutReset = useSetAtom(dismissLayoutResetAtom);
  return (
    <div className="app-feedback">
      <ImportStatus
        state={acquisition}
        pace={pace}
        onCancel={() => cancelImport()}
        onDismiss={() => dismissImport()}
      />
      {annotationActivity.status === "working" ? (
        <AppNotice tone="progress" pace={pace}>
          {annotationActivity.label}
        </AppNotice>
      ) : annotationActivity.status === "done" ? (
        <AppNotice tone="done" pace={pace} onDismiss={() => dismissAnnotationActivity()}>
          {annotationActivity.message}
        </AppNotice>
      ) : annotationActivity.status === "failed" ? (
        <AppNotice tone="failed" pace={pace} onDismiss={() => dismissAnnotationActivity()}>
          {annotationActivity.message}
        </AppNotice>
      ) : null}
      {dictionaryOperation !== null && !isTerminalPhase(dictionaryOperation.phase) ? (
        <AppNotice tone="progress" pace={pace}>
          {dictionaryOperation.phase === "downloading" && dictionaryOperation.bytesTotal > 0
            ? `Installing a dictionary · ${Math.floor((dictionaryOperation.bytesCompleted / dictionaryOperation.bytesTotal) * 100)}% · ${formatBytes(dictionaryOperation.bytesCompleted)} of ${formatBytes(dictionaryOperation.bytesTotal)}`
            : "Installing a dictionary…"}
        </AppNotice>
      ) : null}
      {libraryError !== null ? (
        <AppNotice tone="failed" pace={pace} onDismiss={() => setLibraryError(null)}>
          {libraryError}
        </AppNotice>
      ) : null}
      {websiteDataReset.status === "complete" ? (
        <AppNotice
          tone="done"
          pace={pace}
          onDismiss={() => dismissWebsiteDataReset()}
          data-website-data-reset-status="reset"
          data-physical-purge={websiteDataReset.result.physicalPurge}
        >
          Browser data reset for {websiteDataReset.publication.title}. Data the book stored before
          is no longer used. Physical cleanup was{" "}
          {websiteDataReset.result.physicalPurge === "partial"
            ? "attempted where supported by Android"
            : "unavailable on this platform"}
          .
        </AppNotice>
      ) : null}
      {layoutReset.status === "working" ? (
        <AppNotice tone="progress" pace={pace}>
          Returning {layoutReset.publication.title} to the default layout…
        </AppNotice>
      ) : layoutReset.status === "done" ? (
        <AppNotice tone="done" pace={pace} onDismiss={() => dismissLayoutReset()}>
          {layoutReset.publication.title} follows the default layout again.
        </AppNotice>
      ) : layoutReset.status === "failed" ? (
        <AppNotice tone="failed" pace={pace} onDismiss={() => dismissLayoutReset()}>
          {layoutReset.message}
        </AppNotice>
      ) : null}
      {libraryOpening?.status === "preparing" && destination !== "home" ? (
        <AppNotice tone="progress" pace={pace}>
          Preparing publication…
        </AppNotice>
      ) : null}
    </div>
  );
}

function ImportStatus({
  state,
  pace,
  onCancel,
  onDismiss,
}: {
  state: ImportState;
  pace: "standard" | "minimal";
  onCancel: () => void;
  onDismiss: () => void;
}) {
  if (state.status === "idle") return null;
  if (state.status === "streaming")
    return (
      <AppNotice
        tone="progress"
        pace={pace}
        action={
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
        }
      >
        Importing {state.fileName} ({formatBytes(state.size)})…
        {state.total > 1 ? ` · ${state.position} of ${state.total}` : ""}
      </AppNotice>
    );
  if (state.status === "batch")
    return (
      <AppNotice tone={state.failed > 0 ? "failed" : "done"} pace={pace} onDismiss={onDismiss}>
        {batchSummary(state)}
      </AppNotice>
    );
  if (state.status === "failed")
    return (
      <AppNotice tone="failed" pace={pace} onDismiss={onDismiss}>
        {state.fileName}: {state.message}
      </AppNotice>
    );
  if (state.status === "cancelled")
    return (
      <AppNotice tone="done" pace={pace} onDismiss={onDismiss}>
        Import cancelled: {state.fileName}
      </AppNotice>
    );
  return (
    <AppNotice tone="done" pace={pace} onDismiss={onDismiss}>
      {state.disposition === "created" ? "Imported" : "Already in library"}:{" "}
      {state.publication.title}
    </AppNotice>
  );
}

/** What an import of several books did, counted rather than listed. */
function batchSummary(state: Extract<ImportState, { status: "batch" }>): string {
  const books = (count: number) => `${count} ${count === 1 ? "book" : "books"}`;
  const parts = [
    state.created > 0 ? `Imported ${books(state.created)}` : null,
    state.existing > 0 ? `${books(state.existing)} already in your library` : null,
    state.failed > 0 ? `${books(state.failed)} could not be read` : null,
  ].filter((part) => part !== null);
  const summary = parts.length === 0 ? "No books were imported" : parts.join(" · ");
  return state.cancelled ? `${summary} · cancelled` : summary;
}
