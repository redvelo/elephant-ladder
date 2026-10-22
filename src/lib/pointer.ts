import { useEffect, useState } from "react";

const coarsePointer = "(pointer: coarse)";

function touchPrimary(): boolean {
  return window.matchMedia?.(coarsePointer).matches ?? false;
}

/** True where touch is the primary input, which is what selects the mobile shape of a surface. */
export function useCoarsePointer(): boolean {
  const [coarse, setCoarse] = useState(touchPrimary);

  useEffect(() => {
    const media = window.matchMedia?.(coarsePointer);
    if (media === undefined) return;
    const update = () => setCoarse(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return coarse;
}

const anyHover = "(any-hover: hover)";

function hoverAvailable(): boolean {
  return window.matchMedia?.(anyHover).matches ?? false;
}

/**
 * True where some attached input can hover, which is a mouse or a trackpad rather than a finger.
 * A pointing device of that kind arrives with a keyboard, so this is what makes a keyboard
 * setting worth showing. Pointer precision is not: the BOOX Go6 has neither a mouse nor a
 * keyboard and still answers `any-pointer: fine` yes, while answering hover no.
 */
export function useHoverPointer(): boolean {
  const [hover, setHover] = useState(hoverAvailable);

  useEffect(() => {
    const media = window.matchMedia?.(anyHover);
    if (media === undefined) return;
    const update = () => setHover(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return hover;
}
