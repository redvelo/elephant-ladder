import type { NavigatorDiagnosticEvent, NavigatorDiagnostics } from "epub-navigator/runtime";
import {
  bounded,
  boundedDuration,
  inspectFont,
  maximumBatchRecords,
  maximumQueuedRecords,
  normalize,
} from "./normalize";
import type {
  ReadingPositionCaptureReason,
  ReadingPositionOutcome,
  TrustedDiagnosticBatch,
  TrustedDiagnosticRecord,
  TrustedNavigatorDiagnostics,
} from "./records";

interface AdapterOptions {
  readonly level: "summary" | "trace";
  readonly deliver: (batch: TrustedDiagnosticBatch, signal: AbortSignal) => Promise<void>;
  readonly schedule?: (callback: () => void) => number;
  readonly cancel?: (handle: number) => void;
}

export function createTrustedNavigatorDiagnostics(
  options: AdapterOptions,
): TrustedNavigatorDiagnostics {
  let sequence = 1;
  let queue: TrustedDiagnosticRecord[] = [];
  let timer: number | null = null;
  let delivery = Promise.resolve();
  let transfer = new AbortController();
  let closed = false;
  let lastReaderStatus:
    | Extract<TrustedDiagnosticRecord, { event: "readerStatus" }>["status"]
    | null = null;
  let milestoneRecorded = false;
  const cache = new Map<string, Extract<TrustedDiagnosticRecord, { event: "cacheAggregate" }>>();
  const schedule = options.schedule ?? ((callback) => window.setTimeout(callback, 100));
  const cancel = options.cancel ?? ((handle) => window.clearTimeout(handle));

  const diagnostics: NavigatorDiagnostics = Object.freeze({
    level: options.level,
    record(event: NavigatorDiagnosticEvent) {
      if (closed) return;
      if (
        options.level === "summary" &&
        ["frame-lifecycle", "layout", "layout-phase", "controller-task", "cache"].includes(
          event.type,
        )
      ) {
        return;
      }
      if (event.type === "reader-render") {
        const statusChanged = event.readerStatus !== lastReaderStatus;
        lastReaderStatus = event.readerStatus;
        if (event.publicationVisible) recordMilestone();
        if (!statusChanged) return;
      }
      if (event.type === "cache") {
        aggregateCache(event);
        requestDelivery();
        return;
      }
      const record = normalize(event);
      if (record !== null) enqueue(record);
    },
  });

  return Object.freeze({
    diagnostics,
    async recordInterfaceFontEvidence() {
      const evidence = await inspectFont(
        document,
        document.querySelector(".elephant-app"),
        "Inter Variable",
        '400 16px "Inter Variable"',
        "Elephant Ladder",
      );
      if (closed) return;
      enqueue({
        event: "fontEvidence",
        target: "interface",
        family: "inter-variable",
        frame: null,
        ...evidence,
      });
    },
    reset() {
      if (timer !== null) cancel(timer);
      timer = null;
      sequence = 1;
      queue = [];
      cache.clear();
      lastReaderStatus = null;
      milestoneRecorded = false;
      closed = false;
      abandonDeliveries();
    },
    async flush() {
      flushCache();
      dispatch();
      await delivery;
    },
    recordReadingPosition(
      outcome: ReadingPositionOutcome,
      reason: ReadingPositionCaptureReason | null,
    ) {
      if (closed) return;
      enqueue({ event: "readingPosition", outcome, reason });
    },
    close() {
      if (timer !== null) cancel(timer);
      timer = null;
      closed = true;
      queue = [];
      cache.clear();
      abandonDeliveries();
    },
  });

  function recordMilestone(): void {
    if (closed || milestoneRecorded) return;
    milestoneRecorded = true;
    enqueue({ event: "openMilestone", milestone: "first-visible" });
  }

  /**
   * Drops whatever is still on the wire.
   *
   * Records are numbered in one sequence per reader session, so a batch that arrives after the
   * session ends, or after the sequence restarts, is one the host can only refuse.
   */
  function abandonDeliveries(): void {
    transfer.abort();
    transfer = new AbortController();
  }

  function enqueue(record: TrustedDiagnosticRecord): void {
    if (queue.length === maximumQueuedRecords) queue.shift();
    queue.push(record);
    requestDelivery();
  }

  function requestDelivery(): void {
    if (timer !== null) return;
    timer = schedule(() => {
      timer = null;
      flushCache();
      dispatch();
    });
  }

  function flushCache(): void {
    for (const record of cache.values()) {
      if (queue.length === maximumQueuedRecords) queue.shift();
      queue.push(record);
    }
    cache.clear();
  }

  function dispatch(): void {
    const { signal } = transfer;
    while (queue.length > 0) {
      const records = queue.splice(0, maximumBatchRecords);
      const batch = { version: 1 as const, firstSequence: sequence, records };
      sequence += records.length;
      delivery = delivery.then(() => options.deliver(batch, signal)).catch(() => undefined);
    }
  }

  function aggregateCache(event: Extract<NavigatorDiagnosticEvent, { type: "cache" }>): void {
    const recordType = event.recordType ?? "unknown";
    const key = `${event.action}:${recordType}:${event.outcome}`;
    const current = cache.get(key);
    cache.set(key, {
      event: "cacheAggregate",
      action: event.action,
      recordType,
      outcome: event.outcome,
      count: bounded((current?.count ?? 0) + 1),
      bytes: bounded((current?.bytes ?? 0) + event.bytes),
      durationMs: boundedDuration((current?.durationMs ?? 0) + event.durationMs),
    });
  }
}
