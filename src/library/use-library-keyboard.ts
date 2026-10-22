import { useSetAtom } from "jotai";
import { useEffect, useRef } from "react";
import { destinationAtom, type Destination } from "../app/destination";
import { showShortcutsAtom } from "./dispatch";

/** The library's own keys. The reader has a keymap of its own, so these stay out of it. */
export function useLibraryKeyboard({
  active,
  busy,
  destination,
  shell,
  onImport,
}: {
  readonly active: boolean;
  readonly busy: boolean;
  readonly destination: Destination;
  readonly shell: HTMLElement | null;
  readonly onImport: () => void;
}): void {
  const setDestination = useSetAtom(destinationAtom);
  const showShortcuts = useSetAtom(showShortcutsAtom);
  const searchRequested = useRef(false);
  useEffect(() => {
    if (searchRequested.current && destination === "library") {
      shell?.querySelector<HTMLInputElement>('input[type="search"]')?.focus();
      searchRequested.current = false;
    }
  });
  useEffect(() => {
    if (!active) return undefined;
    const handleKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.isComposing ||
        event.altKey ||
        document.querySelector(
          '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]',
        ) !== null
      )
        return;
      const target = event.target;
      if (
        target instanceof Element &&
        target.closest('input, textarea, select, [contenteditable="true"]')
      )
        return;
      if (event.ctrlKey || event.metaKey) {
        if (event.shiftKey) return;
        if (event.key === "1") setDestination("home");
        else if (event.key === "2") setDestination("library");
        else if (event.key === ",") setDestination("settings");
        else if (event.key.toLowerCase() === "o" && !busy) onImport();
        else return;
      } else if (event.key === "/") {
        searchRequested.current = true;
        setDestination("library");
        shell?.querySelector<HTMLInputElement>('input[type="search"]')?.focus();
      } else if (event.key === "?") {
        showShortcuts(
          document.activeElement instanceof HTMLElement ? document.activeElement : null,
        );
      } else return;
      event.preventDefault();
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [active, busy, onImport, setDestination, shell, showShortcuts]);
}
