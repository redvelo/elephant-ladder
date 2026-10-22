import { atom } from "jotai";
import type { PublicationSummary } from "../host/bootstrap/types";

/** The one surface the library shell has open. They never stack: opening one replaces another. */
export type LibrarySurface =
  | {
      readonly kind: "book-details";
      readonly publication: PublicationSummary;
      readonly returnFocus: HTMLElement | null;
    }
  | {
      readonly kind: "reflections";
      readonly publication: PublicationSummary;
      readonly returnFocus: HTMLElement;
    }
  | { readonly kind: "dictionaries"; readonly returnFocus: HTMLElement | null }
  | { readonly kind: "notices"; readonly returnFocus: HTMLElement | null }
  | { readonly kind: "shortcuts"; readonly returnFocus: HTMLElement | null };

export const librarySurfaceAtom = atom<LibrarySurface | null>(null);

/** Where focus goes when a decision the shell raised closes. A surface carries its own. */
export const returnFocusAtom = atom<HTMLElement | null>(null);

export const closeLibrarySurfaceAtom = atom(null, (_get, set): void => {
  set(librarySurfaceAtom, null);
});
