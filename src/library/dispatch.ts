import { atom } from "jotai";
import { isPublicationSummary } from "../host/bootstrap/publication";
import { type PublicationSummary } from "../host/bootstrap/types";
import { describeError, isAbortError, nativeRequest } from "../host/requests";
import { supersedeOperation } from "../lib/operations";
import { bootstrapAtom, readerViewAtom } from "../app/session";
import { deletionOperationAtom } from "./deletion";
import { acquisitionOperationAtom } from "./import";
import { openOperationAtom } from "./open";
import { dictionaryWatchAtom } from "../dictionaries/state";
import { librarySurfaceAtom } from "./shell-state";
import { libraryBusyAtom, libraryErrorAtom, libraryLoadedAtom, publicationsAtom } from "./state";
import { websiteDataResetOperationAtom } from "./website-data-reset";

const applyLibraryAtom = atom(null, (get, set, loaded: PublicationSummary[]): void => {
  set(publicationsAtom, [
    ...loaded,
    ...get(publicationsAtom).filter(
      (publication) =>
        !loaded.some((candidate) => candidate.publicationId === publication.publicationId),
    ),
  ]);
  set(libraryLoadedAtom, true);
  set(libraryErrorAtom, null);
});

export const loadLibraryAtom = atom(null, (get, set): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap?.mode === "library") {
    void (async () => {
      try {
        set(applyLibraryAtom, await fetchLibrary(bootstrap.sessionToken, controller.signal));
      } catch (cause) {
        if (!isAbortError(cause)) set(libraryErrorAtom, describeError(cause));
      }
    })();
  }
  return () => controller.abort();
});

export const showBookDetailsAtom = atom(
  null,
  (get, set, publication: PublicationSummary, returnFocus: HTMLElement | null): void => {
    if (
      get(bootstrapAtom)?.mode !== "library" ||
      get(readerViewAtom).status !== "library" ||
      get(libraryBusyAtom)
    ) {
      return;
    }
    set(librarySurfaceAtom, { kind: "book-details", publication, returnFocus });
  },
);

export const showReflectionsAtom = atom(
  null,
  (_get, set, publication: PublicationSummary, returnFocus: HTMLElement): void => {
    set(librarySurfaceAtom, { kind: "reflections", publication, returnFocus });
  },
);

export const showDictionariesAtom = atom(
  null,
  (get, set, returnFocus: HTMLElement | null): void => {
    set(librarySurfaceAtom, { kind: "dictionaries", returnFocus });
    set(dictionaryWatchAtom, get(dictionaryWatchAtom) + 1);
  },
);

export const showNoticesAtom = atom(null, (_get, set, returnFocus: HTMLElement | null): void => {
  set(librarySurfaceAtom, { kind: "notices", returnFocus });
});

export const showShortcutsAtom = atom(null, (_get, set, returnFocus: HTMLElement | null): void => {
  set(librarySurfaceAtom, { kind: "shortcuts", returnFocus });
});

export const abortLibraryOperationsAtom = atom(null, (get, set): void => {
  supersedeOperation(get, set, acquisitionOperationAtom);
  supersedeOperation(get, set, openOperationAtom);
  supersedeOperation(get, set, deletionOperationAtom);
  supersedeOperation(get, set, websiteDataResetOperationAtom);
});

async function fetchLibrary(
  sessionToken: string,
  signal: AbortSignal,
): Promise<PublicationSummary[]> {
  const response = await nativeRequest(
    "/native/publications",
    { sessionToken, trusted: true },
    { signal },
  );
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(
      `Library request failed with HTTP ${response.status}${detail ? `: ${detail}` : ""}`,
    );
  }
  const value: unknown = await response.json();
  if (!Array.isArray(value) || !value.every(isPublicationSummary)) {
    throw new Error("Library returned invalid publication data.");
  }
  return value;
}
