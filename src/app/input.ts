import { turnPaginatedSurface } from "epub-navigator/ui";
import { atom } from "jotai";
import { closePublicationAtom } from "../reader/session";
import { readerHandleAtom } from "../reader/state";
import { destinationAtom } from "./destination";
import { readerViewAtom } from "./session";
import { shelfPagerAtom } from "./state";

declare global {
  interface Window {
    __elephantLadderBack?: () => boolean;
    /** A page turn asked for by the device itself: -1 back, 1 forward. */
    __elephantLadderTurnPage?: (direction: number) => boolean;
  }
}

/** The device's page buttons, which reach neither the reader's keys nor the shelf's. */
export const installPageTurnHandlerAtom = atom(null, (get): (() => void) => {
  window.__elephantLadderTurnPage = (direction) => {
    const forward = direction >= 0;
    // A dialog laid over whatever is behind it owns the buttons while it is there, and the
    // settings page owns them where it is: either way the page underneath stays put.
    if (turnPaginatedSurface(forward ? 1 : -1)) return true;
    const view = get(readerViewAtom);
    if (view.status === "reading") {
      const reader = get(readerHandleAtom);
      if (reader === null) return false;
      reader.turnPage(forward ? "forward" : "backward");
      return true;
    }
    // The shelf is mounted while the library is anywhere, so only the shelf's own page turns it.
    if (get(destinationAtom) !== "library") return false;
    const pageShelf = get(shelfPagerAtom);
    if (pageShelf === null) return false;
    pageShelf(forward ? 1 : -1);
    return true;
  };
  return () => {
    delete window.__elephantLadderTurnPage;
  };
});

export const installBackHandlerAtom = atom(null, (get, set): (() => void) => {
  window.__elephantLadderBack = () => {
    // The reader owns its own surfaces, so it is asked first and answers for itself rather than
    // being inferred from a dialog role that an exiting surface still carries.
    if (get(readerHandleAtom)?.dismissSurface() === true) return true;
    if (
      document.querySelector(
        '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]',
      ) !== null
    ) {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
      );
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
      );
      return true;
    }
    const view = get(readerViewAtom);
    if (view.status === "reading") {
      void set(closePublicationAtom, view.session);
      return true;
    }
    if (view.status === "library" && get(destinationAtom) !== "home") {
      set(destinationAtom, "home");
      return true;
    }
    return false;
  };
  return () => {
    delete window.__elephantLadderBack;
  };
});
