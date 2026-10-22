import type { NavigatorReaderHandle } from "epub-navigator/react";
import type {
  PublicationDictionaryCapabilities,
  PublicationMediaOverlayCapabilities,
  PublicationPaginationCapabilities,
  PublicationSearchCapabilities,
} from "epub-navigator/runtime";
import { atom, type Getter } from "jotai";
import type { NetworkActivity } from "../host/network-activity";
import { bootstrapAtom, diagnosticsAtom, readerViewAtom } from "../app/session";
import type {
  ExternalNavigationConsent,
  ExternalNavigationPrompt,
} from "./external-navigation/consent";
import type { NarrationPlayback } from "./narration-playback";
import type { PreferencePersistenceOwner } from "./persistence/preferences";
import type { PositionIssues, PositionPersistenceOwner } from "./persistence/position";

interface ReaderCapabilities {
  readonly pagination: PublicationPaginationCapabilities | null;
  readonly search: PublicationSearchCapabilities | null;
  readonly dictionary: PublicationDictionaryCapabilities | null;
  readonly mediaOverlays: PublicationMediaOverlayCapabilities | null;
}

export const readerHandleAtom = atom<NavigatorReaderHandle | null>(null);
export const readerCapabilitiesAtom = atom<ReaderCapabilities | null>(null);
export const positionOwnerAtom = atom<PositionPersistenceOwner | null>(null);
export const preferenceOwnerAtom = atom<PreferencePersistenceOwner | null>(null);
export const closePendingAtom = atom(false);
export const externalNavigationAtom = atom<ExternalNavigationConsent | null>(null);
export const narrationPlaybackAtom = atom<NarrationPlayback | null>(null);

export const preferenceErrorAtom = atom<string | null>(null);
export const readerNoticeAtom = atom<string | null>(null);
export const networkActivityAtom = atom<NetworkActivity | null>(null);
export const externalNavigationPromptAtom = atom<ExternalNavigationPrompt | null>(null);

/** The mounted reader's session identity, stable while the reader view changes around it. */
export const readingSessionIdAtom = atom((get) => {
  const bootstrap = get(bootstrapAtom);
  const view = get(readerViewAtom);
  return bootstrap?.mode === "reading" &&
    view.status === "reading" &&
    view.session.id === bootstrap.readerSessionId
    ? bootstrap.readerSessionId
    : null;
});

export function readerRef(get: Getter): { readonly current: NavigatorReaderHandle | null } {
  return {
    get current() {
      return get(readerHandleAtom);
    },
  };
}

export function positionOwnerRef(get: Getter): {
  readonly current: PositionPersistenceOwner | null;
} {
  return {
    get current() {
      return get(positionOwnerAtom);
    },
  };
}

export function positionIssues(get: Getter): PositionIssues {
  return {
    capture: (outcome, reason) => get(diagnosticsAtom)?.recordReadingPosition(outcome, reason),
    write: () => get(diagnosticsAtom)?.recordReadingPosition("write-failed", null),
  };
}
