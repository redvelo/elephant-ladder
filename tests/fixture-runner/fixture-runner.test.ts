import { expect, test } from "vitest";
import {
  ACTIVE_PROBE_REPORT_TYPE,
  CONTAINER_CONSTRAINED_REPORT_TYPE,
  PEER_PROBE_SCENARIOS,
  PRIMARY_PROBE_SCENARIOS,
  REMOTE_POLICY_REPORT_TYPE,
  type ProbeOutcome,
  type ProbeScenario,
  type ProbeSource,
} from "../../src/fixture-runner/scenarios";
import { createPublicationObservation } from "../../src/fixture-runner/observation";
import {
  createProbeGeneration,
  evaluateProbeGenerations,
  generationComplete,
  parseProbeReport,
  reduceProbeGeneration,
} from "../../src/fixture-runner/probe";
import {
  RUNNER_CONTRACTS,
  runnerContract,
  isRecognizedRunnerMessage,
  type RunnerResultContract,
} from "../../src/fixture-runner/contracts";
import { isRunnerFixtureDescriptor } from "../../src/fixture-runner/fixture";
import { type SingleReportContractDefinition } from "../../src/fixture-runner/reports";

function singleReportContract(
  resultContract: RunnerResultContract,
): SingleReportContractDefinition {
  const definition = runnerContract(resultContract);
  expect(definition?.workflow).toBe("single-report");
  if (definition?.workflow !== "single-report")
    throw new Error("Expected a single-report workflow");
  return definition;
}

test("runner registry exposes the supported contracts and workflows", () => {
  const keys = [
    "active-probe-v1",
    "container-constrained-v1",
    "remote-policy-denied-v1",
    "static-fxl-v1",
    "evidence-v1",
  ] as const;
  expect(Object.keys(RUNNER_CONTRACTS)).toStrictEqual(keys);
  for (const key of keys) expect(runnerContract(key)).toBe(RUNNER_CONTRACTS[key]);

  expect(runnerContract("unknown-v1")).toBeNull();
  expect(runnerContract("toString")).toBeNull();
  expect(runnerContract("w3c-scr-support-v1")).toBeNull();

  expect(RUNNER_CONTRACTS["active-probe-v1"].workflow).toBe("active-probe");
  expect(RUNNER_CONTRACTS["static-fxl-v1"].workflow).toBe("static-fxl");
  expect(RUNNER_CONTRACTS["evidence-v1"].workflow).toBe("evidence");
  for (const key of ["container-constrained-v1", "remote-policy-denied-v1"] as const) {
    const definition = RUNNER_CONTRACTS[key];
    expect(definition.workflow).toBe("single-report");
    expect(typeof definition.parse).toBe("function");
    expect(typeof definition.evaluate).toBe("function");
    expect(["EPUB/content.xhtml", "EPUB/text/probe.xhtml"]).toContain(definition.source);
  }
});

test("single-report registry dispatch retains strict contract parsing", () => {
  const definition = singleReportContract("container-constrained-v1");
  const value = {
    type: CONTAINER_CONSTRAINED_REPORT_TYPE,
    source: "EPUB/content.xhtml",
    childScript: true,
    epubReadingSystem: true,
    parentDomBlocked: true,
    frameSizeBlocked: true,
    readerDomBlocked: true,
    credentialsBlocked: true,
    nativeRouteBlocked: true,
  };
  const parsed = definition.parse(value);
  expect(parsed).not.toBeNull();
  expect(definition.evaluate(parsed!).every((check) => check.passed)).toBe(true);
  expect(definition.parse({ ...value, type: REMOTE_POLICY_REPORT_TYPE })).toBeNull();
  expect(() => definition.evaluate({ ...value, type: REMOTE_POLICY_REPORT_TYPE } as never)).toThrow(
    /does not match/,
  );
});

