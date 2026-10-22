import type { KernelEvent, KernelSnapshot, KernelTransition } from "epub-navigator";
import type { DocumentObservationRequest, NavigatorRuntime } from "epub-navigator/runtime";
import type { ReadingBootstrap } from "../host/bootstrap/types";
import { openedGeneration, waitForCommit, waitForPaint } from "./frames";
import { isRecord } from "./guards";

const MAX_DOCUMENTS = 64;
const MAX_EVENTS_PER_DOCUMENT = 32;
const SETTLE_TIMEOUT_MS = 10_000;
// A synthetic spread's companion is staged after the primary settles, so a settled
// primary is only evidence once the view has stopped changing for a while.
const QUIET_MS = 600;

export interface DocumentEvidence {
  readonly occurrence: number;
  readonly facts: KernelSnapshot["readingOrder"][number] | null;
  readonly open: {
    readonly accepted: boolean;
    readonly failure: KernelTransition["failure"];
    readonly error?: string;
  };
  readonly committed: KernelSnapshot["document"]["committed"];
  readonly fixedCompanion: KernelSnapshot["document"]["fixedCompanion"];
  readonly visible: ReturnType<NavigatorRuntime["visibleOccurrenceRange"]>;
  readonly settled: boolean;
  readonly observation: Awaited<ReturnType<NavigatorRuntime["observeDocument"]>> | null;
  readonly observationError: string | null;
  readonly errors: readonly string[];
  readonly events: readonly KernelEvent[];
  readonly elapsedMs: number;
}

export interface EvidenceSubmission {
  readonly runner: {
    readonly viewport: { readonly width: number; readonly height: number };
    readonly frame: {
      readonly width: number;
      readonly height: number;
      readonly landscape: boolean;
    };
    readonly devicePixelRatio: number;
    readonly userAgent: string;
    readonly startedAt: string;
    readonly finishedAt: string;
  };
  readonly publication: {
    readonly facts: ReadingBootstrap["publication"]["facts"];
    readonly mount: ReadingBootstrap["publication"]["mount"];
    readonly kernel: Pick<
      KernelSnapshot,
      "packagePath" | "publicationLayout" | "readingOrderLength"
    >;
    readonly policy: ReadingBootstrap["policy"];
    readonly mediaOverlay: ReadingBootstrap["mediaOverlay"];
  };
  readonly documents: readonly DocumentEvidence[];
}

/** The observation every document is asked for when the run supplied no plan of its own. */
export const DEFAULT_OBSERVATION: DocumentObservationRequest = {
  queries: [
    { id: "body", selector: "body", text: true, styles: ["direction", "writing-mode"] },
    { id: "title", selector: "title", text: true },
  ],
  resources: true,
  fonts: true,
};

export function isObservationPlan(value: unknown): value is DocumentObservationRequest {
  return (
    isRecord(value) &&
    Array.isArray(value.queries) &&
    (value.resources === undefined || typeof value.resources === "boolean") &&
    (value.fonts === undefined || typeof value.fonts === "boolean")
  );
}

/**
 * Walks the reading order once. Every document is opened through Navigator, given
 * until its layout settles, and asked for the same observation; what each step
 * reported is recorded as it was, including the steps that did not happen.
 */
export async function collectEvidence(
  runtime: NavigatorRuntime,
  container: HTMLElement,
  bootstrap: ReadingBootstrap,
  observation: DocumentObservationRequest,
  runtimeErrors: readonly string[],
  runtimeEvents: readonly KernelEvent[],
  signal: AbortSignal,
): Promise<EvidenceSubmission> {
  const startedAt = new Date().toISOString();
  const initial = runtime.snapshot();
  const documents: DocumentEvidence[] = [];
  const count = Math.min(initial.readingOrderLength, MAX_DOCUMENTS);
  for (let occurrence = 0; occurrence < count; occurrence += 1) {
    if (signal.aborted) break;
    documents.push(
      await observeOccurrence(runtime, occurrence, observation, runtimeErrors, runtimeEvents),
    );
  }
  const final = runtime.snapshot();
  const frame = container.getBoundingClientRect();
  return {
    runner: {
      viewport: { width: window.innerWidth, height: window.innerHeight },
      frame: { width: frame.width, height: frame.height, landscape: frame.width > frame.height },
      devicePixelRatio: window.devicePixelRatio,
      userAgent: navigator.userAgent,
      startedAt,
      finishedAt: new Date().toISOString(),
    },
    publication: {
      facts: bootstrap.publication.facts,
      mount: bootstrap.publication.mount,
      kernel: {
        packagePath: final.packagePath,
        publicationLayout: final.publicationLayout,
        readingOrderLength: final.readingOrderLength,
      },
      policy: bootstrap.policy,
      mediaOverlay: bootstrap.mediaOverlay,
    },
    documents,
  };
}

