import { atom } from "jotai";
import { isPublicationSummary } from "../host/bootstrap/publication";
import { isRecord } from "../host/guards";
import { type PublicationSummary } from "../host/bootstrap/types";
import type { OpenedPublicationArrival } from "../host/opened-publications";
import { describeError, isAbortError, nativeRequest } from "../host/requests";
import { beginOperation, endOperation, operationAtom } from "../lib/operations";
import { bootstrapAtom } from "../app/session";
import { openPublicationAtom } from "./open";
import {
  acquisitionAtom,
  IDLE_IMPORT,
  libraryBusyAtom,
  publicationsAtom,
  type ImportState,
} from "./state";
import { librarySurfaceAtom } from "./shell-state";

export const acquisitionOperationAtom = operationAtom();

interface ImportPublicationResponse {
  disposition: "created" | "existing";
  publication: PublicationSummary;
}

const maximumAcquisitionBytes = 4 * 1024 * 1024 * 1024;

function isImportPublicationResponse(value: unknown): value is ImportPublicationResponse {
  return (
    isRecord(value) &&
    (value.disposition === "created" || value.disposition === "existing") &&
    isPublicationSummary(value.publication)
  );
}

const completeImportAtom = atom(
  null,
  (get, set, complete: Extract<ImportState, { status: "complete" }>): void => {
    set(publicationsAtom, [
      complete.publication,
      ...get(publicationsAtom).filter(
        (publication) => publication.publicationId !== complete.publication.publicationId,
      ),
    ]);
    set(acquisitionAtom, complete);
  },
);

export const importPublicationAtom = atom(null, async (_get, set, file: File): Promise<void> => {
  await set(importPublicationsAtom, [file]);
});

/**
 * Imports books one after another under a single operation: the host accepts one import at a
 * time, and one cancellation stops the rest. A book that cannot be read does not stop the
 * ones behind it; what happened to the whole set is reported once, at the end.
 */

export const importPublicationsAtom = atom(
  null,
  async (get, set, files: readonly File[]): Promise<void> => {
    if (get(librarySurfaceAtom) !== null || files.length === 0) return;
    const single = files.length === 1;
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || bootstrap.mode !== "library") {
      set(acquisitionAtom, {
        status: "failed",
        fileName: files[0]?.name ?? "",
        message: "Host bootstrap is unavailable.",
      });
      return;
    }
    const operation = beginOperation(get, set, acquisitionOperationAtom);
    const current = (): boolean => get(acquisitionOperationAtom).sequence === operation.sequence;
    let created = 0;
    let existing = 0;
    let failed = 0;
    try {
      for (const [index, file] of files.entries()) {
        if (!current()) return;
        if (file.size === 0 || file.size > maximumAcquisitionBytes) {
          failed += 1;
          if (single) {
            set(acquisitionAtom, {
              status: "failed",
              fileName: file.name,
              message: "EPUB files must contain bytes and may not exceed 4 GiB.",
            });
            return;
          }
          continue;
        }
        set(acquisitionAtom, {
          status: "streaming",
          fileName: file.name,
          size: file.size,
          position: index + 1,
          total: files.length,
        });
        const started = performance.now();
        try {
          const imported = await requestImport(
            bootstrap.sessionToken,
            file,
            operation.controller.signal,
          );
          if (!current()) return;
          if (imported.disposition === "created") created += 1;
          else existing += 1;
          set(completeImportAtom, {
            status: "complete",
            fileName: file.name,
            browserMilliseconds: Math.round(performance.now() - started),
            disposition: imported.disposition,
            publication: imported.publication,
          });
        } catch (cause) {
          if (!current()) return;
          if (isAbortError(cause)) {
            if (single) set(acquisitionAtom, { status: "cancelled", fileName: file.name });
            else
              set(acquisitionAtom, { status: "batch", created, existing, failed, cancelled: true });
            return;
          }
          failed += 1;
          if (single) {
            set(acquisitionAtom, {
              status: "failed",
              fileName: file.name,
              message: describeError(cause),
            });
            return;
          }
        }
      }
      if (!single && current()) {
        set(acquisitionAtom, { status: "batch", created, existing, failed, cancelled: false });
      }
    } finally {
      endOperation(get, set, acquisitionOperationAtom, operation.controller);
    }
  },
);

