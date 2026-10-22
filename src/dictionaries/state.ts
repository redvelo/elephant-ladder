import { atom } from "jotai";
import {
  dictionaryLanguages,
  installedLanguages,
  type CatalogPackSummary,
  type DictionaryInstallation,
  type DictionaryLanguage,
  type DictionaryOperation,
  type InstalledLanguage,
} from "./model";

export type DictionariesView =
  | { status: "loading" }
  | { status: "ready"; packs: CatalogPackSummary[]; installations: DictionaryInstallation[] }
  /** The catalog could not be reached, so only what the device holds can be offered. */
  | { status: "offline"; installations: DictionaryInstallation[] }
  | { status: "failed"; message: string };

export const LOADING: DictionariesView = { status: "loading" };

export const dictionariesViewAtom = atom<DictionariesView>(LOADING);

/** The catalog as installable languages, which is what the dialog offers. */
export const dictionariesLanguagesAtom = atom<DictionaryLanguage[]>((get) => {
  const view = get(dictionariesViewAtom);
  return view.status === "ready" ? dictionaryLanguages(view.packs) : [];
});

/** What the device holds, which is what an offline panel lists. */
export const dictionariesInstalledAtom = atom<InstalledLanguage[]>((get) => {
  const view = get(dictionariesViewAtom);
  return view.status === "ready" || view.status === "offline"
    ? installedLanguages(view.installations)
    : [];
});

export const dictionariesDialogOperationAtom = atom<DictionaryOperation | null>(null);
export const dictionariesBusyAtom = atom(false);
export const dictionariesAbortAtom = atom<AbortController | null>(null);

export const dictionaryOperationAtom = atom<DictionaryOperation | null>(null);
/** Bumped whenever there may be an install to follow, so the watcher restarts. */
export const dictionaryWatchAtom = atom(0);
