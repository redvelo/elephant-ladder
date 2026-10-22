import { nativeRequest } from "../host/requests";
import { readExportedAnnotationSet, type ExportedAnnotationSet } from "./markdown";

export interface ImportReport {
  readonly added: number;
  readonly updated: number;
  readonly unchanged: number;
  readonly skipped: readonly { readonly index: number; readonly reason: string }[];
}

export type ImportOutcome =
  | { readonly status: "imported"; readonly report: ImportReport }
  | { readonly status: "publication-mismatch" }
  | { readonly status: "publication-open" };

function annotationsUrl(publicationId: string, action: string): string {
  return `/native/publications/${encodeURIComponent(publicationId)}/annotations/${action}`;
}

export async function readAnnotationSet(
  publicationId: string,
  sessionToken: string,
  signal?: AbortSignal,
): Promise<ExportedAnnotationSet> {
  const response = await nativeRequest(
    annotationsUrl(publicationId, "export"),
    { sessionToken, trusted: true },
    { signal },
  );
  if (!response.ok) throw new Error(`Annotation export failed with HTTP ${response.status}`);
  const set = readExportedAnnotationSet(await response.json());
  if (set === null) throw new Error("Annotation export returned invalid data.");
  return set;
}

export async function saveAnnotationSet(
  publicationId: string,
  sessionToken: string,
  libraryDocumentId: string,
  signal?: AbortSignal,
): Promise<string | null> {
  const response = await nativeRequest(
    annotationsUrl(publicationId, "save"),
    { sessionToken, libraryDocumentId },
    { method: "POST", signal },
  );
  if (response.status === 501) throw new Error("Saving files is not supported on this device.");
  if (!response.ok) throw new Error(`Annotation export failed with HTTP ${response.status}`);
  const value: unknown = await response.json();
  if (isRecord(value) && value.status === "cancelled") return null;
  if (isRecord(value) && value.status === "saved" && typeof value.location === "string") {
    return value.location;
  }
  throw new Error("Annotation export returned invalid data.");
}

export async function importAnnotationSet(
  publicationId: string,
  sessionToken: string,
  libraryDocumentId: string,
  contents: string,
  allowPublicationMismatch: boolean,
  signal?: AbortSignal,
): Promise<ImportOutcome> {
  const query = allowPublicationMismatch ? "?allowPublicationMismatch=true" : "";
  const response = await nativeRequest(
    `${annotationsUrl(publicationId, "import")}${query}`,
    { sessionToken, libraryDocumentId },
    { method: "POST", contentType: "application/json", body: contents, signal },
  );
  if (response.status === 422) {
    throw new Error("The file is not a valid annotation set, or it exceeds this book's limits.");
  }
  if (!response.ok && response.status !== 409) {
    throw new Error(`Annotation import failed with HTTP ${response.status}`);
  }
  const value: unknown = await response.json().catch(() => null);
  if (!isRecord(value)) throw new Error(`Annotation import failed with HTTP ${response.status}`);
  if (value.status === "publication-mismatch" || value.status === "publication-open") {
    return { status: value.status };
  }
  if (value.status === "imported" && isImportReport(value.report)) {
    return { status: "imported", report: value.report };
  }
  throw new Error("Annotation import returned invalid data.");
}

function isImportReport(value: unknown): value is ImportReport {
  return (
    isRecord(value) &&
    typeof value.added === "number" &&
    typeof value.updated === "number" &&
    typeof value.unchanged === "number" &&
    Array.isArray(value.skipped)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
