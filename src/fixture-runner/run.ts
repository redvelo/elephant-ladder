import type { KernelEvent } from "epub-navigator";
import {
  createNavigatorRuntime,
  type NavigatorRuntime,
  type PublicationPaginationCapabilities,
} from "epub-navigator/runtime";
import { loadBootstrap } from "../host/bootstrap/parse";
import type { ReadingBootstrap } from "../host/bootstrap/types";
import { createLivePaginationCapabilities } from "../reader/capabilities/pagination";
import { acquireFixture } from "./acquire";
import { createRunnerChannel, type RunnerChannel } from "./channel";
import { runnerContract } from "./contracts";
import {
  DEFAULT_OBSERVATION,
  collectEvidence,
  isObservationPlan,
  type EvidenceSubmission,
} from "./evidence";
import { loadFixture, type RunnerFixtureDescriptor } from "./fixture";
import {
  captureGeometry,
  openedGeneration,
  requireCurrentFrame,
  waitForCommit,
  waitForPaint,
} from "./frames";
import { evaluateHostBoundary } from "./host-boundary";
import { createPublicationObservation } from "./observation";
import { evaluateProbeGenerations } from "./probe";
import type {
  PublicationObservation,
  RunnerCheck,
  SingleReportContractDefinition,
} from "./reports";
import { evaluateStaticFxlSnapshot, parseStaticFxlSnapshot } from "./static-fxl";
import {
  authenticatedReportRunnerError,
  authenticatedSubmit,
  authenticatedSubmitEvidence,
  authenticatedSubmitIncomplete,
  authenticatedSubmitIncompleteEvidence,
} from "./submit";

export type RunnerState =
  | "starting"
  | "policy"
  | "importing"
  | "opening"
  | "reloading"
  | "opening-peer"
  | "observing"
  | "submitting"
  | "passed"
  | "failed"
  | "recorded";

interface RunnerView {
  readonly setState: (state: RunnerState) => void;
  readonly setChecks: (checks: RunnerCheck[]) => void;
  readonly setError: (error: string | null) => void;
  readonly setFixture: (fixture: RunnerFixtureDescriptor) => void;
}

/**
 * One fixture run spans two page loads. The library page takes the fixture through the
 * product's import and open routes and navigates to the reading page the host answers with;
 * the reading page drives Navigator over the product's own reading bootstrap and reports.
 */
