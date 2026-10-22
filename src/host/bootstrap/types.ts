import type { MountedPublication, NavigatorMediaOverlayAvailability } from "epub-navigator";
import type {
  NavigatorAnnotationProjection,
  NavigatorReaderPreferences,
  NavigatorReadingPosition,
} from "epub-navigator/react";
import type { DictionaryCapability } from "../../reader/capabilities/dictionary";
import type { PaginationDescriptor } from "../../reader/capabilities/pagination";

export interface BootstrapBase {
  display: {
    technology: "eink" | "unknown";
    /** The refresh mode as the device names it, such as `regal_plus`; only read on e-ink. */
    refreshMode?: string;
    /** Whether the device drags in its fast waveform, which costs a swipe turn a flash. */
    speedModeWhileDragging?: boolean;
  };
  readerSessionId: string;
  launchId: string;
  readerOrigin: string;
  assetOrigin: string;
  sessionToken: string;
  tauriIpcEnabled: boolean;
  nativeActions: {
    frontLightDialog: boolean;
    /** Whether this device has buttons of its own that can turn pages. */
    sideButtons: boolean;
    /** Whether the host has a window of its own whose fullscreen state the reader may change. */
    windowFullscreen: boolean;
  };
  diagnosticsLevel: "summary" | "trace";
  recoveryNavigationPath: string;
  preferences: NavigatorReaderPreferences;
  preferenceRevisions: PreferenceRevisions;
  readerTypefaces: readonly ("reader-serif" | "reader-sans")[];
}

export type Bootstrap =
  | (BootstrapBase & {
      mode: "library";
      libraryDocumentId: string;
      dictionaries: { installed: number; operationActive: boolean };
      application: { name: string; version: string };
    })
  | (BootstrapBase & {
      mode: "reading";
      libraryDocumentId: null;
      publicationOrigin: string;
      measurementOrigin: string;
      publicationId: string;
      publication: MountedPublication;
      pagination: PaginationDescriptor;
      initialReadingPosition: NavigatorReadingPosition | null;
      positionRevision: number;
      annotations: NavigatorAnnotationProjection;
      annotationIdentifiers: string[];
      policy: ResolvedPublicationPolicy;
      mediaOverlay: NavigatorMediaOverlayAvailability;
      dictionary: DictionaryCapability | null;
    });

export type LibraryBootstrap = Extract<Bootstrap, { mode: "library" }>;
export type ReadingBootstrap = Extract<Bootstrap, { mode: "reading" }>;

export type MetadataDirection = "ltr" | "rtl" | "auto";

export interface PublicationSummary {
  publicationId: string;
  title: string;
  /** What the publication declares about how its title reads, when it declares anything.
   * `creators` and `creatorDirections` line up by index, so the names stay plain for
   * sorting and search while each one still renders the way it was authored. */
  titleDirection: MetadataDirection | null;
  creators: string[];
  creatorDirections: (MetadataDirection | null)[];
  language: string | null;
  readingDirection: "ltr" | "rtl" | "default" | null;
  importedAt: string;
  lastReadAt: string | null;
  hasReadingPosition: boolean;
  hasCover: boolean;
}

export interface ResolvedPublicationPolicy {
  activeContent: "enabled" | "disabled";
  remoteResources: "allowed" | "blocked";
}

export interface PreferenceRevisions {
  global: number;
  publication: number;
}

export interface StoredPreferences {
  preferences: NavigatorReaderPreferences;
  revisions: PreferenceRevisions;
}
