import { atom } from "jotai";
import { atomEffect } from "jotai-effect";
import { watchAnnotationLinks } from "../annotations/links";
import { watchOpenedPublications } from "../host/opened-publications";
import { loadBootstrap } from "../host/bootstrap/parse";
import type { Bootstrap } from "../host/bootstrap/types";
import { createTrustedDiagnostics, describeError } from "../host/requests";
import { abortLibraryOperationsAtom } from "../library/dispatch";
import { importOpenedPublicationAtom } from "../library/import";
import { openPublicationAtom } from "../library/open";
import {
  libraryBusyAtom,
  libraryErrorAtom,
  libraryLoadedAtom,
  publicationsAtom,
} from "../library/state";
import { closePublicationAtom, startReadingSessionAtom } from "../reader/session";
import { readerNoticeAtom } from "../reader/state";
import { arrivedLinkAtom, arrivedPublicationAtom } from "./state";
import { bootstrapAtom, bootstrapErrorAtom, diagnosticsAtom, readerViewAtom } from "./session";

const applyBootstrapAtom = atom(null, (get, set, configuration: Bootstrap): void => {
  get(diagnosticsAtom)?.close();
  const diagnostics = createTrustedDiagnostics(
    configuration.diagnosticsLevel,
    configuration.sessionToken,
  );
  set(diagnosticsAtom, diagnostics);
  // Diagnostics intake is scoped to a reader session, which numbers its records in one
  // contiguous sequence. A record made in the library belongs to no session, so it is refused.
  if (configuration.mode === "reading") {
    void diagnostics.recordInterfaceFontEvidence();
    set(startReadingSessionAtom, configuration);
  }
  set(bootstrapAtom, configuration);
});

/** Consumes the native bootstrap, returning the teardown for everything the page started. */
export const startAppAtom = atom(null, (get, set): (() => void) => {
  let active = true;
  void loadBootstrap()
    .then((configuration) => {
      if (active) set(applyBootstrapAtom, configuration);
    })
    .catch((cause: unknown) => {
      if (active) set(bootstrapErrorAtom, describeError(cause));
    });
  return () => {
    active = false;
    get(diagnosticsAtom)?.close();
    set(abortLibraryOperationsAtom);
  };
});

export const watchAnnotationLinksAtom = atom(
  null,
  (_get, set, sessionToken: string): (() => void) => {
    const controller = new AbortController();
    void watchAnnotationLinks(sessionToken, controller.signal, (link) =>
      set(arrivedLinkAtom, link),
    );
    return () => controller.abort();
  },
);

/** An arrived annotation link waits until the library can act on it, then is consumed once. */
export const followArrivedLinkEffect = atomEffect((get, set) => {
  const arrived = get(arrivedLinkAtom);
  if (arrived === null) return;
  const missingMessage = "The linked book is not in this library.";
  const view = get(readerViewAtom);
  if (view.status === "reading") {
    set(arrivedLinkAtom, null);
    if (arrived.status === "missing") set(readerNoticeAtom, missingMessage);
    else void set(closePublicationAtom, view.session);
    return;
  }
  if (get(bootstrapAtom)?.mode !== "library" || view.status !== "library") return;
  if (!get(libraryLoadedAtom) || get(libraryBusyAtom)) return;
  set(arrivedLinkAtom, null);
  const publication =
    arrived.status === "open"
      ? get(publicationsAtom).find((candidate) => candidate.publicationId === arrived.publicationId)
      : undefined;
  if (publication === undefined) set(libraryErrorAtom, missingMessage);
  else void set(openPublicationAtom, publication);
});

export const watchOpenedPublicationsAtom = atom(
  null,
  (_get, set, sessionToken: string): (() => void) => {
    const controller = new AbortController();
    void watchOpenedPublications(sessionToken, controller.signal, (arrival) =>
      set(arrivedPublicationAtom, arrival),
    );
    return () => controller.abort();
  },
);

/** A handed publication waits for a library that can import it, closing an open book first. */
export const importArrivedPublicationEffect = atomEffect((get, set) => {
  const arrived = get(arrivedPublicationAtom);
  if (arrived === null) return;
  const view = get(readerViewAtom);
  if (view.status === "reading") {
    // Asking to read a book closes the one in hand. Dropping books onto the window does not:
    // they wait for the reader to be put down, which the host requires anyway.
    if (arrived.intent === "read") void set(closePublicationAtom, view.session);
    return;
  }
  if (get(bootstrapAtom)?.mode !== "library" || view.status !== "library") return;
  if (!get(libraryLoadedAtom) || get(libraryBusyAtom)) return;
  set(arrivedPublicationAtom, null);
  void set(importOpenedPublicationAtom, arrived);
});
