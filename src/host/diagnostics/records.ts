import type { NavigatorDiagnostics, PaginationCancellationReason } from "epub-navigator/runtime";

export type TrustedDiagnosticRecord =
  | {
      readonly event: "fontEvidence";
      readonly target: "interface" | "publication";
      readonly family: "inter-variable" | "navigator-reader-serif";
      readonly frame: number | null;
      readonly configured: boolean;
      readonly declared: boolean;
      readonly loaded: boolean;
      readonly check: boolean;
    }
  | {
      readonly event: "readerStatus";
      readonly status: "ready" | "network" | "loading" | "issue";
    }
  | {
      readonly event: "paginationRun";
      readonly action: "start" | "terminal";
      readonly run: number;
      readonly outcome: "running" | "complete" | "unavailable" | "superseded" | "cancelled";
      readonly durationMs: number;
      readonly measuredOccurrences: number;
      readonly totalOccurrences: number;
      readonly measuredPageCount: number;
      readonly totalPageCount: number | null;
      readonly measuredTargets: number;
      readonly totalTargets: number;
      readonly pageCountAttempts: number;
      readonly timedOutOccurrences: number;
      readonly measurementMs: number;
    }
  | {
      readonly event: "cacheRestore";
      readonly action: "start" | "complete" | "stalled" | "cancelled";
      readonly run: number;
      readonly claimedOccurrences: number;
      readonly remainingOccurrences: number;
      readonly durationMs: number;
    }
  | {
      readonly event: "cacheWrite";
      readonly action: "start" | "complete" | "cancelled" | "failed";
      readonly operation: number;
      readonly run: number;
      readonly occurrenceCount: number;
      readonly operationCount: number;
      readonly completedOperationCount: number;
      readonly durationMs: number;
    }
  | {
      readonly event: "cacheDrain";
      readonly run: number;
      readonly outcome: "durable" | "failed" | "cancelled";
      readonly durationMs: number;
    }
  | {
      readonly event: "frameLifecycle";
      readonly role: "foreground" | "auxiliary" | "prepared";
      readonly occurrence: number;
      readonly frame: number;
      readonly outcome: "committed" | "prepared" | "failed" | "cancelled";
      readonly reason:
        | "complete"
        | "load-error"
        | "unexpected-navigation"
        | "effect-execution-failed"
        | "readiness-timeout"
        | "layout-conflict"
        | "layout-unavailable"
        | "superseded"
        | "closed";
      readonly createToTerminalMs: number;
      readonly createToLoadMs?: number;
      readonly loadToAgentReadyMs?: number;
      readonly agentReadyToCommitMs?: number;
    }
  | {
      readonly event: "layout";
      readonly action: "start" | "settle" | "cancel" | "discard";
      readonly role: "foreground" | "auxiliary";
      readonly document: number;
      readonly layout: number;
      readonly operation: number;
      readonly phase: "initial" | "update";
      readonly durationMs?: number;
    }
  | {
      readonly event: "layoutPhase";
      readonly role: "foreground" | "auxiliary" | "prepared";
      readonly frame: number;
      readonly operation: number | null;
      readonly phase:
        | "capture"
        | "content-end-scan"
        | "text-assessment"
        | "settlement"
        | "observation"
        | "live-observation";
      readonly durationMs: number;
      readonly scanCount?: number;
      readonly rectangleCount?: number;
    }
  | {
      readonly event: "controllerTask";
      readonly action: "schedule" | "start" | "pause" | "requeue" | "complete" | "discard";
      readonly task: "page-count" | "toc";
      readonly occurrence: number;
      readonly run: number;
      readonly taskGeneration: number;
      readonly document?: number;
      readonly layout?: number;
      readonly reason?: PaginationCancellationReason;
    }
  | {
      readonly event: "cacheAggregate";
      readonly action: "read" | "write" | "remove";
      readonly recordType: "page" | "toc-target" | "unknown";
      readonly outcome: "hit" | "miss" | "complete" | "cancelled" | "failed" | "invalid";
      readonly count: number;
      readonly bytes: number;
      readonly durationMs: number;
    }
  | { readonly event: "openMilestone"; readonly milestone: "first-visible" }
  | {
      readonly event: "readingPosition";
      readonly outcome: ReadingPositionOutcome;
      readonly reason: ReadingPositionCaptureReason | null;
    };

export type ReadingPositionOutcome =
  | "unavailable"
  | "unsupported"
  | "ambiguous"
  | "conflict"
  | "stale"
  | "failed"
  | "cancelled"
  | "write-failed";

export type ReadingPositionCaptureReason =
  | "runtime-unavailable"
  | "current-location-unavailable"
  | "not-authored"
  | "no-exact-selector"
  | "no-exact-movement"
  | "decoration-not-rendered";

export interface TrustedDiagnosticBatch {
  readonly version: 1;
  readonly firstSequence: number;
  readonly records: readonly TrustedDiagnosticRecord[];
}

export interface TrustedNavigatorDiagnostics {
  readonly diagnostics: NavigatorDiagnostics;
  recordInterfaceFontEvidence(): Promise<void>;
  recordReadingPosition(
    outcome: ReadingPositionOutcome,
    reason: ReadingPositionCaptureReason | null,
  ): void;
  reset(): void;
  flush(): Promise<void>;
  close(): void;
}
