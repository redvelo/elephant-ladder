import type { MountedPublication } from "epub-navigator";
import type { NavigatorReaderPreferences } from "epub-navigator/react";
import {
  type LibraryBootstrap,
  type PublicationSummary,
  type ReadingBootstrap,
} from "../../src/host/bootstrap/types";

const hex64 = "a".repeat(64);

export const readerPreferences: NavigatorReaderPreferences = {
  colorScheme: "system",
  presentationMode: "paginated",
  paginationColumns: "auto",
  publicationTheme: "auto",
  publicationTypeface: "original",
  fontScale: "100",
  lineHeight: "original",
  letterSpacing: "0",
  wordSpacing: "0",
  paragraphSpacing: "original",
  textIndent: "original",
  measure: "wide",
  alignment: "original",
  hyphenation: "original",
  keyboardMode: "standard",
  touchZones: true,
  narrationRate: "1",
  narrationSkippedTypes: ["pagebreak"],
};

const base = {
  readerSessionId: hex64,
  launchId: "b".repeat(32),
  readerOrigin: "http://reader.test",
  assetOrigin: "http://reader.test",
  sessionToken: "c".repeat(64),
  tauriIpcEnabled: false,
  nativeActions: { frontLightDialog: false, sideButtons: false, windowFullscreen: false },
  display: { technology: "unknown" },
  diagnosticsLevel: "summary",
  recoveryNavigationPath: `/window/${hex64}/start`,
  preferences: readerPreferences,
  preferenceRevisions: { global: 0, publication: 0 },
  readerTypefaces: ["reader-serif", "reader-sans"],
} as const;

export const appSettings = {
  revision: 1,
  preferences: readerPreferences,
  device: { sideButtonsTurnPages: false },
};

export const libraryBootstrap: LibraryBootstrap = {
  ...base,
  mode: "library",
  libraryDocumentId: "d".repeat(64),
  dictionaries: { installed: 0, operationActive: false },
  application: { name: "Elephant Ladder", version: "0.1.0" },
};

const mountedPublication = {
  facts: {
    package: {
      metadata: { title: [{ content: "Moby-Dick" }], creator: [], language: [] },
      spine: { pageProgressionDirection: null },
    },
  },
  mount: {
    baseUrl: "http://publication.test/",
    remoteResources: "blocked",
    contentOrigin: "isolated-publication-origin",
  },
} as unknown as MountedPublication;

export const readingBootstrap: ReadingBootstrap = {
  ...base,
  mode: "reading",
  libraryDocumentId: null,
  publicationOrigin: "http://publication.test",
  measurementOrigin: "http://measurement.test",
  publicationId: "e".repeat(32),
  publication: mountedPublication,
  pagination: { state: "unavailable", reason: "capabilities-unavailable" },
  initialReadingPosition: null,
  positionRevision: 0,
  annotations: { revision: 0, records: [] },
  annotationIdentifiers: [],
  policy: { activeContent: "disabled", remoteResources: "blocked" },
  mediaOverlay: { status: "absent" },
  dictionary: null,
};

export function publicationSummary(id: string, title = `Book ${id}`): PublicationSummary {
  return {
    publicationId: id.padEnd(32, "0"),
    title,
    titleDirection: null,
    creators: [],
    creatorDirections: [],
    language: null,
    readingDirection: null,
    importedAt: "2026-09-01 10:00:00",
    lastReadAt: null,
    hasReadingPosition: false,
    hasCover: false,
  };
}

/** A fetch whose responses the test releases one request at a time. */
export function deferredFetch() {
  const requests: {
    readonly url: string;
    readonly init: RequestInit | undefined;
    readonly respond: (response: Response) => void;
  }[] = [];
  const fetch = (input: RequestInfo | URL, init?: RequestInit): Promise<Response> =>
    new Promise((resolve) => {
      requests.push({
        url: input instanceof Request ? input.url : String(input),
        init,
        respond: resolve,
      });
    });
  return { fetch, requests };
}
