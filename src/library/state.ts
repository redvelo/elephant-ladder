import { atom } from "jotai";
import { type PublicationSummary } from "../host/bootstrap/types";
import type { WebsiteDataResetResponse } from "../host/website-data";
import { readerViewAtom } from "../app/session";
import { librarySurfaceAtom } from "./shell-state";
import { dateTimestamp } from "../lib/format";

export type ImportState =
  | { status: "idle" }
  | { status: "streaming"; fileName: string; size: number; position: number; total: number }
  | {
      status: "complete";
      fileName: string;
      browserMilliseconds: number;
      disposition: "created" | "existing";
      publication: PublicationSummary;
    }
  | { status: "cancelled"; fileName: string }
  | { status: "failed"; fileName: string; message: string }
  /** What became of an import of several books, which reports once rather than per book. */
  | {
      status: "batch";
      created: number;
      existing: number;
      failed: number;
      cancelled: boolean;
    };

export type DeletionState =
  | { status: "idle" }
  | { status: "confirming"; publication: PublicationSummary }
  | { status: "deleting"; publication: PublicationSummary };

export type WebsiteDataResetState =
  | { status: "idle" }
  | { status: "confirming"; publication: PublicationSummary }
  | { status: "resetting"; publication: PublicationSummary }
  | {
      status: "complete";
      publication: PublicationSummary;
      result: WebsiteDataResetResponse;
    };

export type LayoutResetState =
  | { readonly status: "idle" }
  | { readonly status: "working"; readonly publication: PublicationSummary }
  | { readonly status: "done"; readonly publication: PublicationSummary }
  | { readonly status: "failed"; readonly message: string };

export const IDLE_IMPORT: ImportState = { status: "idle" };
export const IDLE_DELETION: DeletionState = { status: "idle" };
export const IDLE_RESET: WebsiteDataResetState = { status: "idle" };
const NO_PUBLICATIONS: PublicationSummary[] = [];

export const publicationsAtom = atom<PublicationSummary[]>(NO_PUBLICATIONS);
export const libraryLoadedAtom = atom(false);
export const libraryErrorAtom = atom<string | null>(null);
export const acquisitionAtom = atom<ImportState>(IDLE_IMPORT);
export const deletionAtom = atom<DeletionState>(IDLE_DELETION);
export const websiteDataResetAtom = atom<WebsiteDataResetState>(IDLE_RESET);
export const layoutResetAtom = atom<LayoutResetState>({ status: "idle" });

export const libraryOpeningAtom = atom((get) => {
  const view = get(readerViewAtom);
  return view.status === "opening" || view.status === "preparing" ? view : null;
});

export const libraryBusyAtom = atom((get) => {
  const reset = get(websiteDataResetAtom).status;
  return (
    get(librarySurfaceAtom) !== null ||
    get(libraryOpeningAtom) !== null ||
    get(deletionAtom).status !== "idle" ||
    reset === "confirming" ||
    reset === "resetting" ||
    get(acquisitionAtom).status === "streaming"
  );
});

export const featuredPublicationAtom = atom((get) =>
  selectFeaturedPublication(get(publicationsAtom)),
);

function selectFeaturedPublication(
  publications: readonly PublicationSummary[],
): PublicationSummary | null {
  if (publications.length === 0) return null;
  return [...publications].sort((left, right) => {
    const leftRead = dateTimestamp(left.lastReadAt);
    const rightRead = dateTimestamp(right.lastReadAt);
    if (leftRead !== rightRead) return rightRead - leftRead;
    return dateTimestamp(right.importedAt) - dateTimestamp(left.importedAt);
  })[0]!;
}
