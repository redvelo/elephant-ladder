import type { KernelEvent, KernelTransition } from "epub-navigator";
import type { NavigatorRuntime } from "epub-navigator/runtime";
import { isRecognizedRunnerMessage } from "./contracts";
import { openedGeneration, requirePendingFrame, waitForCommit } from "./frames";
import { isRecord } from "./guards";
import {
  createProbeGeneration,
  generationComplete,
  missingProbeScenarios,
  parseProbeReport,
  reduceProbeGeneration,
} from "./probe";
import type { ProbeGeneration, SingleReport } from "./reports";
import type { ProbeSource } from "./scenarios";

const MAXIMUM_REJECTIONS = 8;

export interface RunnerChannel {
  /** Binds the conversation to the frame Navigator has prepared but not yet promoted. */
  registerFrame(container: HTMLElement): void;
  collectProbeGeneration(
    number: 1 | 2 | 3,
    source: ProbeSource,
    transition: KernelTransition,
    runtime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
  ): Promise<ProbeGeneration>;
  collectSingleReport<T extends SingleReport>(
    runtime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
    contract: { readonly reportType: string; readonly parse: (value: unknown) => T | null },
    readingOrderIndex: number,
  ): Promise<T>;
  signalWorkerLifecycleObservation(): void;
  /** Origins the publication's agent spoke from. */
  readonly agentMessageOrigins: ReadonlySet<string>;
}

/**
 * The conversation with the publication frame: which window may speak, from which origin, and
 * who is waiting for what it says. A report that arrives from anywhere else is counted as a
 * rejection rather than read, and the count is reported when a wait times out.
 */
export function createRunnerChannel(publicationOrigin: string, signal: AbortSignal): RunnerChannel {
  let registeredWindow: Window | null = null;
  let receiveContractReport: ((value: unknown) => void) | null = null;
  const rejectedContractMessages: string[] = [];
  const agentMessageOrigins = new Set<string>();

  const receiveMessage = (event: MessageEvent): void => {
    const recognized = isRecognizedRunnerMessage(event.data);
    if (
      registeredWindow !== null &&
      event.source === registeredWindow &&
      isRecord(event.data) &&
      event.data.namespace === "epub-navigator"
    ) {
      agentMessageOrigins.add(event.origin);
    }
    if (registeredWindow === null || event.source !== registeredWindow) {
      if (recognized && rejectedContractMessages.length < MAXIMUM_REJECTIONS)
        rejectedContractMessages.push(`window:${String(event.data.source)}`);
      return;
    }
    if (event.origin !== publicationOrigin) {
      if (recognized && rejectedContractMessages.length < MAXIMUM_REJECTIONS)
        rejectedContractMessages.push(`origin:${event.origin}`);
      return;
    }
    receiveContractReport?.(event.data);
  };
  window.addEventListener("message", receiveMessage, { signal });
  signal.addEventListener(
    "abort",
    () => {
      registeredWindow = null;
      receiveContractReport = null;
    },
    { once: true },
  );

  function registerFrame(container: HTMLElement): void {
    const frame = requirePendingFrame(container);
    if (frame.contentWindow === null) throw new Error("Publication frame has no window");
    registeredWindow = frame.contentWindow;
  }

  function signalWorkerLifecycleObservation(): void {
    const target = registeredWindow;
    if (target === null) return;
    let remaining = 20;
    const send = () => {
      if (signal.aborted || remaining-- === 0) return;
      target.postMessage({ type: "elephant-worker-lifecycle-observe-v1" }, publicationOrigin);
      window.setTimeout(send, 100);
    };
    send();
  }

  async function collectProbeGeneration(
    number: 1 | 2 | 3,
    source: ProbeSource,
    transition: KernelTransition,
    runtime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
  ): Promise<ProbeGeneration> {
    let collecting = createProbeGeneration(number, source);
    let complete: ((generation: ProbeGeneration) => void) | null = null;
    let failed: ((cause: Error) => void) | null = null;
    receiveContractReport = (value) => {
      const report = parseProbeReport(value);
      if (report === null || report.source !== source) return;
      const next = reduceProbeGeneration(collecting, report);
      if (next === null) {
        failed?.(new Error(`Duplicate terminal report for ${report.scenario}`));
        return;
      }
      collecting = next;
      if (generationComplete(next)) complete?.(next);
    };
    registerFrame(container);
    await waitForCommit(runtime, openedGeneration(transition), runtimeErrors, runtimeEvents);
    signalWorkerLifecycleObservation();
    return new Promise<ProbeGeneration>((resolve, reject) => {
      const timeout = window.setTimeout(() => {
        const diagnostics =
          rejectedContractMessages.length === 0 ? "none" : rejectedContractMessages.join(", ");
        rejectGeneration(
          new Error(
            `Probe phase ${source} timed out; missing ${missingProbeScenarios(collecting).join(", ")}; rejected ${diagnostics}`,
          ),
        );
      }, 15_000);
      const cleanup = () => {
        window.clearTimeout(timeout);
        signal.removeEventListener("abort", abandon);
        complete = null;
        failed = null;
        receiveContractReport = null;
      };
      const rejectGeneration = (cause: Error) => {
        cleanup();
        reject(cause);
      };
      function abandon(): void {
        rejectGeneration(new Error(`Probe phase ${source} abandoned`));
      }
      signal.addEventListener("abort", abandon, { once: true });
      failed = rejectGeneration;
      complete = (generation) => {
        cleanup();
        resolve(generation);
      };
      if (generationComplete(collecting)) complete(collecting);
    });
  }

  async function collectSingleReport<T extends SingleReport>(
    runtime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
    contract: { readonly reportType: string; readonly parse: (value: unknown) => T | null },
    readingOrderIndex: number,
  ): Promise<T> {
    let report: T | null = null;
    let complete: ((value: T) => void) | null = null;
    let failed: ((cause: Error) => void) | null = null;
    receiveContractReport = (value) => {
      const parsed = contract.parse(value);
      if (parsed === null) return;
      if (report !== null) {
        failed?.(new Error(`Duplicate ${contract.reportType} report`));
        return;
      }
      report = parsed;
      complete?.(parsed);
    };
    const transition = runtime.open(readingOrderIndex);
    registerFrame(container);
    await waitForCommit(runtime, openedGeneration(transition), runtimeErrors, runtimeEvents);
    return new Promise<T>((resolve, reject) => {
      const timeout = window.setTimeout(
        () => rejectReport(new Error(`${contract.reportType} observer timed out`)),
        10_000,
      );
      const cleanup = () => {
        window.clearTimeout(timeout);
        signal.removeEventListener("abort", abandon);
        complete = null;
        failed = null;
        receiveContractReport = null;
      };
      const rejectReport = (cause: Error) => {
        cleanup();
        reject(cause);
      };
      function abandon(): void {
        rejectReport(new Error(`${contract.reportType} observer abandoned`));
      }
      signal.addEventListener("abort", abandon, { once: true });
      failed = rejectReport;
      complete = (value) => {
        cleanup();
        resolve(value);
      };
      if (report !== null) complete(report);
    });
  }

  return {
    registerFrame,
    collectProbeGeneration,
    collectSingleReport,
    signalWorkerLifecycleObservation,
    agentMessageOrigins,
  };
}
