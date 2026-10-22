import type { NavigatorNarrationStatus } from "epub-navigator/react";
import { atom } from "jotai";
import { narrationPlaybackAtom } from "./state";

export const reportNarrationAtom = atom(
  null,
  (get, _set, narration: NavigatorNarrationStatus, publicationTitle: string): void => {
    get(narrationPlaybackAtom)?.report(narration, publicationTitle);
  },
);