async function observeOccurrence(
  runtime: NavigatorRuntime,
  occurrence: number,
  observation: DocumentObservationRequest,
  runtimeErrors: readonly string[],
  runtimeEvents: readonly KernelEvent[],
): Promise<DocumentEvidence> {
  const started = performance.now();
  const errorsBefore = runtimeErrors.length;
  const eventsBefore = runtimeEvents.length;
  const facts = runtime.snapshot().readingOrder[occurrence] ?? null;
  let transition: KernelTransition | null = null;
  let open: DocumentEvidence["open"];
  try {
    transition = runtime.open(occurrence);
    open = { accepted: transition.accepted, failure: transition.failure };
  } catch (cause) {
    open = {
      accepted: false,
      failure: null,
      error: cause instanceof Error ? `${cause.name}: ${cause.message}` : String(cause),
    };
  }
  let settled = false;
  let observed: DocumentEvidence["observation"] = null;
  let observationError: string | null = null;
  if (transition !== null && transition.accepted) {
    try {
      await waitForCommit(runtime, openedGeneration(transition), runtimeErrors, runtimeEvents);
      settled = await waitForSettled(runtime, runtimeErrors);
      await waitForPaint();
      observed = await runtime.observeDocument(observation);
    } catch (cause) {
      observationError = cause instanceof Error ? `${cause.name}: ${cause.message}` : String(cause);
    }
  }
  const snapshot = runtime.snapshot();
  return {
    occurrence,
    facts,
    open,
    committed: snapshot.document.committed,
    fixedCompanion: snapshot.document.fixedCompanion,
    visible: runtime.visibleOccurrenceRange(),
    settled,
    observation: observed,
    observationError,
    errors: runtimeErrors.slice(errorsBefore),
    events: runtimeEvents.slice(eventsBefore).slice(-MAX_EVENTS_PER_DOCUMENT),
    elapsedMs: Math.round(performance.now() - started),
  };
}

async function waitForSettled(
  runtime: NavigatorRuntime,
  runtimeErrors: readonly string[],
): Promise<boolean> {
  const deadline = performance.now() + SETTLE_TIMEOUT_MS;
  const errorsBefore = runtimeErrors.length;
  let quiet: { readonly signature: string; readonly since: number } | null = null;
  while (performance.now() < deadline) {
    const snapshot = runtime.snapshot();
    const primary = snapshot.document.committed;
    const companion = snapshot.document.fixedCompanion;
    const settled =
      primary !== null &&
      primary.layout.state === "settled" &&
      snapshot.layout.pending === null &&
      (companion === null || companion.layout.state === "settled");
    const signature = settled
      ? JSON.stringify([
          primary.generation,
          primary.layout.layout.layoutGeneration,
          companion?.frame.occurrence ?? null,
          companion?.placement ?? null,
          snapshot.preparedFrames.length,
        ])
      : null;
    if (signature === null) quiet = null;
    else if (quiet === null || quiet.signature !== signature) {
      quiet = { signature, since: performance.now() };
    } else if (performance.now() - quiet.since >= QUIET_MS) return true;
    if (runtimeErrors.length > errorsBefore) return false;
    await new Promise((resolve) => window.setTimeout(resolve, 16));
  }
  return false;
}
