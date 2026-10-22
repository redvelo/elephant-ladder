import { atom } from "jotai";
import type { AnnotationLinkArrival } from "../annotations/links";
import type { OpenedPublicationArrival } from "../host/opened-publications";

/** The shelf's own pager, registered while a library page is on screen. */
export const shelfPagerAtom = atom<((delta: number) => void) | null>(null);

export const arrivedLinkAtom = atom<AnnotationLinkArrival | null>(null);
export const arrivedPublicationAtom = atom<OpenedPublicationArrival | null>(null);
