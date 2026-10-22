import { Button } from "epub-navigator/ui";
import { AppDialog } from "../ui/dialog";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect } from "react";
import { formatBytes } from "../lib/format";
import {
  isTerminalPhase,
  type DictionaryLanguage,
  type DictionaryOperation,
  type InstalledLanguage,
} from "./model";
import {
  cancelDictionaryInstallAtom,
  installLanguageAtom,
  openDictionariesAtom,
  removeLanguageAtom,
  setLanguageAudioAtom,
} from "./dispatch";
import {
  dictionariesBusyAtom,
  dictionariesDialogOperationAtom,
  dictionariesInstalledAtom,
  dictionariesLanguagesAtom,
  dictionariesViewAtom,
} from "./state";

export interface DictionariesDialogProps {
  readonly portalContainer: HTMLElement | null;
  readonly returnFocus: HTMLElement | null;
  readonly onClose: () => void;
}

const phaseLabels: Record<DictionaryOperation["phase"], string> = {
  catalog: "Reading the catalog",
  preflight: "Checking space",
  downloading: "Downloading",
  verifying: "Verifying",
  installing: "Installing",
  completed: "Installed",
  cancelled: "Cancelled",
  failed: "Failed",
  interrupted: "Interrupted",
};

export function DictionariesDialog({
  portalContainer,
  returnFocus,
  onClose,
}: DictionariesDialogProps) {
  const view = useAtomValue(dictionariesViewAtom);
  const languages = useAtomValue(dictionariesLanguagesAtom);
  const installed = useAtomValue(dictionariesInstalledAtom);
  const operation = useAtomValue(dictionariesDialogOperationAtom);
  const busy = useAtomValue(dictionariesBusyAtom);
  const openDictionaries = useSetAtom(openDictionariesAtom);
  const install = useSetAtom(installLanguageAtom);
  const cancel = useSetAtom(cancelDictionaryInstallAtom);
  const remove = useSetAtom(removeLanguageAtom);
  const setAudio = useSetAtom(setLanguageAudioAtom);

  useEffect(() => openDictionaries(), [openDictionaries]);

  const running = operation !== null && !isTerminalPhase(operation.phase);

  return (
    <AppDialog
      kicker="Offline"
      title="Dictionaries"
      description="Dictionaries are downloaded once and then work without a network. Pronunciations play from Wikimedia Commons unless you keep them on this device."
      pageLabel="Dictionaries"
      container={portalContainer}
      returnFocus={returnFocus}
      onClose={onClose}
      actions={
        operation === null ? undefined : running ? (
          <Button variant="outline" className="dictionary-background-button" onClick={onClose}>
            <span>Continue in background</span>
            <small aria-live="polite">{operationProgress(operation)}</small>
          </Button>
        ) : operation.phase === "failed" ? (
          <p className="app-dialog-actions-status acquisition-error" role="alert">
            {operation.failure ?? "The dictionary could not be installed."}
          </p>
        ) : (
          <p className="app-dialog-actions-status" role="status">
            {operationProgress(operation)}
          </p>
        )
      }
    >
      {view.status === "loading" ? (
        <p className="acquisition-status" role="status">
          Loading dictionaries...
        </p>
      ) : view.status === "failed" ? (
        <p className="acquisition-status acquisition-error" role="alert">
          {view.message}
        </p>
      ) : view.status === "offline" ? (
        <OfflineList
          languages={installed}
          busy={busy}
          onRemove={(corpusLanguage) => void remove(corpusLanguage)}
        />
      ) : (
        <ul className="dictionary-list">
          {languages.map((language) => {
            const active = running && operation?.corpusLanguage === language.corpusLanguage;
            return (
              <li key={language.corpusLanguage} className="dictionary-pack">
                <LanguageFacts language={language} />
                <div className="dictionary-pack-actions">
                  {language.complete && language.stored !== null ? (
                    <Button
                      variant="outline"
                      disabled={busy}
                      onClick={() =>
                        void setAudio(language, language.audio === "stored" ? "streamed" : "stored")
                      }
                    >
                      {language.audio === "stored" ? "Stream recordings" : "Store recordings"}
                    </Button>
                  ) : null}
                  {active ? (
                    <Button variant="outline" onClick={() => void cancel()}>
                      Cancel
                    </Button>
                  ) : (
                    <>
                      {/* An install that stopped before the pronunciations left the language
                          incomplete, and finishing it asks only for what is missing. */}
                      {language.complete ? null : (
                        <Button
                          variant="outline"
                          disabled={busy}
                          onClick={() => void install(language)}
                        >
                          {language.installed ? "Finish installing" : "Install"}
                        </Button>
                      )}
                      {language.installed ? (
                        <Button
                          variant="outline"
                          disabled={busy}
                          onClick={() => void remove(language.corpusLanguage)}
                        >
                          Remove
                        </Button>
                      ) : null}
                    </>
                  )}
                </div>
              </li>
            );
          })}
          {languages.length === 0 ? (
            <li className="acquisition-status">No dictionaries are published yet.</li>
          ) : null}
        </ul>
      )}
    </AppDialog>
  );
}

