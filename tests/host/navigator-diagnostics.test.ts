import { expect, test } from "vitest";
import type { NavigatorDiagnosticEvent } from "epub-navigator/runtime";
import { createTrustedNavigatorDiagnostics } from "../../src/host/diagnostics/adapter";
import type { TrustedDiagnosticBatch } from "../../src/host/diagnostics/records";

function harness(level: "summary" | "trace" = "trace") {
  const batches: TrustedDiagnosticBatch[] = [];
  const scheduled: (() => void)[] = [];
  const adapter = createTrustedNavigatorDiagnostics({
    level,
    deliver: async (batch) => void batches.push(batch),
    schedule: (callback) => (scheduled.push(callback), scheduled.length),
    cancel: () => undefined,
  });
  return {
    adapter,
    batches,
    runScheduled() {
      scheduled.splice(0).forEach((callback) => callback());
    },
  };
}

test.each(["ready", "network"] as const)(
  "adapter deduplicates %s status, aggregates cache records, and sequences batches",
  async (readerStatus) => {
    const { adapter, batches, runScheduled } = harness();
    const render: NavigatorDiagnosticEvent = {
      type: "reader-render",
      sequence: 1,
      publicationSequence: 1,
      stateRevision: 1,
      readerStatus,
      publicationVisible: false,
      progressAccuracy: "exact",
      progressFreshness: "current",
    };
    adapter.diagnostics.record(render);
    adapter.diagnostics.record({ ...render, sequence: 2 });
    adapter.diagnostics.record({
      type: "cache",
      action: "read",
      recordType: "page",
      occurrence: 1,
      bytes: 10,
      durationMs: 2,
      outcome: "hit",
    });
    adapter.diagnostics.record({
      type: "cache",
      action: "read",
      recordType: "page",
      occurrence: 2,
      bytes: 12,
      durationMs: 3,
      outcome: "hit",
    });
    runScheduled();
    await adapter.flush();
    expect(batches).toStrictEqual([
      {
        version: 1,
        firstSequence: 1,
        records: [
          { event: "readerStatus", status: readerStatus },
          {
            event: "cacheAggregate",
            action: "read",
            recordType: "page",
            outcome: "hit",
            count: 2,
            bytes: 22,
            durationMs: 5,
          },
        ],
      },
    ]);
  },
);

test("adapter records first-visible when the publication is revealed and resets that state", async () => {
  const { adapter, batches, runScheduled } = harness("summary");
  const render: NavigatorDiagnosticEvent = {
    type: "reader-render",
    sequence: 1,
    publicationSequence: 1,
    stateRevision: 1,
    readerStatus: "loading",
    publicationVisible: false,
    progressAccuracy: "estimated",
    progressFreshness: "current",
  };
  adapter.diagnostics.record(render);
  adapter.diagnostics.record({ ...render, sequence: 2, publicationVisible: true });
  adapter.diagnostics.record({ ...render, sequence: 3, publicationVisible: true });
  runScheduled();
  await adapter.flush();
  adapter.reset();
  adapter.diagnostics.record({ ...render, sequence: 4, publicationVisible: true });
  runScheduled();
  await adapter.flush();
  expect(batches.map(({ firstSequence, records }) => [firstSequence, records])).toStrictEqual([
    [
      1,
      [
        { event: "readerStatus", status: "loading" },
        { event: "openMilestone", milestone: "first-visible" },
      ],
    ],
    [
      1,
      [
        { event: "openMilestone", milestone: "first-visible" },
        { event: "readerStatus", status: "loading" },
      ],
    ],
  ]);
});

test("summary level is passed through and trace-only unbounded phase names are dropped", async () => {
  const { adapter, batches, runScheduled } = harness("summary");
  expect(adapter.diagnostics.level).toBe("summary");
  adapter.diagnostics.record({
    type: "layout-phase",
    role: "foreground",
    frameGeneration: 1,
    operation: 1,
    phase: "authored path must not pass",
    durationMs: 1,
  });
  runScheduled();
  await adapter.flush();
  expect(batches).toStrictEqual([]);
});

test("adapter preserves source-validated Reader Serif evidence at summary level", async () => {
  const { adapter, batches, runScheduled } = harness("summary");
  adapter.diagnostics.record({
    type: "font-evidence",
    target: "publication",
    family: "navigator-reader-serif",
    frameGeneration: 7,
    configured: true,
    declared: true,
    loaded: true,
    check: true,
  });
  runScheduled();
  await adapter.flush();
  expect(batches[0]?.records).toStrictEqual([
    {
      event: "fontEvidence",
      target: "publication",
      family: "navigator-reader-serif",
      frame: 7,
      configured: true,
      declared: true,
      loaded: true,
      check: true,
    },
  ]);
});

test("closing abandons a delivery that is still on the wire", async () => {
  const signals: AbortSignal[] = [];
  let release = (): void => undefined;
  const scheduled: (() => void)[] = [];
  const adapter = createTrustedNavigatorDiagnostics({
    level: "trace",
    deliver: async (_batch, signal) => {
      signals.push(signal);
      await new Promise<void>((resolve) => (release = resolve));
    },
    schedule: (callback) => (scheduled.push(callback), scheduled.length),
    cancel: () => undefined,
  });

  adapter.recordReadingPosition("write-failed", null);
  scheduled.splice(0).forEach((callback) => callback());
  // Delivery is chained onto a promise, so it starts a microtask after the flush.
  await Promise.resolve();
  expect(signals).toHaveLength(1);
  expect(signals[0]?.aborted).toBe(false);

  adapter.close();
  expect(signals[0]?.aborted, "the reader session is gone, so its record is dropped").toBe(true);
  release();
});
