import type {
  NavigatorAnnotationProjection,
  NavigatorReaderPreferences,
  NavigatorReaderSession,
  NavigatorReadingPosition,
} from "epub-navigator/react";
import type {
  PublicationDictionaryCapabilities,
  PublicationMediaOverlayCapabilities,
  PublicationPaginationCapabilities,
  PublicationSearchCapabilities,
} from "epub-navigator/runtime";
import { atom } from "jotai";
import {
  type Bootstrap,
  type PublicationSummary,
  type ResolvedPublicationPolicy,
} from "../host/bootstrap/types";
import type { TrustedNavigatorDiagnostics } from "../host/diagnostics/records";

export type ReaderViewState =
  | { status: "library" }
  | { status: "opening"; publication: PublicationSummary }
  | { status: "preparing"; publication: PublicationSummary }
  | {
      status: "reading";
      publication: PublicationSummary;
      session: NavigatorReaderSession;
      initialReadingPosition: NavigatorReadingPosition | null;
      annotations: NavigatorAnnotationProjection;
      preferences: NavigatorReaderPreferences;
      policy: ResolvedPublicationPolicy;
      pagination:
        | { readonly state: "available"; readonly capabilities: PublicationPaginationCapabilities }
        | { readonly state: "unavailable" };
      search: PublicationSearchCapabilities;
      dictionary: PublicationDictionaryCapabilities | undefined;
      mediaOverlays: PublicationMediaOverlayCapabilities | undefined;
    }
  | {
      status: "closing";
      publication: PublicationSummary;
      session: NavigatorReaderSession;
    }
  | {
      status: "error";
      operation: "open" | "close";
      publication: PublicationSummary;
      session: NavigatorReaderSession | null;
      message: string;
    };

export const LIBRARY_VIEW: ReaderViewState = { status: "library" };

export const bootstrapAtom = atom<Bootstrap | null>(null);
export const bootstrapErrorAtom = atom<string | null>(null);
export const diagnosticsAtom = atom<TrustedNavigatorDiagnostics | null>(null);
export const readerViewAtom = atom<ReaderViewState>(LIBRARY_VIEW);

/** A display that ghosts gets minimal feedback. The host says which display it drives. */
export const interactionFeedbackAtom = atom<"standard" | "minimal">((get) =>
  get(bootstrapAtom)?.display.technology === "eink" ? "minimal" : "standard",
);
