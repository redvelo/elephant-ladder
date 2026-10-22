import { NavigatorReader } from "epub-navigator/react";
import { Button } from "epub-navigator/ui";
import { useAtomValue, useSetAtom, useStore } from "jotai";
import { useEffect, useMemo } from "react";
import { useReadingActivity } from "../reading-activity/use-reading-activity";
import { FrontLightControl, SideButtonsRow } from "../device/controls";
import { useDeviceSettings } from "../device/settings";
import { useWindowFullscreen } from "../device/window-fullscreen";
import { openPublicationAtom, returnToLibraryAtom } from "../library/open";
import { createNoteReader } from "./notes";
import { changeReaderPreferencesAtom } from "./preferences";
import { closePublicationAtom, startReaderServicesAtom } from "./session";
import { copyAnnotationAtom, mutateAnnotationsAtom } from "./annotations";
import {
  networkActivityAtom,
  preferenceErrorAtom,
  readerHandleAtom,
  readerNoticeAtom,
  readingSessionIdAtom,
} from "./state";
import { openExternalLinkAtom } from "./external-navigation/dispatch";
import { recordReaderLocationAtom, recordReaderSummaryAtom } from "./position";
import { reportNarrationAtom } from "./narration";
import {
  bootstrapAtom,
  diagnosticsAtom,
  interactionFeedbackAtom,
  readerViewAtom,
  type ReaderViewState,
} from "../app/session";
import { MetadataText } from "../library/metadata-text";
import "./reader.css";

export function ReaderSection() {
  const bootstrap = useAtomValue(bootstrapAtom);
  const view = useAtomValue(readerViewAtom);
  const openPublication = useSetAtom(openPublicationAtom);
  const closePublication = useSetAtom(closePublicationAtom);
  const returnToLibrary = useSetAtom(returnToLibraryAtom);
  if (view.status === "library" || view.status === "opening" || view.status === "preparing") {
    return null;
  }
  return (
    <section
      className="product-reader"
      aria-label="Publication reader"
      data-qualification-publication-id={
        bootstrap?.mode === "reading" ? bootstrap.publicationId : undefined
      }
      data-qualification-publication-origin={
        bootstrap?.mode === "reading" ? bootstrap.publicationOrigin : undefined
      }
      data-qualification-launch-id={bootstrap?.mode === "reading" ? bootstrap.launchId : undefined}
    >
      {view.status === "reading" ? (
        <ReadingSession view={view} />
      ) : view.status === "closing" ? (
        <div className="product-reader-status" role="status">
          <p className="kicker">Closing publication</p>
          <h1 dir={view.publication.titleDirection ?? "auto"}>
            <MetadataText
              text={view.publication.title}
              direction={view.publication.titleDirection}
            />
          </h1>
        </div>
      ) : (
        <div className="product-reader-status" role="alert">
          <p className="kicker">
            {view.operation === "open" ? "Publication could not open" : "Reader could not close"}
          </p>
          <h1 dir={view.publication.titleDirection ?? "auto"}>
            <MetadataText
              text={view.publication.title}
              direction={view.publication.titleDirection}
            />
          </h1>
          <p>{view.message}</p>
          <div className="acquisition-actions">
            <Button
              onClick={() => {
                if (view.operation === "open") {
                  void openPublication(view.publication);
                } else if (view.session !== null) {
                  void closePublication(view.session);
                }
              }}
            >
              Retry
            </Button>
            {view.operation === "open" ? (
              <Button variant="outline" onClick={() => returnToLibrary()}>
                Return to library
              </Button>
            ) : null}
          </div>
        </div>
      )}
    </section>
  );
}

