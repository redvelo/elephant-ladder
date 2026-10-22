import type { NavigatorDiagnosticEvent } from "epub-navigator/runtime";
import type { TrustedDiagnosticRecord } from "./records";

export const maximumQueuedRecords = 64;
export const maximumBatchRecords = 16;
const maximumCount = 1_000_000;
const maximumDurationMs = 3_600_000;
const layoutPhases = new Set([
  "capture",
  "content-end-scan",
  "text-assessment",
  "settlement",
  "observation",
  "live-observation",
]);

export function normalize(event: NavigatorDiagnosticEvent): TrustedDiagnosticRecord | null {
  switch (event.type) {
    case "reader-render":
      return { event: "readerStatus", status: event.readerStatus };
    case "font-evidence":
      return {
        event: "fontEvidence",
        target: event.target,
        family: event.family,
        frame: bounded(event.frameGeneration),
        configured: event.configured,
        declared: event.declared,
        loaded: event.loaded,
        check: event.check,
      };
    case "pagination-run":
      return {
        event: "paginationRun",
        action: event.action,
        run: bounded(event.runRevision),
        outcome: event.outcome,
        durationMs: boundedDuration(event.durationMs),
        measuredOccurrences: bounded(event.measuredOccurrences),
        totalOccurrences: bounded(event.totalOccurrences),
        measuredPageCount: bounded(event.measuredPageCount),
        totalPageCount: event.totalPageCount === null ? null : bounded(event.totalPageCount),
        measuredTargets: bounded(event.measuredTargets),
        totalTargets: bounded(event.totalTargets),
        pageCountAttempts: bounded(event.pageCountAttempts),
        timedOutOccurrences: bounded(event.timedOutOccurrences),
        measurementMs: boundedDuration(event.measurementMs),
      };
    case "cache-restore":
      return {
        event: "cacheRestore",
        action: event.action,
        run: bounded(event.runRevision),
        claimedOccurrences: bounded(event.claimedOccurrences),
        remainingOccurrences: bounded(event.remainingOccurrences),
        durationMs: boundedDuration(event.durationMs),
      };
    case "cache-write":
      return {
        event: "cacheWrite",
        action: event.action,
        operation: bounded(event.operationId),
        run: bounded(event.runRevision),
        occurrenceCount: bounded(event.occurrenceCount),
        operationCount: bounded(event.operationCount),
        completedOperationCount: bounded(event.completedOperationCount),
        durationMs: boundedDuration(event.durationMs),
      };
    case "cache-drain":
      return {
        event: "cacheDrain",
        run: bounded(event.runRevision),
        outcome: event.outcome,
        durationMs: boundedDuration(event.durationMs),
      };
    case "frame-lifecycle":
      return {
        event: "frameLifecycle",
        role: event.role,
        occurrence: bounded(event.occurrence),
        frame: bounded(event.frameGeneration),
        outcome: event.outcome,
        reason: event.reason,
        createToTerminalMs: boundedDuration(event.createToTerminalMs),
        ...optionalDuration("createToLoadMs", event.createToLoadMs),
        ...optionalDuration("loadToAgentReadyMs", event.loadToAgentReadyMs),
        ...optionalDuration("agentReadyToCommitMs", event.agentReadyToCommitMs),
      };
    case "layout":
      return {
        event: "layout",
        action: event.action,
        role: event.role,
        document: bounded(event.documentGeneration),
        layout: bounded(event.layoutGeneration),
        operation: bounded(event.operation),
        phase: event.phase,
        ...optionalDuration("durationMs", event.durationMs),
      };
    case "layout-phase":
      if (!layoutPhases.has(event.phase)) return null;
      return {
        event: "layoutPhase",
        role: event.role,
        frame: bounded(event.frameGeneration),
        operation: event.operation === null ? null : bounded(event.operation),
        phase: event.phase as Extract<TrustedDiagnosticRecord, { event: "layoutPhase" }>["phase"],
        durationMs: boundedDuration(event.durationMs),
        ...optionalCount("scanCount", event.scanCount),
        ...optionalCount("rectangleCount", event.rectangleCount),
      };
    case "controller-task":
      return {
        event: "controllerTask",
        action: event.action,
        task: event.task,
        occurrence: bounded(event.occurrence),
        run: bounded(event.runRevision),
        taskGeneration: bounded(event.taskGeneration),
        ...optionalCount("document", event.documentGeneration),
        ...optionalCount("layout", event.layoutGeneration),
        ...(event.reason === undefined ? {} : { reason: event.reason }),
      };
    default:
      return null;
  }
}

export async function inspectFont(
  owner: Document,
  element: Element | null,
  family: string,
  specification: string,
  sample: string,
): Promise<{
  configured: boolean;
  declared: boolean;
  loaded: boolean;
  check: boolean;
}> {
  const normalizeFamily = (value: string): string => value.trim().replace(/^(["'])(.*)\1$/, "$2");
  const configured =
    element !== null &&
    normalizeFamily(
      owner.defaultView?.getComputedStyle(element).fontFamily.split(",", 1)[0] ?? "",
    ) === family;
  const matchingFaces = (): FontFace[] =>
    Array.from(owner.fonts).filter((face) => normalizeFamily(face.family) === family);
  const declared = matchingFaces().length > 0;
  let loaded = false;
  let check = false;
  try {
    const faces = await owner.fonts.load(specification, sample);
    loaded =
      faces.some((face) => normalizeFamily(face.family) === family) &&
      faces
        .filter((face) => normalizeFamily(face.family) === family)
        .every((face) => face.status === "loaded");
    check = owner.fonts.check(specification, sample);
  } catch {
    // Evidence records failures rather than allowing diagnostics to affect the reader.
  }
  return { configured, declared, loaded, check };
}

export function bounded(value: number): number {
  return Number.isSafeInteger(value) && value >= 0 ? Math.min(value, maximumCount) : 0;
}

export function boundedDuration(value: number): number {
  return Number.isFinite(value) && value >= 0 ? Math.min(value, maximumDurationMs) : 0;
}

function optionalCount<Key extends string>(
  key: Key,
  value: number | undefined,
): Partial<Record<Key, number>> {
  return value === undefined ? {} : ({ [key]: bounded(value) } as Record<Key, number>);
}

function optionalDuration<Key extends string>(
  key: Key,
  value: number | undefined,
): Partial<Record<Key, number>> {
  return value === undefined ? {} : ({ [key]: boundedDuration(value) } as Record<Key, number>);
}
