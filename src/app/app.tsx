import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useState } from "react";
import { loadSettingsAtom } from "../settings/state";
import { ExternalNavigationDialog } from "../reader/external-navigation/dialog";
import { Library } from "../library/library";
import { PolicyDecisionDialog } from "../policy/decision-dialog";
import { ReaderSection } from "../reader/reader";
import {
  followArrivedLinkEffect,
  importArrivedPublicationEffect,
  startAppAtom,
  watchAnnotationLinksAtom,
  watchOpenedPublicationsAtom,
} from "./dispatch";
import { installBackHandlerAtom, installPageTurnHandlerAtom } from "./input";
import { closeReaderCapabilitiesAtom } from "../reader/session";
import { bootstrapAtom, bootstrapErrorAtom } from "./session";
import "./shell.css";

function App() {
  const bootstrap = useAtomValue(bootstrapAtom);
  const error = useAtomValue(bootstrapErrorAtom);
  const startApp = useSetAtom(startAppAtom);
  const installBackHandler = useSetAtom(installBackHandlerAtom);
  const installPageTurnHandler = useSetAtom(installPageTurnHandlerAtom);
  const watchAnnotationLinks = useSetAtom(watchAnnotationLinksAtom);
  const watchOpenedPublications = useSetAtom(watchOpenedPublicationsAtom);
  const closeReaderCapabilities = useSetAtom(closeReaderCapabilitiesAtom);
  const [portalContainer, setPortalContainer] = useState<HTMLElement | null>(null);
  const sessionToken = bootstrap?.sessionToken ?? null;
  const loadSettings = useSetAtom(loadSettingsAtom);
  useEffect(() => {
    if (sessionToken === null) return undefined;
    return loadSettings();
  }, [sessionToken, loadSettings]);
  useAtomValue(followArrivedLinkEffect);
  useAtomValue(importArrivedPublicationEffect);

  useEffect(() => startApp(), [startApp]);
  useEffect(() => installBackHandler(), [installBackHandler]);
  useEffect(() => installPageTurnHandler(), [installPageTurnHandler]);
  useEffect(() => {
    const consumeHistoryButton = (event: MouseEvent) => {
      if (event.button === 3 || event.button === 4) event.preventDefault();
    };
    for (const type of ["mousedown", "mouseup", "auxclick"] as const)
      window.addEventListener(type, consumeHistoryButton, true);
    return () => {
      for (const type of ["mousedown", "mouseup", "auxclick"] as const)
        window.removeEventListener(type, consumeHistoryButton, true);
    };
  }, []);

  useEffect(() => {
    if (sessionToken === null) return;
    return watchAnnotationLinks(sessionToken);
  }, [sessionToken, watchAnnotationLinks]);

  useEffect(() => {
    if (sessionToken === null) return;
    return watchOpenedPublications(sessionToken);
  }, [sessionToken, watchOpenedPublications]);

  useEffect(() => {
    const closeOnTeardown = (event: PageTransitionEvent): void => {
      if (!event.persisted) closeReaderCapabilities();
    };
    window.addEventListener("pagehide", closeOnTeardown);
    return () => {
      window.removeEventListener("pagehide", closeOnTeardown);
      closeReaderCapabilities();
    };
  }, [closeReaderCapabilities]);

  return (
    <main ref={setPortalContainer} className="elephant-shell">
      <PolicyDecisionDialog container={portalContainer} />
      <ExternalNavigationDialog container={portalContainer} />
      {error !== null && bootstrap === null ? (
        <p className="acquisition-status acquisition-error" role="alert">
          {error}
        </p>
      ) : null}
      <ReaderSection />
      {bootstrap?.mode === "library" ? <Library portalContainer={portalContainer} /> : null}
    </main>
  );
}

export default App;
