import { Button as BaseButton } from "@base-ui/react/button";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { useEffect, useRef, useState } from "react";
import { annotationImportRequestAtom, importAnnotationsAtom } from "../annotations/dispatch";
import { watchDictionaryOperationsAtom } from "../dictionaries/dispatch";
import { dictionaryWatchAtom } from "../dictionaries/state";
import { type PublicationSummary } from "../host/bootstrap/types";
import { journalsAtom, loadJournalAtom } from "../journal/state";
import { useVisualViewportHeight } from "../lib/use-visual-viewport";
import { SettingsPage } from "../settings/page";
import { destinationAtom } from "../app/destination";
import { bootstrapAtom, interactionFeedbackAtom, readerViewAtom } from "../app/session";
import { useBookAction } from "./actions";
import { destinations } from "./destinations";
import { LibraryDialogs } from "./dialogs";
import {
  loadLibraryAtom,
  showDictionariesAtom,
  showNoticesAtom,
  showReflectionsAtom,
  showShortcutsAtom,
} from "./dispatch";
import { importPublicationsAtom, importSamplePublicationAtom } from "./import";
import { LibraryNotices } from "./notices";
import { openPublicationAtom } from "./open";
import { HomePage } from "./home-page";
import { LibraryPage } from "./shelf-page";

import { ShortcutsDialog } from "./shortcuts-dialog";
import {
  featuredPublicationAtom,
  libraryBusyAtom,
  libraryLoadedAtom,
  libraryOpeningAtom,
  publicationsAtom,
} from "./state";
import { useLibraryKeyboard } from "./use-library-keyboard";
import { EmptyShelf, LibraryWelcome } from "./welcome";

/** The library's own shell: where it is, what it is doing, and which page is showing. */
export function Library({ portalContainer }: { readonly portalContainer: HTMLElement | null }) {
  const bootstrap = useAtomValue(bootstrapAtom);
  const pace = useAtomValue(interactionFeedbackAtom);
  const view = useAtomValue(readerViewAtom);
  const [destination, setDestination] = useAtom(destinationAtom);
  const libraryBusy = useAtomValue(libraryBusyAtom);
  const loaded = useAtomValue(libraryLoadedAtom);
  const libraryOpening = useAtomValue(libraryOpeningAtom);
  const featured = useAtomValue(featuredPublicationAtom);
  const publications = useAtomValue(publicationsAtom);
  const dictionaryWatch = useAtomValue(dictionaryWatchAtom);
  const journals = useAtomValue(journalsAtom);
  const showShortcuts = useSetAtom(showShortcutsAtom);
  const showNotices = useSetAtom(showNoticesAtom);
  const showReflections = useSetAtom(showReflectionsAtom);
  const loadLibrary = useSetAtom(loadLibraryAtom);
  const loadJournal = useSetAtom(loadJournalAtom);
  const watchDictionaryOperations = useSetAtom(watchDictionaryOperationsAtom);
  const importPublications = useSetAtom(importPublicationsAtom);
  const importSamplePublication = useSetAtom(importSamplePublicationAtom);
  const importAnnotations = useSetAtom(importAnnotationsAtom);
  const openPublication = useSetAtom(openPublicationAtom);
  const showDictionaries = useSetAtom(showDictionariesAtom);
  const [annotationImportRequest, setAnnotationImportRequest] = useAtom(
    annotationImportRequestAtom,
  );
  const acquisitionInput = useRef<HTMLInputElement>(null);
  const annotationInput = useRef<HTMLInputElement>(null);
  const [shell, setShell] = useState<HTMLDivElement | null>(null);
  const bookAction = useBookAction(annotationInput);
  const importBook = () => acquisitionInput.current?.click();
  const featuredId = featured?.publicationId;
  useEffect(() => {
    if (featuredId !== undefined) return loadJournal(featuredId);
    return undefined;
  }, [featuredId, loadJournal]);
  useLibraryKeyboard({
    active: view.status === "library",
    busy: libraryBusy,
    destination,
    shell,
    onImport: importBook,
  });
  useVisualViewportHeight(shell);
  useEffect(() => loadLibrary(), [loadLibrary]);
  useEffect(
    () => watchDictionaryOperations(dictionaryWatch),
    [watchDictionaryOperations, dictionaryWatch],
  );

  if (bootstrap?.mode !== "library") return null;

  const actions = {
    sessionToken: bootstrap.sessionToken,
    disabled: libraryBusy,
    onOpen: (publication: PublicationSummary) => void openPublication(publication),
  };

  return (
    <>
      <LibraryDialogs portalContainer={portalContainer} />
      {view.status === "library" || libraryOpening !== null ? (
        <div ref={setShell} className="library-shell" aria-busy={libraryBusy}>
          <nav className="app-navigation" aria-label="Main navigation">
            {destinations.map((item) => (
              <BaseButton
                key={item.id}
                aria-current={destination === item.id ? "page" : undefined}
                title={`${item.label} (Ctrl/Cmd+${item.id === "home" ? "1" : item.id === "library" ? "2" : ","})`}
                onClick={() => setDestination(item.id)}
              >
                <svg
                  viewBox="0 0 26 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  aria-hidden="true"
                >
                  {item.path}
                </svg>
                <span>{item.label}</span>
              </BaseButton>
            ))}
          </nav>
          <ShortcutsDialog portalContainer={portalContainer} />
          <input
            ref={acquisitionInput}
            className="visually-hidden"
            type="file"
            data-qualification-action="import-publication"
            accept=".epub,application/epub+zip"
            multiple
            disabled={libraryBusy}
            onChange={(event) => {
              const chosen = [...(event.currentTarget.files ?? [])];
              event.currentTarget.value = "";
              if (chosen.length > 0) void importPublications(chosen);
            }}
          />
          <input
            ref={annotationInput}
            className="visually-hidden"
            type="file"
            accept=".json,application/json,application/ld+json"
            onChange={(event) => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = "";
              const publication = annotationImportRequest;
              setAnnotationImportRequest(null);
              if (file === undefined || publication === null) return;
              void file.text().then((contents) => importAnnotations(publication, contents, false));
            }}
          />
          <LibraryNotices pace={pace} />
          <div className="app-page" hidden={destination !== "home"}>
            {featured !== null ? (
              <HomePage
                {...actions}
                featured={featured}
                publications={publications}
                journalState={journals[featured.publicationId]}
                portalContainer={portalContainer}
                onBookAction={bookAction}
                onReflection={(trigger) => showReflections(featured, trigger)}
                preparing={
                  libraryOpening?.status === "preparing" &&
                  libraryOpening.publication.publicationId === featured.publicationId
                }
              />
            ) : (
              <LibraryWelcome
                loaded={loaded}
                disabled={libraryBusy}
                onImport={importBook}
                onSample={() => void importSamplePublication()}
              />
            )}
          </div>
          <div className="app-page" hidden={destination !== "library"}>
            {featured !== null ? (
              <LibraryPage
                {...actions}
                publications={publications}
                onImport={importBook}
                portalContainer={portalContainer}
                onBookAction={bookAction}
              />
            ) : (
              <EmptyShelf loaded={loaded} disabled={libraryBusy} onImport={importBook} />
            )}
          </div>
          <div className="app-page" hidden={destination !== "settings"}>
            <SettingsPage
              disabled={libraryBusy}
              portalContainer={portalContainer}
              onDictionaries={(trigger) => showDictionaries(trigger)}
              onNotices={(trigger) => showNotices(trigger)}
              onShortcuts={(trigger) => showShortcuts(trigger)}
            />
          </div>
        </div>
      ) : null}
    </>
  );
}
