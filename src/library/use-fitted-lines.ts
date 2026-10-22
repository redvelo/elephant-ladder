import { useLayoutEffect, useState } from "react";

/** Whole lines of the hero description that fit the space the rest of the copy leaves. */
export function useFittedLines(): [(node: HTMLElement | null) => void, number | null] {
  const [target, setTarget] = useState<HTMLElement | null>(null);
  const [lines, setLines] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (target === null) return undefined;
    const slot = target.parentElement;
    if (slot === null) return undefined;
    const measure = () => {
      const lineHeight = Number.parseFloat(getComputedStyle(target).lineHeight);
      const gap = Number.parseFloat(getComputedStyle(slot).rowGap) || 0;
      const taken = [...slot.children].reduce(
        (total, child) =>
          child === target ? total : total + child.getBoundingClientRect().height + gap,
        0,
      );
      const available = slot.getBoundingClientRect().height - taken;
      setLines(
        Number.isFinite(lineHeight) && lineHeight > 0
          ? Math.max(1, Math.floor((available + 0.5) / lineHeight))
          : null,
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(slot);
    return () => observer.disconnect();
  }, [target]);
  return [setTarget, lines];
}
