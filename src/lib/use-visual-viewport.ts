import { useEffect } from "react";

/**
 * Publishes the visual viewport's height to the element, so a page can size itself to what is
 * actually on screen while an on-screen keyboard is up.
 */
export function useVisualViewportHeight(shell: HTMLElement | null): void {
  useEffect(() => {
    const viewport = window.visualViewport;
    if (shell === null || !viewport) return;
    const update = () => {
      if (viewport.scale === 1) {
        shell.style.setProperty(
          "--app-viewport-height",
          `${viewport.height + viewport.offsetTop}px`,
        );
      }
    };
    update();
    viewport.addEventListener("resize", update);
    viewport.addEventListener("scroll", update);
    return () => {
      viewport.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
    };
  }, [shell]);
}