test("recognized runner messages are derived from registered report types", () => {
  expect(isRecognizedRunnerMessage({ type: ACTIVE_PROBE_REPORT_TYPE })).toBe(true);
  expect(isRecognizedRunnerMessage({ type: REMOTE_POLICY_REPORT_TYPE })).toBe(true);
  expect(isRecognizedRunnerMessage({ type: "elephant-publication-snapshot-v1" })).toBe(false);
  expect(isRecognizedRunnerMessage({ type: "unknown" })).toBe(false);
  expect(isRecognizedRunnerMessage(null)).toBe(false);
});

test("fixture descriptors are accepted only in the exact bounded shape", () => {
  const descriptor = {
    id: "active-probe",
    label: "Elephant Active Probe",
    resultContract: "active-probe-v1",
    expectedTotal: 37,
    expectedSha256: "e".repeat(64),
    blake3: "b".repeat(64),
    policyProfile: "active-enabled-offline-v1",
  };
  expect(isRunnerFixtureDescriptor(descriptor)).toBe(true);
  for (const override of [
    { id: "Active Probe" },
    { resultContract: "w3c-scr-support-v1" },
    { expectedTotal: 0 },
    { expectedTotal: 65 },
    { expectedSha256: "short" },
    { policyProfile: "other" },
    { extra: true },
  ]) {
    expect(isRunnerFixtureDescriptor({ ...descriptor, ...override })).toBe(false);
  }

  const evidence = {
    ...descriptor,
    id: "lay-pp-svg-icb_multi",
    label: "lay-pp-svg-icb_multi",
    resultContract: "evidence-v1",
    expectedTotal: 0,
  };
  expect(isRunnerFixtureDescriptor(evidence)).toBe(true);
  expect(isRunnerFixtureDescriptor({ ...evidence, expectedTotal: 1 })).toBe(false);
  expect(isRunnerFixtureDescriptor({ ...evidence, id: "a".repeat(81) })).toBe(false);
});

test("container-constrained reports require the exact bounded proof shape", () => {
  const definition = singleReportContract("container-constrained-v1");
  const report = {
    type: CONTAINER_CONSTRAINED_REPORT_TYPE,
    source: "EPUB/content.xhtml",
    childScript: true,
    epubReadingSystem: true,
    parentDomBlocked: true,
    frameSizeBlocked: true,
    readerDomBlocked: true,
    credentialsBlocked: true,
    nativeRouteBlocked: true,
  } as const;
  const parsed = definition.parse(report);
  expect(parsed).toStrictEqual(report);
  expect(definition.evaluate(parsed!)).toHaveLength(7);
  expect(definition.evaluate(parsed!).every((check) => check.passed)).toBe(true);
  expect(definition.parse({ ...report, extra: true })).toBeNull();
  expect(definition.parse({ ...report, nativeRouteBlocked: "yes" })).toBeNull();
});

test("remote policy denied contract requires directive-specific CSP outcomes", () => {
  const definition = singleReportContract("remote-policy-denied-v1");
  const report = {
    type: REMOTE_POLICY_REPORT_TYPE,
    source: "EPUB/text/probe.xhtml",
    httpsFetch: "blocked:connect-src",
    httpsImage: "blocked:img-src",
    httpsStyle: "blocked:style-src-elem",
    httpsScript: "blocked:script-src-elem",
    httpsAudio: "blocked:media-src",
    httpsFrame: "blocked:frame-src",
    wssEcho: "blocked:connect-src",
    httpFetch: "blocked:connect-src",
    wsEcho: "blocked:connect-src",
  } as const;
  const parsed = definition.parse(report);
  expect(parsed).toStrictEqual(report);
  expect(definition.evaluate(parsed!)).toHaveLength(9);
  expect(definition.evaluate(parsed!).every((check) => check.passed)).toBe(true);
  expect(definition.parse({ ...report, httpsFetch: "blocked" })).toBeNull();
  const loaded = definition.parse({ ...report, httpsImage: "loaded" });
  expect(loaded).not.toBeNull();
  expect(
    definition.evaluate(loaded!).find((check) => check.label === "remote-policy-httpsImage")
      ?.passed,
  ).toBe(false);
});