const samplePublication = {
  path: "/sample/alices-adventures-in-wonderland.epub",
  fileName: "alices-adventures-in-wonderland.epub",
} as const;

export const importSamplePublicationAtom = atom(null, async (get, set): Promise<void> => {
  if (get(libraryBusyAtom)) return;
  let file: File;
  try {
    const response = await fetch(samplePublication.path, { cache: "no-store" });
    if (!response.ok) {
      throw new Error(`The bundled sample answered HTTP ${response.status}.`);
    }
    file = new File([await response.blob()], samplePublication.fileName, {
      type: "application/epub+zip",
    });
  } catch (cause) {
    set(acquisitionAtom, {
      status: "failed",
      fileName: samplePublication.fileName,
      message: describeError(cause),
    });
    return;
  }
  await set(importPublicationAtom, file);
});

export const cancelImportAtom = atom(null, (get): void => {
  get(acquisitionOperationAtom).controller?.abort();
});

/// A publication another application handed over: read once from the host, then imported the

export const importOpenedPublicationAtom = atom(
  null,
  async (get, set, arrival: OpenedPublicationArrival): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap?.mode !== "library") return;
    // Claim the acquisition before reading the bytes. The read is a round trip of its own, and
    // until something says an import is under way a second handed book starts one beside it.
    set(acquisitionAtom, {
      status: "streaming",
      fileName: arrival.fileName,
      size: arrival.sizeBytes,
      position: 1,
      total: 1,
    });
    let file: File;
    try {
      const response = await nativeRequest(
        `/native/opened-publications/${encodeURIComponent(arrival.openId)}/bytes`,
        { sessionToken: bootstrap.sessionToken, trusted: true },
      );
      if (!response.ok) {
        throw new Error(`The handed book could not be read (HTTP ${response.status}).`);
      }
      file = new File([await response.blob()], arrival.fileName, {
        type: "application/epub+zip",
      });
    } catch (cause) {
      set(acquisitionAtom, {
        status: "failed",
        fileName: arrival.fileName,
        message: describeError(cause),
      });
      return;
    }
    await set(importPublicationAtom, file);
    // A book handed over to be read opens, new or already here. A dropped one is only kept:
    // a drop may carry a shelf, and nobody asked to read all of it.
    const acquisition = get(acquisitionAtom);
    if (arrival.intent === "read" && acquisition.status === "complete") {
      void set(openPublicationAtom, acquisition.publication);
    }
  },
);

export const dismissImportAtom = atom(null, (get, set): void => {
  if (get(acquisitionAtom).status !== "streaming") set(acquisitionAtom, IDLE_IMPORT);
});

const importConflictRetryMs = 400;
const importConflictAttempts = 25;

async function requestImport(
  sessionToken: string,
  file: File,
  signal: AbortSignal,
): Promise<ImportPublicationResponse> {
  for (let attempt = 0; ; attempt += 1) {
    const response = await nativeRequest(
      "/native/publications/import",
      { sessionToken },
      { method: "POST", contentType: "application/epub+zip", body: file, signal },
    );
    if (response.ok) {
      const imported: unknown = await response.json();
      if (!isImportPublicationResponse(imported)) {
        throw new Error("Import returned invalid publication data.");
      }
      return imported;
    }
    const message = await response.text();
    // A conflict is the host saying "not yet": it is still closing a book, or another import
    // holds the slot. Both pass, and a book handed over should wait rather than be refused.
    if (response.status === 409 && attempt < importConflictAttempts) {
      await new Promise((resolve) => setTimeout(resolve, importConflictRetryMs));
      if (signal.aborted) throw new DOMException("Import cancelled.", "AbortError");
      continue;
    }
    throw new Error(
      `Import failed with HTTP ${response.status}${message.length > 0 ? `: ${message}` : ""}`,
    );
  }
}