export function startFixtureRun(
  frameHost: { readonly current: HTMLDivElement | null },
  { setState, setChecks, setError, setFixture }: RunnerView,
): () => void {
  const abort = new AbortController();
  const running = (): boolean => !abort.signal.aborted;
  let runtime: NavigatorRuntime | null = null;
  let pagination: PublicationPaginationCapabilities | undefined;
  let session: {
    readonly token: string;
    readonly expectedTotal: number;
    readonly evidence: boolean;
  } | null = null;
  const publicationObservations: PublicationObservation[] = [];
  let submitted = false;

  void run().catch(async (cause) => {
    if (!running()) return;
    const message = cause instanceof Error ? `${cause.name}: ${cause.message}` : String(cause);
    setError(message);
    setChecks([{ label: "runner-execution", passed: false, detail: message }]);
    // A terminal result is terminal: a later failure is reported, never submitted again.
    if (session !== null && !submitted) {
      setState("submitting");
      try {
        await authenticatedReportRunnerError(session.token, message);
        await waitForPaint();
        if (session.evidence) await authenticatedSubmitIncompleteEvidence(session.token, message);
        else await authenticatedSubmitIncomplete(session.token, session.expectedTotal, message);
      } catch (submissionCause) {
        setError(
          `${message}\n${submissionCause instanceof Error ? submissionCause.message : String(submissionCause)}`,
        );
      }
    }
    if (running()) setState("failed");
  });

  async function run(): Promise<void> {
    const bootstrap = await loadBootstrap();
    const { descriptor, observation } = await loadFixture(bootstrap.sessionToken);
    const contract = runnerContract(descriptor.resultContract);
    if (contract === null) throw new Error("Fixture result contract is unsupported");
    session = {
      token: bootstrap.sessionToken,
      expectedTotal: descriptor.expectedTotal,
      evidence: contract.workflow === "evidence",
    };
    setFixture(descriptor);
    if (bootstrap.mode === "library") {
      const navigationPath = await acquireFixture(bootstrap, descriptor, abort.signal, setState);
      if (running()) window.location.replace(navigationPath);
      return;
    }
    if (contract.workflow === "evidence") {
      if (observation !== null && !isObservationPlan(observation))
        throw new Error("Observation plan is invalid");
      await observe(bootstrap, descriptor, observation ?? DEFAULT_OBSERVATION);
      return;
    }
    await read(bootstrap, descriptor);
  }

  async function mountRuntime(
    bootstrap: ReadingBootstrap,
    descriptor: RunnerFixtureDescriptor,
    documentObservation: boolean,
  ) {
    const container = frameHost.current;
    if (container === null) throw new Error("Navigator frame host is missing");
    setState("opening");
    pagination =
      bootstrap.pagination.state === "available"
        ? createLivePaginationCapabilities(bootstrap.pagination, bootstrap.sessionToken)
        : undefined;
    runtime = await createNavigatorRuntime({
      session: { id: bootstrap.readerSessionId, publication: bootstrap.publication },
      container,
      frameTitle: descriptor.label,
      pagination,
      documentObservation,
    });
    const runtimeErrors: string[] = [];
    const runtimeEvents: KernelEvent[] = [];
    runtime.subscribeErrors((value) =>
      runtimeErrors.push(`${value.failure.code}: ${value.message}`),
    );
    runtime.subscribe((transition) => runtimeEvents.push(...transition.events));
    runtime.configure({
      appearance: { rootScrollbar: "publisher" },
      input: {
        keyboardMode: "off",
        touchZones: false,
        progressionDirection: "ltr",
        selectionMenu: "native",
      },
    });
    return { runtime, container, runtimeErrors, runtimeEvents };
  }

  async function observe(
    bootstrap: ReadingBootstrap,
    descriptor: RunnerFixtureDescriptor,
    observation: Parameters<typeof collectEvidence>[3],
  ) {
    const mounted = await mountRuntime(bootstrap, descriptor, true);
    setState("observing");
    const evidence = await collectEvidence(
      mounted.runtime,
      mounted.container,
      bootstrap,
      observation,
      mounted.runtimeErrors,
      mounted.runtimeEvents,
      abort.signal,
    );
    if (!running()) return;
    setChecks(
      evidence.documents.map((document) => ({
        label: `${document.occurrence}: ${document.committed?.frame.resource ?? "(not committed)"}`,
        passed: document.observation !== null,
        detail:
          document.observationError ??
          `${document.settled ? "settled" : "unsettled"}; ${document.observation?.queries.length ?? 0} queries; ${document.errors.length} errors`,
      })),
    );
    setState("submitting");
    await waitForPaint();
    await authenticatedSubmitEvidence(bootstrap.sessionToken, evidence, abort.signal);
    submitted = true;
    if (!running()) return;
    setState("recorded");
    handOver(mounted.runtime, evidence);
  }

  // Evidence is in; a person inspecting the run can now page through the book. Nothing
  // here may fail the run, which has already reported its result.
  function handOver(activeRuntime: NavigatorRuntime, evidence: EvidenceSubmission): void {
    try {
      activeRuntime.configure({
        appearance: { rootScrollbar: "publisher" },
        input: {
          keyboardMode: "standard",
          touchZones: true,
          clickZones: true,
          progressionDirection: "ltr",
          selectionMenu: "native",
        },
      });
      if (evidence.documents[0]?.open.accepted === true) activeRuntime.open(0);
    } catch (cause) {
      setError(`The publication was recorded but could not be handed over: ${String(cause)}`);
    }
  }

  async function read(bootstrap: ReadingBootstrap, descriptor: RunnerFixtureDescriptor) {
    const conversation = createRunnerChannel(bootstrap.publicationOrigin, abort.signal);
    const mounted = await mountRuntime(bootstrap, descriptor, false);
    const { container, runtimeErrors, runtimeEvents } = mounted;
    const runtime = mounted.runtime;
    const contract = runnerContract(descriptor.resultContract);
    if (contract === null || contract.workflow === "evidence")
      throw new Error("Fixture result contract is unsupported");
    const evaluated =
      contract.workflow === "active-probe"
        ? await runActiveProbe(
            conversation,
            runtime,
            container,
            runtimeErrors,
            runtimeEvents,
            bootstrap.publicationOrigin,
          )
        : contract.workflow === "static-fxl"
          ? await runStaticFxl(
              runtime,
              container,
              runtimeErrors,
              runtimeEvents,
              bootstrap,
              descriptor,
            )
          : await runSingleReport(
              conversation,
              runtime,
              container,
              runtimeErrors,
              runtimeEvents,
              contract,
            );
    if (!running()) return;
    if (evaluated.length !== descriptor.expectedTotal) {
      throw new Error(
        `Fixture produced ${evaluated.length} checks; expected ${descriptor.expectedTotal}`,
      );
    }
    setChecks(evaluated);
    setState("submitting");
    await waitForPaint();
    await authenticatedSubmit(
      bootstrap.sessionToken,
      evaluated,
      captureGeometry(),
      publicationObservations,
      abort.signal,
    );
    submitted = true;
    if (running()) setState(evaluated.every((check) => check.passed) ? "passed" : "failed");
  }

  async function runStaticFxl(
    activeRuntime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
    bootstrap: ReadingBootstrap,
    descriptor: RunnerFixtureDescriptor,
  ): Promise<RunnerCheck[]> {
    const result: RunnerCheck[] = [];
    for (const phase of [0, 1] as const) {
      const generation = openedGeneration(activeRuntime.open(phase));
      await waitForCommit(activeRuntime, generation, runtimeErrors, runtimeEvents);
      type Snapshot = ReturnType<typeof activeRuntime.snapshot>;
      const deadline = performance.now() + 10_000;
      // A settled spread is one whose snapshot survives a paint unchanged, including
      // whether the companion frame is in the DOM: the companion is rendered from
      // runtime state and reaches the DOM after the runtime reports it, and font or
      // layout delivery can still move geometry between two polls. Capturing what was
      // confirmed, rather than re-reading after the proof, leaves no gap to race.
      const settledSignature = (snapshot: Snapshot) => {
        const primary = snapshot.document.committed;
        const companion = snapshot.document.fixedCompanion;
        const mounted =
          container.querySelector('iframe[data-epub-navigator-participant="fixed-companion"]') !==
          null;
        const settled =
          primary?.generation === generation &&
          primary.layout.state === "settled" &&
          snapshot.layout.pending === null &&
          (companion !== null) === mounted &&
          (phase === 0 ? companion === null : companion?.layout.state === "settled");
        return settled ? JSON.stringify(snapshot) : null;
      };
      type CommittedDocument = NonNullable<Snapshot["document"]["committed"]>;
      let confirmed: { snapshot: Snapshot; primary: CommittedDocument } | null = null;
      while (confirmed === null) {
        if (runtimeErrors.length > 0) throw new Error(runtimeErrors.join("; "));
        if (performance.now() >= deadline)
          throw new Error(`Static FXL phase ${phase} did not settle its visible spread`);
        const signature = settledSignature(activeRuntime.snapshot());
        if (signature === null) {
          await new Promise((resolve) => window.setTimeout(resolve, 16));
          continue;
        }
        await waitForPaint();
        const after = activeRuntime.snapshot();
        const primary = after.document.committed;
        if (primary !== null && settledSignature(after) === signature) {
          confirmed = { snapshot: after, primary };
        }
      }
      const { primary } = confirmed;
      const companion = confirmed.snapshot.document.fixedCompanion;
      const primaryFrame = requireCurrentFrame(container);
      const companionFrame = container.querySelector<HTMLIFrameElement>(
        'iframe[data-epub-navigator-participant="fixed-companion"]',
      );
      if ((companion === null) !== (companionFrame === null))
        throw new Error("Visible companion DOM does not match runtime");
      const participant = (
        document: typeof primary | NonNullable<typeof companion>,
        frame: HTMLIFrameElement,
      ) => {
        if (
          frame.src !== `${bootstrap.publicationOrigin}/${document.frame.resource}` ||
          getComputedStyle(frame).visibility !== "visible" ||
          getComputedStyle(frame).display === "none"
        )
          throw new Error("Static participant is not the visible canonical frame");
        return {
          occurrence: document.frame.occurrence,
          resource: document.frame.resource,
          adapter: document.frame.adapter,
          frameGeneration: document.frame.generation,
          layoutGeneration: document.layout.layout.layoutGeneration,
          rect: frame.getBoundingClientRect().toJSON(),
        };
      };
      const observed = parseStaticFxlSnapshot({
        kind: "elephant-static-fxl-parent-v1",
        documentGeneration: primary.generation,
        primary: participant(primary, primaryFrame),
        companion:
          companion === null
            ? null
            : {
                ...participant(companion, companionFrame!),
                ownerDocumentGeneration: companion.ownerDocumentGeneration,
                placement: companion.placement,
              },
        visible: activeRuntime.visibleOccurrenceRange(),
        hostRect: container.getBoundingClientRect().toJSON(),
      });
      if (observed === null) throw new Error("Static FXL parent snapshot is malformed");
      publicationObservations.push({
        generation: phase === 0 ? 1 : 2,
        source: phase === 0 ? "EPUB/center.xhtml" : "EPUB/first.xhtml",
        reports: [],
        snapshot: observed,
      });
      result.push(...evaluateStaticFxlSnapshot(observed, descriptor.id, phase));
    }
    return result;
  }

  async function runActiveProbe(
    conversation: RunnerChannel,
    activeRuntime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
    publicationOrigin: string,
  ): Promise<RunnerCheck[]> {
    const first = await conversation.collectProbeGeneration(
      1,
      "probe.xhtml",
      activeRuntime.open(0),
      activeRuntime,
      container,
      runtimeErrors,
      runtimeEvents,
    );
    publicationObservations.push(createPublicationObservation(1, "EPUB/text/probe.xhtml", first));
    if (!running()) return [];
    setState("reloading");
    const second = await conversation.collectProbeGeneration(
      2,
      "probe.xhtml",
      activeRuntime.reload(),
      activeRuntime,
      container,
      runtimeErrors,
      runtimeEvents,
    );
    publicationObservations.push(createPublicationObservation(2, "EPUB/text/probe.xhtml", second));
    if (!running()) return [];
    setState("opening-peer");
    const peer = await conversation.collectProbeGeneration(
      3,
      "peer.xhtml",
      activeRuntime.open(1),
      activeRuntime,
      container,
      runtimeErrors,
      runtimeEvents,
    );
    publicationObservations.push(createPublicationObservation(3, "EPUB/text/peer.xhtml", peer));
    return [
      ...evaluateProbeGenerations(first, second, peer),
      ...evaluateHostBoundary(container, publicationOrigin, conversation.agentMessageOrigins),
    ];
  }

  async function runSingleReport(
    conversation: RunnerChannel,
    activeRuntime: NavigatorRuntime,
    container: HTMLElement,
    runtimeErrors: readonly string[],
    runtimeEvents: readonly KernelEvent[],
    contract: SingleReportContractDefinition,
  ): Promise<RunnerCheck[]> {
    const observed = await conversation.collectSingleReport(
      activeRuntime,
      container,
      runtimeErrors,
      runtimeEvents,
      contract,
      0,
    );
    publicationObservations.push(createPublicationObservation(1, contract.source, observed));
    return contract.evaluate(observed);
  }

  return () => {
    abort.abort();
    runtime?.close();
    pagination?.close();
  };
}
