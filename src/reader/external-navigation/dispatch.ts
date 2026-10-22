import type { NavigatorLinkActivation } from "epub-navigator/react";
import { atom } from "jotai";
import { bootstrapAtom } from "../../app/session";
import { closePendingAtom, externalNavigationAtom, readerNoticeAtom } from "../state";
import type { ExternalNavigationPrompt } from "./consent";

export const openExternalLinkAtom = atom(
  null,
  async (get, set, activation: NavigatorLinkActivation): Promise<void> => {
    if (activation.internal) return;
    if (activation.download !== null) {
      set(readerNoticeAtom, "Publication downloads are not supported.");
      return;
    }
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || bootstrap.mode !== "reading" || get(closePendingAtom)) return;
    const consent = get(externalNavigationAtom);
    if (consent?.readerSessionId !== bootstrap.readerSessionId) return;
    await consent.requestOpen(activation.resolvedUrl);
  },
);

export const decideExternalNavigationAtom = atom(
  null,
  (get, _set, prompt: ExternalNavigationPrompt, allowed: boolean): void => {
    void get(externalNavigationAtom)?.decide(prompt, allowed);
  },
);