function report(
  source: ProbeSource,
  scenario: ProbeScenario,
  outcome: ProbeOutcome = "passed",
  evidence: object = {},
) {
  return { type: ACTIVE_PROBE_REPORT_TYPE, source, scenario, outcome, evidence };
}

test("strict active-probe validation binds scenarios to their authored document", () => {
  expect(
    parseProbeReport({ ...report("probe.xhtml", "image"), source: "scripted-probe.svg" }),
  ).toBeNull();
  expect(parseProbeReport(report("peer.xhtml", "image"))).toBeNull();
  expect(parseProbeReport(report("probe.xhtml", "peer-complete"))).toBeNull();
  expect(parseProbeReport({ ...report("probe.xhtml", "image"), scenario: "unknown" })).toBeNull();
  expect(parseProbeReport({ ...report("probe.xhtml", "image"), outcome: "maybe" })).toBeNull();
  expect(parseProbeReport({ ...report("probe.xhtml", "image"), extra: true })).toBeNull();
  expect(
    parseProbeReport(report("probe.xhtml", "image", "passed", { nested: { value: true } })),
  ).toBeNull();
  expect(
    parseProbeReport(report("probe.xhtml", "image", "passed", { text: "x".repeat(257) })),
  ).toBeNull();
  expect(
    parseProbeReport(report("probe.xhtml", "image", "passed", { loaded: true }))?.evidence,
  ).toStrictEqual({ loaded: true });
});

test("generation reduction accepts one report for every expected phase scenario", () => {
  let primary = createProbeGeneration(1, "probe.xhtml");
  for (const scenario of PRIMARY_PROBE_SCENARIOS)
    primary = reduceProbeGeneration(primary, parseProbeReport(report("probe.xhtml", scenario))!)!;
  expect(generationComplete(primary)).toBe(true);
  expect(
    reduceProbeGeneration(
      primary,
      parseProbeReport(report("probe.xhtml", PRIMARY_PROBE_SCENARIOS[0]))!,
    ),
  ).toBeNull();

  let peer = createProbeGeneration(3, "peer.xhtml");
  for (const scenario of PEER_PROBE_SCENARIOS)
    peer = reduceProbeGeneration(peer, parseProbeReport(report("peer.xhtml", scenario))!)!;
  expect(generationComplete(peer)).toBe(true);
  expect(reduceProbeGeneration(peer, parseProbeReport(report("probe.xhtml", "image"))!)).toBeNull();
});

test("active probe evaluation covers reload continuity and peer storage sharing", () => {
  const first = completePrimary(1, 1, null);
  const second = completePrimary(2, 2, 1);
  let peer = createProbeGeneration(3, "peer.xhtml");
  peer = reduceProbeGeneration(
    peer,
    parseProbeReport(report("peer.xhtml", "peer-local-storage", "passed", { value: 2 }))!,
  )!;
  peer = reduceProbeGeneration(
    peer,
    parseProbeReport(report("peer.xhtml", "peer-indexed-db", "passed", { value: 2 }))!,
  )!;
  peer = reduceProbeGeneration(
    peer,
    parseProbeReport(report("peer.xhtml", "peer-cookie", "passed", { value: "2" }))!,
  )!;
  peer = reduceProbeGeneration(
    peer,
    parseProbeReport(
      report("peer.xhtml", "peer-worker-teardown", "passed", { staleHeartbeat: false }),
    )!,
  )!;
  peer = reduceProbeGeneration(peer, parseProbeReport(report("peer.xhtml", "peer-complete"))!)!;
  const checks = evaluateProbeGenerations(first, second, peer);
  expect(checks.length).toBe(33);
  expect(checks.every((check) => check.passed)).toBe(true);
  expect(checks.slice(-3).map((check) => check.label)).toStrictEqual([
    "local-storage-peer-sharing",
    "indexed-db-peer-sharing",
    "cookie-peer-sharing",
  ]);

  const relaxedReports = new Map(second.reports);
  relaxedReports.set(
    "document-domain-relaxation",
    parseProbeReport(
      report("probe.xhtml", "document-domain-relaxation", "passed", {
        assignment: "accepted",
        domainBefore: "p-publication.localhost",
        domainAfter: "localhost",
        originBefore: "http://p-publication.localhost:43123",
        originAfter: "http://p-publication.localhost:43123",
        target: "localhost",
        crossOriginIsolated: false,
      }),
    )!,
  );
  const relaxed = evaluateProbeGenerations(first, { ...second, reports: relaxedReports }, peer);
  expect(relaxed.find((check) => check.label === "document-domain-relaxation")?.passed).toBe(false);
});

