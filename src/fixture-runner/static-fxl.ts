import { hasExactKeys } from "./guards";
import type { PublicationRect, RunnerCheck, StaticFxlSnapshot } from "./reports";

const rectKeys = new Set(["x", "y", "width", "height", "top", "right", "bottom", "left"]);

function isPublicationRect(value: unknown): value is PublicationRect {
  return (
    hasExactKeys(value, rectKeys) &&
    [...rectKeys].every((key) => {
      const item = value[key];
      return typeof item === "number" && Number.isFinite(item) && Math.abs(item) <= 32768;
    })
  );
}

export function parseStaticFxlSnapshot(value: unknown): StaticFxlSnapshot | null {
  const ordinal = (item: unknown) =>
    typeof item === "number" && Number.isInteger(item) && item >= 0 && item <= 65535;
  const placement = (item: unknown) => item === "left" || item === "right" || item === "center";
  const participant = (item: unknown, companion: boolean) => {
    const keys = [
      "occurrence",
      "resource",
      "adapter",
      "frameGeneration",
      "layoutGeneration",
      "rect",
    ];
    if (companion) keys.push("ownerDocumentGeneration", "placement");
    return (
      hasExactKeys(item, new Set(keys)) &&
      ordinal(item.occurrence) &&
      ordinal(item.frameGeneration) &&
      ordinal(item.layoutGeneration) &&
      typeof item.resource === "string" &&
      /^EPUB\/(center\.xhtml|first\.xhtml|second\.(xhtml|svg))$/.test(item.resource) &&
      (item.adapter === "fixed-xhtml" || item.adapter === "fixed-svg") &&
      isPublicationRect(item.rect) &&
      (!companion || (ordinal(item.ownerDocumentGeneration) && placement(item.placement)))
    );
  };
  if (
    !hasExactKeys(
      value,
      new Set(["kind", "documentGeneration", "primary", "companion", "visible", "hostRect"]),
    ) ||
    value.kind !== "elephant-static-fxl-parent-v1" ||
    !ordinal(value.documentGeneration) ||
    !participant(value.primary, false) ||
    (value.companion !== null && !participant(value.companion, true)) ||
    !isPublicationRect(value.hostRect) ||
    !hasExactKeys(
      value.visible,
      new Set(["primaryOccurrence", "firstOccurrence", "lastOccurrence", "companion"]),
    ) ||
    !ordinal(value.visible.primaryOccurrence) ||
    !ordinal(value.visible.firstOccurrence) ||
    !ordinal(value.visible.lastOccurrence) ||
    (value.visible.companion !== null &&
      (!hasExactKeys(value.visible.companion, new Set(["occurrence", "placement"])) ||
        !ordinal(value.visible.companion.occurrence) ||
        !placement(value.visible.companion.placement)))
  )
    return null;
  return value as unknown as StaticFxlSnapshot;
}

export function evaluateStaticFxlSnapshot(
  snapshot: StaticFxlSnapshot,
  fixtureId: string,
  phase: 0 | 1,
): RunnerCheck[] {
  const { primary: p, companion: c, visible: v, hostRect: h } = snapshot;
  const rtl = fixtureId === "static-fxl-rtl";
  const mixed = fixtureId === "static-fxl-mixed-svg";
  const close = (a: number, b: number) => Math.abs(a - b) <= 1.5;
  const contained = (r: PublicationRect) =>
    r.width > 0 &&
    r.height > 0 &&
    r.left >= h.left - 1.5 &&
    r.right <= h.right + 1.5 &&
    r.top >= h.top - 1.5 &&
    r.bottom <= h.bottom + 1.5;
  const centered = (r: PublicationRect) => close((r.left + r.right) / 2, (h.left + h.right) / 2);
  const checks: [string, boolean][] = [
    [
      "visible",
      p.occurrence === phase &&
        v.primaryOccurrence === phase &&
        v.firstOccurrence === phase &&
        v.lastOccurrence === (phase === 0 ? 0 : 2) &&
        (phase === 0
          ? c === null && v.companion === null
          : c?.occurrence === 2 &&
            v.companion?.occurrence === 2 &&
            c.ownerDocumentGeneration === snapshot.documentGeneration &&
            c.frameGeneration !== p.frameGeneration),
    ],
    [
      "adapters",
      p.adapter === "fixed-xhtml" &&
        p.resource === (phase === 0 ? "EPUB/center.xhtml" : "EPUB/first.xhtml") &&
        (phase === 0
          ? c === null
          : c?.adapter === (mixed ? "fixed-svg" : "fixed-xhtml") &&
            c.resource === (mixed ? "EPUB/second.svg" : "EPUB/second.xhtml")),
    ],
    [
      "placement",
      phase === 0
        ? centered(p.rect)
        : c !== null &&
          c.placement === (rtl ? "left" : "right") &&
          v.companion?.placement === c.placement &&
          (rtl ? c.rect.left < p.rect.left : p.rect.left < c.rect.left),
    ],
    [
      "geometry",
      contained(p.rect) &&
        Math.abs(p.rect.width / p.rect.height - 0.75) < 0.01 &&
        (phase === 0
          ? true
          : c !== null &&
            contained(c.rect) &&
            close(p.rect.width, c.rect.width) &&
            close(p.rect.top, c.rect.top) &&
            close(p.rect.bottom, c.rect.bottom) &&
            close(rtl ? c.rect.right : p.rect.right, rtl ? p.rect.left : c.rect.left)),
    ],
  ];
  return checks.map(([label, passed]) => ({
    label: `static-fxl-${phase}-${label}`,
    passed,
    detail: JSON.stringify(snapshot),
  }));
}
