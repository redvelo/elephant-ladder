import { atom, type Getter } from "jotai";
import {
  activeOperation,
  cancelDictionaryOperation,
  deleteDictionaryInstallation,
  DictionaryCatalogUnavailable,
  fetchDictionaryCatalog,
  fetchDictionaryInstallations,
  fetchDictionaryOperations,
  pollDictionaryOperation,
  startDictionaryInstall,
  watchDictionaryOperations,
} from "./requests";
import {
  type DictionaryAudio,
  type DictionaryLanguage,
  type DictionaryRequestContext,
} from "./model";
import { bootstrapAtom } from "../app/session";
import {
  dictionariesAbortAtom,
  dictionariesBusyAtom,
  dictionariesDialogOperationAtom,
  dictionariesViewAtom,
  dictionaryOperationAtom,
  dictionaryWatchAtom,
  LOADING,
  type DictionariesView,
} from "./state";

function requestContext(get: Getter): DictionaryRequestContext | null {
  const bootstrap = get(bootstrapAtom);
  return bootstrap?.mode === "library"
    ? { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId }
    : null;
}

function failure(cause: unknown): DictionariesView {
  return { status: "failed", message: cause instanceof Error ? cause.message : String(cause) };
}

/**
 * Lists what is offered and what is installed.
 *
 * Installed dictionaries are local facts, so an unreachable catalog leaves the panel
 * offline with the languages the device already holds rather than with nothing at all.
 */
const loadDictionariesAtom = atom(null, async (get, set): Promise<void> => {
  const context = requestContext(get);
  if (context === null) return;
  try {
    const [catalog, installations] = await Promise.all([
      fetchDictionaryCatalog(context).catch((cause: unknown) => {
        if (cause instanceof DictionaryCatalogUnavailable) return null;
        throw cause;
      }),
      fetchDictionaryInstallations(context),
    ]);
    set(
      dictionariesViewAtom,
      catalog === null
        ? { status: "offline", installations }
        : { status: "ready", packs: catalog.packs, installations },
    );
  } catch (cause) {
    set(dictionariesViewAtom, failure(cause));
  }
});

/**
 * Loads the dialog and reattaches to an install that kept running while it was closed,
 * returning the teardown that stops following it.
 */

export const openDictionariesAtom = atom(null, (get, set): (() => void) => {
  set(dictionariesViewAtom, LOADING);
  set(dictionariesDialogOperationAtom, null);
  set(dictionariesBusyAtom, false);
  const controller = new AbortController();
  void (async () => {
    await set(loadDictionariesAtom);
    const context = requestContext(get);
    if (context === null || controller.signal.aborted) return;
    set(dictionariesAbortAtom, controller);
    try {
      const running = activeOperation(await fetchDictionaryOperations(context));
      if (running === null) return;
      set(dictionariesDialogOperationAtom, running);
      set(dictionariesBusyAtom, true);
      const finished = await pollDictionaryOperation(
        { ...context, signal: controller.signal },
        running.operationId,
        { onProgress: (operation) => set(dictionariesDialogOperationAtom, operation) },
      );
      set(dictionariesDialogOperationAtom, finished);
      if (finished.phase === "completed") await set(loadDictionariesAtom);
    } catch {
      // A listing failure is not worth replacing the pack list with an error.
    } finally {
      set(dictionariesBusyAtom, false);
      if (get(dictionariesAbortAtom) === controller) set(dictionariesAbortAtom, null);
    }
  })();
  return () => {
    controller.abort();
    get(dictionariesAbortAtom)?.abort();
  };
});

/**
 * Installs one language and follows it to its end, reporting progress to the dialog.
 *
 * Pronunciations are part of a language rather than a separate choice, so `audio` only
 * decides whether the recordings stream from Commons or are stored on the device. The host
 * installs everything the choice implies under one operation, so this is one download
 * whatever is already on the device.
 */
export const installLanguageAtom = atom(
  null,
  async (
    get,
    set,
    language: DictionaryLanguage,
    audio: DictionaryAudio = "streamed",
  ): Promise<void> => {
    const context = requestContext(get);
    if (context === null || (language.complete && language.audio === audio)) return;
    set(dictionariesBusyAtom, true);
    const controller = new AbortController();
    set(dictionariesAbortAtom, controller);
    try {
      const operationId = await startDictionaryInstall(context, language.corpusLanguage, audio);
      set(dictionaryWatchAtom, get(dictionaryWatchAtom) + 1);
      const finished = await pollDictionaryOperation(
        { ...context, signal: controller.signal },
        operationId,
        { onProgress: (operation) => set(dictionariesDialogOperationAtom, operation) },
      );
      set(dictionariesDialogOperationAtom, finished);
      await set(loadDictionariesAtom);
    } catch (cause) {
      set(dictionariesViewAtom, failure(cause));
    } finally {
      set(dictionariesBusyAtom, false);
      if (get(dictionariesAbortAtom) === controller) set(dictionariesAbortAtom, null);
    }
  },
);

export const setLanguageAudioAtom = atom(
  null,
  async (_get, set, language: DictionaryLanguage, audio: DictionaryAudio): Promise<void> => {
    await set(installLanguageAtom, language, audio);
  },
);

export const cancelDictionaryInstallAtom = atom(null, async (get): Promise<void> => {
  const context = requestContext(get);
  const operation = get(dictionariesDialogOperationAtom);
  if (context === null || operation === null) return;
  try {
    await cancelDictionaryOperation(context, operation.operationId);
  } catch {
    // The operation reports its own terminal phase through polling.
  }
});

export const removeLanguageAtom = atom(
  null,
  async (get, set, corpusLanguage: string): Promise<void> => {
    const context = requestContext(get);
    const view = get(dictionariesViewAtom);
    if (context === null || view.status === "loading" || view.status === "failed") return;
    const installed = view.installations.filter(
      (installation) => installation.corpusLanguage === corpusLanguage,
    );
    set(dictionariesBusyAtom, true);
    try {
      for (const installation of installed) {
        await deleteDictionaryInstallation(context, installation.packId, installation.packRevision);
      }
      await set(loadDictionariesAtom);
    } catch (cause) {
      set(dictionariesViewAtom, failure(cause));
    } finally {
      set(dictionariesBusyAtom, false);
    }
  },
);

export const watchDictionaryOperationsAtom = atom(null, (get, set, watch: number): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap?.mode === "library" && (bootstrap.dictionaries.operationActive || watch !== 0)) {
    void watchDictionaryOperations(
      {
        sessionToken: bootstrap.sessionToken,
        libraryDocumentId: bootstrap.libraryDocumentId,
        signal: controller.signal,
      },
      { onChange: (operation) => set(dictionaryOperationAtom, operation) },
    );
  }
  return () => controller.abort();
});