/**
 * The languages this device holds, which is what there is to show without a catalog.
 *
 * Installing a language or changing where its recordings come from both need the catalog,
 * so removing one is the only action left.
 */
function OfflineList({
  languages,
  busy,
  onRemove,
}: {
  readonly languages: readonly InstalledLanguage[];
  readonly busy: boolean;
  readonly onRemove: (corpusLanguage: string) => void;
}) {
  return (
    <>
      <p className="acquisition-status" role="status">
        The dictionary catalog could not be reached. These dictionaries are on this device and keep
        working; installing another one needs a network.
      </p>
      <ul className="dictionary-list">
        {languages.map((language) => (
          <li key={language.corpusLanguage} className="dictionary-pack">
            <div className="dictionary-pack-facts">
              <strong>{languageName(language.corpusLanguage)}</strong>
              <span>{formatBytes(language.installedBytes)}</span>
              <span>{installedAudio(language.audio)}</span>
            </div>
            <div className="dictionary-pack-actions">
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => onRemove(language.corpusLanguage)}
              >
                Remove
              </Button>
            </div>
          </li>
        ))}
        {languages.length === 0 ? (
          <li className="acquisition-status">No dictionaries are installed on this device.</li>
        ) : null}
      </ul>
    </>
  );
}

function installedAudio(audio: InstalledLanguage["audio"]): string {
  switch (audio) {
    case "stored":
      return "Pronunciations kept on this device";
    case "streamed":
      return "Pronunciations play from Wikimedia Commons, which needs a network";
    case "none":
      return "No pronunciations";
  }
}

/** How far an install has got, worded for the button that leaves it running. */
function operationProgress(operation: DictionaryOperation): string {
  const phase = phaseLabels[operation.phase];
  return operation.phase === "downloading" && operation.bytesTotal > 0
    ? `${phase} · ${formatBytes(operation.bytesCompleted)} of ${formatBytes(operation.bytesTotal)}`
    : phase;
}

function LanguageFacts({ language }: { readonly language: DictionaryLanguage }) {
  const { corpus, streamed, stored } = language;
  const recordings = streamed?.recordings ?? stored?.recordings ?? 0;
  const storedHere = language.audio === "stored" && stored !== null;
  const size =
    corpus.installedBytes + (storedHere ? stored.installedBytes : (streamed?.installedBytes ?? 0));
  // An install that stopped before the pronunciations is incomplete rather than a language
  // that plays from Commons, and saying so is what makes finishing it make sense.
  const pronunciations = storedHere
    ? "kept on this device"
    : language.installed && !language.complete
      ? "not installed yet"
      : "played from Wikimedia Commons, not stored on this device";
  return (
    <div className="dictionary-pack-facts">
      <strong>{languageName(corpus.corpusLanguage)}</strong>
      <span>
        {corpus.edition} · {corpus.dumpDate} · {formatBytes(size)}
      </span>
      {recordings === 0 ? null : (
        <span>
          {recordings.toLocaleString("en")} pronunciations · {pronunciations}
        </span>
      )}
      <span className="dictionary-pack-licenses">{corpus.licenses.join(", ")}</span>
    </div>
  );
}

function languageName(code: string): string {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" }).of(code) ?? code;
  } catch {
    return code;
  }
}