test("publication observation shaping canonically orders reports and binds phases", () => {
  let generation = createProbeGeneration(1, "probe.xhtml");
  for (const scenario of [...PRIMARY_PROBE_SCENARIOS].reverse()) {
    generation = reduceProbeGeneration(
      generation,
      parseProbeReport(report("probe.xhtml", scenario))!,
    )!;
  }
  const phase = createPublicationObservation(1, "EPUB/text/probe.xhtml", generation);
  if (phase.source !== "EPUB/text/probe.xhtml") throw new Error("Expected a probe observation");
  expect(
    phase.reports.map((item) => ("scenario" in item ? item.scenario : item.type)),
  ).toStrictEqual(PRIMARY_PROBE_SCENARIOS);
  expect("snapshot" in phase).toBe(false);
  expect(() => createPublicationObservation(3, "EPUB/text/probe.xhtml", generation)).toThrow(
    /complete publication phase/,
  );
  expect(() => createPublicationObservation(1, "EPUB/text/peer.xhtml", generation)).toThrow(
    /complete publication phase/,
  );
  const remote = singleReportContract("remote-policy-denied-v1").parse({
    type: REMOTE_POLICY_REPORT_TYPE,
    source: "EPUB/text/probe.xhtml",
    httpsFetch: "blocked:connect-src",
    httpsImage: "blocked:img-src",
    httpsStyle: "blocked:style-src",
    httpsScript: "blocked:script-src",
    httpsAudio: "blocked:media-src",
    httpsFrame: "blocked:frame-src",
    wssEcho: "blocked:connect-src",
    httpFetch: "blocked:connect-src",
    wsEcho: "blocked:connect-src",
  })!;
  expect(createPublicationObservation(1, "EPUB/text/probe.xhtml", remote).reports).toStrictEqual([
    remote,
  ]);
  expect(() => createPublicationObservation(1, "EPUB/content.xhtml", remote)).toThrow(
    /does not match/,
  );
});

function completePrimary(generation: 1 | 2, current: number, previous: number | null) {
  let state = createProbeGeneration(generation, "probe.xhtml");
  for (const scenario of PRIMARY_PROBE_SCENARIOS) {
    const outcome =
      scenario === "service-worker-registration" ||
      scenario === "reader-isolation" ||
      scenario === "popup" ||
      scenario === "remote-fetch"
        ? "blocked"
        : "passed";
    const evidence =
      scenario === "local-storage" || scenario === "indexed-db"
        ? { current, previous }
        : scenario === "cookie"
          ? { current: String(current), previous: previous === null ? null : String(previous) }
          : scenario === "document-domain-relaxation"
            ? {
                assignment: "accepted",
                domainBefore: "p-publication.localhost",
                domainAfter: "p-publication.localhost",
                originBefore: "http://p-publication.localhost:43123",
                originAfter: "http://p-publication.localhost:43123",
                target: "localhost",
                crossOriginIsolated: false,
              }
            : {};
    state = reduceProbeGeneration(
      state,
      parseProbeReport(report("probe.xhtml", scenario, outcome, evidence))!,
    )!;
  }
  return state;
}
