import { atom, type Getter } from "jotai";
import { type PublicationSummary } from "../host/bootstrap/types";
import { annotationSetMarkdown } from "./markdown";
import {
  importAnnotationSet,
  readAnnotationSet,
  saveAnnotationSet,
  type ImportReport,
} from "./transfer";
import { bootstrapAtom } from "../app/session";

export type AnnotationActivity =
  | { readonly status: "idle" }
  | { readonly status: "working"; readonly label: string }
  | { readonly status: "done"; readonly message: string }
  | { readonly status: "failed"; readonly message: string }
  | {
      readonly status: "confirm-mismatch";
      readonly publication: PublicationSummary;
      readonly contents: string;
    };

const IDLE: AnnotationActivity = { status: "idle" };

export const annotationActivityAtom = atom<AnnotationActivity>(IDLE);
export const annotationImportRequestAtom = atom<PublicationSummary | null>(null);

export const dismissAnnotationActivityAtom = atom(null, (_get, set): void => {
  set(annotationActivityAtom, IDLE);
});

function credentials(
  get: Getter,
): { readonly sessionToken: string; readonly libraryDocumentId: string } | null {
  const bootstrap = get(bootstrapAtom);
  if (bootstrap === null || bootstrap.mode !== "library") return null;
  return {
    sessionToken: bootstrap.sessionToken,
    libraryDocumentId: bootstrap.libraryDocumentId,
  };
}

export const copyAnnotationsAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<void> => {
    const host = credentials(get);
    if (host === null) return;
    set(annotationActivityAtom, { status: "working", label: "Copying annotations…" });
    try {
      const set_ = await readAnnotationSet(publication.publicationId, host.sessionToken);
      const quotes = set_.items.filter((item) => item.motivation !== "bookmarking").length;
      if (quotes === 0) {
        set(annotationActivityAtom, {
          status: "done",
          message: `${publication.title} has no highlights or notes.`,
        });
        return;
      }
      await navigator.clipboard.writeText(annotationSetMarkdown(set_));
      set(annotationActivityAtom, {
        status: "done",
        message: `Copied ${quotes} ${quotes === 1 ? "annotation" : "annotations"} as Markdown.`,
      });
    } catch (cause) {
      set(annotationActivityAtom, { status: "failed", message: describe(cause) });
    }
  },
);

export const exportAnnotationsAtom = atom(
  null,
  async (get, set, publication: PublicationSummary): Promise<void> => {
    const host = credentials(get);
    if (host === null) return;
    set(annotationActivityAtom, { status: "working", label: "Exporting annotations…" });
    try {
      const location = await saveAnnotationSet(
        publication.publicationId,
        host.sessionToken,
        host.libraryDocumentId,
      );
      set(annotationActivityAtom, {
        status: "done",
        message: location === null ? "Annotations exported." : `Annotations saved to ${location}.`,
      });
    } catch (cause) {
      set(annotationActivityAtom, { status: "failed", message: describe(cause) });
    }
  },
);

export const importAnnotationsAtom = atom(
  null,
  async (
    get,
    set,
    publication: PublicationSummary,
    contents: string,
    allowPublicationMismatch: boolean,
  ): Promise<void> => {
    const host = credentials(get);
    if (host === null) return;
    set(annotationActivityAtom, { status: "working", label: "Importing annotations…" });
    try {
      const outcome = await importAnnotationSet(
        publication.publicationId,
        host.sessionToken,
        host.libraryDocumentId,
        contents,
        allowPublicationMismatch,
      );
      if (outcome.status === "publication-mismatch") {
        set(annotationActivityAtom, { status: "confirm-mismatch", publication, contents });
        return;
      }
      if (outcome.status === "publication-open") {
        set(annotationActivityAtom, {
          status: "failed",
          message: "Close this book in the reader before importing annotations.",
        });
        return;
      }
      set(annotationActivityAtom, { status: "done", message: importSummary(outcome.report) });
    } catch (cause) {
      set(annotationActivityAtom, { status: "failed", message: describe(cause) });
    }
  },
);

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function importSummary(report: ImportReport): string {
  const parts = [
    `${report.added} added`,
    `${report.updated} updated`,
    `${report.unchanged} unchanged`,
  ];
  if (report.skipped.length > 0) parts.push(`${report.skipped.length} skipped`);
  return `Import complete: ${parts.join(", ")}.`;
}
