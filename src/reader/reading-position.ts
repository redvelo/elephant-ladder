import type { NavigatorLocationSnapshot } from "epub-navigator";
import type {
  ReadingPositionCaptureReason,
  ReadingPositionOutcome,
} from "../host/diagnostics/records";
import type {
  NavigatorReadingPosition,
  NavigatorReadingPositionCaptureResult,
} from "epub-navigator/react";

export interface ReadingPositionTimerHost {
  clearTimeout(timer: number): void;
  setTimeout(callback: () => void, delay: number): number;
}

export function replaceReadingPositionTimer(
  host: ReadingPositionTimerHost,
  current: number | null,
  snapshot: NavigatorLocationSnapshot,
  capture: () => void,
): number | null {
  if (current !== null) host.clearTimeout(current);
  if (snapshot.location?.kind !== "point" || snapshot.basis === null) return null;
  return host.setTimeout(capture, 500);
}

export function readingPositionKey(position: NavigatorReadingPosition): string {
  return JSON.stringify(position);
}

export function copyReadingPosition(position: NavigatorReadingPosition): NavigatorReadingPosition {
  return {
    targetJson: position.targetJson,
    occurrenceHint: position.occurrenceHint,
    ...(position.rollProgression === undefined
      ? {}
      : { rollProgression: position.rollProgression }),
  };
}

export function isReadingPosition(value: unknown): value is NavigatorReadingPosition {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const position = value as Record<string, unknown>;
  if (
    Object.keys(position).some(
      (key) => key !== "targetJson" && key !== "occurrenceHint" && key !== "rollProgression",
    ) ||
    typeof position.targetJson !== "string" ||
    ("rollProgression" in position &&
      (typeof position.rollProgression !== "number" ||
        !Number.isFinite(position.rollProgression) ||
        position.rollProgression < 0 ||
        position.rollProgression > 1))
  ) {
    return false;
  }
  const hint = position.occurrenceHint;
  if (hint === null) return true;
  if (typeof hint !== "object" || hint === null || Array.isArray(hint)) return false;
  const occurrenceHint = hint as Record<string, unknown>;
  return (
    Object.keys(occurrenceHint).every((key) => key === "occurrence" || key === "resource") &&
    Number.isSafeInteger(occurrenceHint.occurrence) &&
    (occurrenceHint.occurrence as number) >= 0 &&
    typeof occurrenceHint.resource === "string"
  );
}

export function readingPositionCaptureDiagnostic(
  result: Exclude<NavigatorReadingPositionCaptureResult, { readonly status: "generated" }>,
): {
  readonly outcome: ReadingPositionOutcome;
  readonly reason: ReadingPositionCaptureReason | null;
} {
  if (result.status === "unavailable") return { outcome: "unavailable", reason: result.reason };
  return { outcome: result.status, reason: null };
}
