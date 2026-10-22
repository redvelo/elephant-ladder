import type { NavigatorReaderPreferences } from "epub-navigator/react";
import { atom } from "jotai";
import { applyApplicationColorScheme } from "../app/color-scheme";
import { readerViewAtom } from "../app/session";
import { flushPreferences } from "./persistence/preferences";
import { preferenceErrorAtom, preferenceOwnerAtom } from "./state";

function samePreferences(
  left: NavigatorReaderPreferences,
  right: NavigatorReaderPreferences,
): boolean {
  const keys = Object.keys(left) as (keyof NavigatorReaderPreferences)[];
  return (
    keys.length === Object.keys(right).length &&
    keys.every((key) => {
      const a = left[key];
      const b = right[key];
      return Array.isArray(a) && Array.isArray(b)
        ? a.length === b.length && a.every((value, index) => value === b[index])
        : a === b;
    })
  );
}

export const applyReaderPreferencesAtom = atom(
  null,
  (get, set, preferences: NavigatorReaderPreferences): void => {
    const view = get(readerViewAtom);
    if (view.status !== "reading" || samePreferences(view.preferences, preferences)) return;
    applyApplicationColorScheme(preferences.colorScheme);
    set(readerViewAtom, { ...view, preferences });
  },
);

export const changeReaderPreferencesAtom = atom(
  null,
  (get, set, preferences: NavigatorReaderPreferences): void => {
    const view = get(readerViewAtom);
    if (view.status !== "reading" || samePreferences(view.preferences, preferences)) return;
    set(applyReaderPreferencesAtom, preferences);
    const owner = get(preferenceOwnerAtom);
    if (owner === null || owner.stopping || owner.terminalError !== null) return;
    owner.pending = preferences;
    void flushPreferences(
      owner,
      (authoritative) => set(applyReaderPreferencesAtom, authoritative),
      (message) => set(preferenceErrorAtom, message),
    );
  },
);