function ReadingSession({ view }: { view: Extract<ReaderViewState, { status: "reading" }> }) {
  const store = useStore();
  const bootstrap = useAtomValue(bootstrapAtom);
  const sessionId = useAtomValue(readingSessionIdAtom);
  const networkActivity = useAtomValue(networkActivityAtom);
  const diagnostics = useAtomValue(diagnosticsAtom);
  const preferenceError = useAtomValue(preferenceErrorAtom);
  const readerNotice = useAtomValue(readerNoticeAtom);
  const interactionFeedback = useAtomValue(interactionFeedbackAtom);
  const device = useDeviceSettings();
  const windowFullscreen = useWindowFullscreen();
  const setReaderHandle = useSetAtom(readerHandleAtom);
  const setReaderNotice = useSetAtom(readerNoticeAtom);
  const startReaderServices = useSetAtom(startReaderServicesAtom);
  const changePreferences = useSetAtom(changeReaderPreferencesAtom);
  const reportNarration = useSetAtom(reportNarrationAtom);
  const mutateAnnotations = useSetAtom(mutateAnnotationsAtom);
  const copyAnnotation = useSetAtom(copyAnnotationAtom);
  const recordLocation = useSetAtom(recordReaderLocationAtom);
  const recordSummary = useSetAtom(recordReaderSummaryAtom);
  const openExternalLink = useSetAtom(openExternalLinkAtom);
  const closePublication = useSetAtom(closePublicationAtom);
  const sessionToken = bootstrap?.sessionToken;
  const activity = useReadingActivity(sessionToken, view.session.id);
  const readNote = useMemo(
    () =>
      sessionToken === undefined || sessionId === null
        ? undefined
        : createNoteReader({ sessionToken, readerSessionId: sessionId }),
    [sessionToken, sessionId],
  );

  useEffect(() => {
    if (sessionId === null || sessionToken === undefined) return;
    return startReaderServices(sessionId, sessionToken);
  }, [sessionId, sessionToken, startReaderServices]);

  const frontLight = device.frontLight ? <FrontLightControl /> : undefined;
  // The reader's own Device section carries every device control; the quick panel carries
  // only the one that belongs in a glance.
  const deviceSettings =
    device.frontLight || device.sideButtons ? (
      <>
        {device.frontLight ? <FrontLightControl /> : null}
        {device.sideButtons ? <SideButtonsRow /> : null}
      </>
    ) : undefined;

  return (
    <>
      <NavigatorReader
        store={store}
        networkActivity={networkActivity}
        ref={setReaderHandle}
        className="product-navigator-reader"
        session={view.session}
        frameSandbox={
          view.policy.activeContent === "enabled"
            ? { allowForms: true, allowModals: true }
            : undefined
        }
        initialReadingPosition={view.initialReadingPosition}
        annotations={view.annotations}
        readerTypefaces={bootstrap?.readerTypefaces ?? []}
        preferences={view.preferences}
        onPreferencesChange={changePreferences}
        pagination={
          view.pagination.state === "available" ? view.pagination.capabilities : undefined
        }
        paginationAvailability={view.pagination.state === "unavailable" ? "unavailable" : undefined}
        search={view.search}
        dictionary={view.dictionary}
        mediaOverlays={view.mediaOverlays}
        onNarrationChange={(narration) => reportNarration(narration, view.publication.title)}
        diagnostics={diagnostics?.diagnostics}
        onAnnotationMutation={mutateAnnotations}
        annotationActions={(record) =>
          record.type === "bookmark"
            ? [
                {
                  id: "copy-link",
                  label: "Copy link",
                  onSelect: () => copyAnnotation(record, "link"),
                },
              ]
            : [
                {
                  id: "copy-markdown",
                  label: "Copy as Markdown",
                  onSelect: () => copyAnnotation(record, "markdown"),
                },
                {
                  id: "copy-link",
                  label: "Copy link",
                  onSelect: () => copyAnnotation(record, "link"),
                },
              ]
        }
        onLocationChange={recordLocation}
        onReadingSummaryChange={recordSummary}
        interactionFeedback={interactionFeedback}
        onReadingActivityChange={activity.observe}
        onLinkActivation={(activation) => void openExternalLink(activation)}
        onExternalFormAttempt={() => setReaderNotice("Blocked external form attempt.")}
        hostControls={{
          quickMenu: frontLight,
          deviceSettings,
          readNote,
          fullscreen: windowFullscreen,
        }}
        onLibraryRequest={() => {
          void activity
            .flush()
            .then(() => closePublication(view.session))
            .catch(() => setReaderNotice("Reading activity could not be saved. Please try again."));
        }}
      />
      {preferenceError === null ? null : (
        <p className="position-error" role="alert">
          Reader preferences could not be saved: {preferenceError}
        </p>
      )}
      {readerNotice === null ? null : (
        <p className="reader-notice" role="status" aria-live="polite">
          {readerNotice}
        </p>
      )}
    </>
  );
}
