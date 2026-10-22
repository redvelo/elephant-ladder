import type { NavigatorReaderSession } from "epub-navigator/react";
import { atom } from "jotai";
import { bootstrapPublicationSummary } from "../host/bootstrap/publication";
import { type ReadingBootstrap } from "../host/bootstrap/types";
import { observeNetworkActivity } from "../host/network-activity";
import { isNavigationResponse, nativeRequest } from "../host/requests";
import { applyApplicationColorScheme } from "../app/color-scheme";
import { bootstrapAtom, diagnosticsAtom, readerViewAtom } from "../app/session";
import { createNativeDictionaryCapabilities } from "./capabilities/dictionary";
import { createNativeMediaOverlayCapabilities } from "./capabilities/media-overlays";
import { createLivePaginationCapabilities } from "./capabilities/pagination";
import { createNativeSearchCapabilities } from "./capabilities/search";
import { createExternalNavigationConsent } from "./external-navigation/consent";
import { createNarrationPlayback } from "./narration-playback";
import { createPositionPersistenceOwner, flushReadingPosition } from "./persistence/position";
import { flushPreferences } from "./persistence/preferences";
import { applyReaderPreferencesAtom } from "./preferences";
import { checkpointReadingPositionAtom } from "./position";
import {
  closePendingAtom,
  externalNavigationAtom,
  externalNavigationPromptAtom,
  narrationPlaybackAtom,
  networkActivityAtom,
  positionIssues,
  positionOwnerAtom,
  preferenceErrorAtom,
  preferenceOwnerAtom,
  readerCapabilitiesAtom,
  readerHandleAtom,
  readerNoticeAtom,
  readerRef,
} from "./state";

export const startReadingSessionAtom = atom(
  null,
  (_get, set, configuration: ReadingBootstrap): void => {
    const pagination =
      configuration.pagination.state === "available"
        ? {
            state: "available" as const,
            capabilities: createLivePaginationCapabilities(
              configuration.pagination,
              configuration.sessionToken,
            ),
          }
        : { state: "unavailable" as const };
    const search = createNativeSearchCapabilities(
      configuration.readerSessionId,
      configuration.sessionToken,
    );
    const dictionary =
      configuration.dictionary === null
        ? undefined
        : createNativeDictionaryCapabilities(
            configuration.readerSessionId,
            configuration.sessionToken,
            configuration.dictionary,
          );
    const mediaOverlays =
      configuration.mediaOverlay.status === "available"
        ? createNativeMediaOverlayCapabilities(
            configuration.readerSessionId,
            configuration.sessionToken,
            configuration.mediaOverlay,
          )
        : undefined;
    set(readerCapabilitiesAtom, {
      pagination: pagination.state === "available" ? pagination.capabilities : null,
      search,
      dictionary: dictionary ?? null,
      mediaOverlays: mediaOverlays ?? null,
    });
    set(
      positionOwnerAtom,
      createPositionPersistenceOwner(
        configuration.readerSessionId,
        configuration.sessionToken,
        configuration.positionRevision,
        configuration.initialReadingPosition,
      ),
    );
    set(preferenceOwnerAtom, {
      sessionId: configuration.readerSessionId,
      token: configuration.sessionToken,
      revisions: configuration.preferenceRevisions,
      pending: null,
      writePromise: null,
      stopping: false,
      terminalError: null,
    });
    applyApplicationColorScheme(configuration.preferences.colorScheme);
    set(readerViewAtom, {
      status: "reading",
      publication: bootstrapPublicationSummary(configuration),
      session: { id: configuration.readerSessionId, publication: configuration.publication },
      initialReadingPosition: configuration.initialReadingPosition,
      annotations: configuration.annotations,
      preferences: configuration.preferences,
      policy: configuration.policy,
      pagination,
      search,
      dictionary,
      mediaOverlays,
    });
  },
);

export const closeReaderCapabilitiesAtom = atom(null, (get, set): void => {
  const capabilities = get(readerCapabilitiesAtom);
  if (capabilities === null) return;
  set(readerCapabilitiesAtom, null);
  capabilities.pagination?.close();
  capabilities.search?.close();
  capabilities.dictionary?.close();
  capabilities.mediaOverlays?.close();
});

export const closePublicationAtom = atom(
  null,
  async (get, set, session: NavigatorReaderSession): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || get(closePendingAtom)) return;
    set(closePendingAtom, true);
    get(externalNavigationAtom)?.cancel();
    const issues = positionIssues(get);
    const owner = get(positionOwnerAtom);
    try {
      if (owner === null || owner.sessionId !== session.id) {
        throw new Error("Reading position session is unavailable.");
      }
      try {
        await flushReadingPosition(owner, readerRef(get), issues);
      } catch {
        issues.write();
      }
      const preferences = get(preferenceOwnerAtom);
      if (preferences === null || preferences.sessionId !== session.id) {
        throw new Error("Reader preference session is unavailable.");
      }
      preferences.stopping = true;
      await flushPreferences(
        preferences,
        (authoritative) => set(applyReaderPreferencesAtom, authoritative),
        (message) => set(preferenceErrorAtom, message),
      );
      if (preferences.terminalError !== null) throw preferences.terminalError;
      const response = await nativeRequest(
        "/native/reader/close",
        { sessionToken: bootstrap.sessionToken },
        { method: "POST", json: { readerSessionId: session.id } },
      );
      if (!response.ok) throw new Error(`Close failed with HTTP ${response.status}`);
      const closed: unknown = await response.json();
      if (!isNavigationResponse(closed)) throw new Error("Close returned invalid navigation data.");
      set(closeReaderCapabilitiesAtom);
      // The host has revoked the reader session, so anything still queued for it can only be
      // refused. The page lives on until the navigation below completes.
      get(diagnosticsAtom)?.close();
      window.location.replace(closed.navigationPath);
    } catch {
      if (owner !== null) owner.stopping = false;
      issues.write();
    } finally {
      set(closePendingAtom, false);
    }
  },
);

/** Starts the services a mounted reading session owns, returning their teardown. */
export const startReaderServicesAtom = atom(
  null,
  (get, set, sessionId: string, sessionToken: string): (() => void) => {
    const stopNetworkActivity = observeNetworkActivity(sessionId, sessionToken, (activity) =>
      set(networkActivityAtom, activity),
    );

    const playback = createNarrationPlayback(sessionId, sessionToken, {
      play: () => get(readerHandleAtom)?.playNarration(),
      pause: () => get(readerHandleAtom)?.pauseNarration(),
      next: () => get(readerHandleAtom)?.nextNarrationSegment(),
      previous: () => get(readerHandleAtom)?.previousNarrationSegment(),
    });
    set(narrationPlaybackAtom, playback);

    const consent = createExternalNavigationConsent({
      readerSessionId: sessionId,
      sessionToken,
      showPrompt: (prompt) => set(externalNavigationPromptAtom, prompt),
      reportError: (message) => set(readerNoticeAtom, message),
    });
    set(externalNavigationAtom, consent);
    const cancelExternalNavigation = (): void => consent.cancel();

    const checkpoint = (): void => set(checkpointReadingPositionAtom);
    const checkpointWhenHidden = (): void => {
      if (document.visibilityState === "hidden") checkpoint();
    };

    window.addEventListener("pagehide", cancelExternalNavigation);
    window.addEventListener("blur", checkpoint);
    window.addEventListener("pagehide", checkpoint);
    document.addEventListener("visibilitychange", checkpointWhenHidden);
    return () => {
      window.removeEventListener("pagehide", cancelExternalNavigation);
      window.removeEventListener("blur", checkpoint);
      window.removeEventListener("pagehide", checkpoint);
      document.removeEventListener("visibilitychange", checkpointWhenHidden);
      stopNetworkActivity();
      playback.close();
      if (get(narrationPlaybackAtom) === playback) set(narrationPlaybackAtom, null);
      consent.close();
      if (get(externalNavigationAtom) === consent) set(externalNavigationAtom, null);
    };
  },
);
