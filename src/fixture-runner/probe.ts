import { isRecord } from "./guards";
import type { ProbeGeneration, ProbeReport, RunnerCheck } from "./reports";
import {
  ACTIVE_PROBE_REPORT_TYPE,
  PEER_PROBE_SCENARIOS,
  PRIMARY_PROBE_SCENARIOS,
  PROBE_OUTCOMES,
  type ProbeEvidenceValue,
  type ProbeOutcome,
  type ProbeScenario,
  type ProbeSource,
} from "./scenarios";

const primaryScenarios = new Set<string>(PRIMARY_PROBE_SCENARIOS);
const peerScenarios = new Set<string>(PEER_PROBE_SCENARIOS);
const outcomes = new Set<string>(PROBE_OUTCOMES);
const reportKeys = new Set(["type", "source", "scenario", "outcome", "evidence"]);

export function parseProbeReport(value: unknown): ProbeReport | null {
  if (!isRecord(value) || Object.keys(value).some((key) => !reportKeys.has(key))) return null;
  if (
    value.type !== ACTIVE_PROBE_REPORT_TYPE ||
    (value.source !== "probe.xhtml" && value.source !== "peer.xhtml") ||
    typeof value.scenario !== "string" ||
    (value.source === "probe.xhtml"
      ? !primaryScenarios.has(value.scenario)
      : !peerScenarios.has(value.scenario)) ||
    typeof value.outcome !== "string" ||
    !outcomes.has(value.outcome)
  )
    return null;
  const evidence = parseEvidence(value.evidence);
  if (evidence === null) return null;
  return {
    type: ACTIVE_PROBE_REPORT_TYPE,
    source: value.source,
    scenario: value.scenario as ProbeScenario,
    outcome: value.outcome as ProbeOutcome,
    evidence,
  };
}

export function createProbeGeneration(generation: 1 | 2 | 3, source: ProbeSource): ProbeGeneration {
  return { generation, source, reports: new Map() };
}

export function reduceProbeGeneration(
  state: ProbeGeneration,
  report: ProbeReport,
): ProbeGeneration | null {
  if (report.source !== state.source || state.reports.has(report.scenario)) return null;
  const reports = new Map(state.reports);
  reports.set(report.scenario, report);
  return { ...state, reports };
}

export function generationComplete(state: ProbeGeneration): boolean {
  const expected = state.source === "probe.xhtml" ? PRIMARY_PROBE_SCENARIOS : PEER_PROBE_SCENARIOS;
  return expected.every((scenario) => state.reports.has(scenario));
}

export function missingProbeScenarios(state: ProbeGeneration): readonly ProbeScenario[] {
  const expected = state.source === "probe.xhtml" ? PRIMARY_PROBE_SCENARIOS : PEER_PROBE_SCENARIOS;
  return expected.filter((scenario) => !state.reports.has(scenario));
}

export function evaluateProbeGenerations(
  first: ProbeGeneration,
  second: ProbeGeneration,
  peer: ProbeGeneration,
): RunnerCheck[] {
  const checks: RunnerCheck[] = PRIMARY_PROBE_SCENARIOS.map((scenario) => {
    const expected: readonly ProbeOutcome[] =
      scenario === "document-domain-relaxation" || scenario === "cookie"
        ? ["passed", "blocked"]
        : scenario === "shared-worker"
          ? ["passed", "blocked", "unsupported"]
          : scenario === "service-worker-registration" ||
              scenario === "reader-isolation" ||
              scenario === "popup" ||
              scenario === "remote-fetch"
            ? ["blocked"]
            : ["passed"];
    const firstOutcome = first.reports.get(scenario)?.outcome;
    const secondOutcome = second.reports.get(scenario)?.outcome;
    const passed =
      firstOutcome !== undefined &&
      secondOutcome !== undefined &&
      expected.includes(firstOutcome) &&
      expected.includes(secondOutcome) &&
      (scenario !== "document-domain-relaxation" ||
        [first, second].every(domainRelaxationPrevented)) &&
      (scenario !== "cookie" || cookieAvailabilityCoherent(first, second));
    const evidence = passed
      ? ""
      : `; ${JSON.stringify({ first: first.reports.get(scenario)?.evidence, second: second.reports.get(scenario)?.evidence })}`;
    return {
      label: scenario,
      passed,
      detail: `${firstOutcome ?? "missing"}/${secondOutcome ?? "missing"}${evidence}`,
    };
  });
  for (const scenario of PEER_PROBE_SCENARIOS) {
    const report = peer.reports.get(scenario);
    checks.push({
      label: scenario,
      passed: report?.outcome === "passed",
      detail:
        report === undefined ? "missing" : `${report.outcome}; ${JSON.stringify(report.evidence)}`,
    });
  }
  checks.push(storageContinuityCheck("local-storage", first, second, "previous", "current"));
  checks.push(storageContinuityCheck("indexed-db", first, second, "previous", "current"));
  checks.push(storageContinuityCheck("cookie", first, second, "previous", "current"));
  checks.push(peerStorageCheck("local-storage", second, peer, "peer-local-storage"));
  checks.push(peerStorageCheck("indexed-db", second, peer, "peer-indexed-db"));
  checks.push(peerStorageCheck("cookie", second, peer, "peer-cookie"));
  return checks;
}

function storageContinuityCheck(
  scenario: "local-storage" | "indexed-db" | "cookie",
  first: ProbeGeneration,
  second: ProbeGeneration,
  previousKey: string,
  currentKey: string,
): RunnerCheck {
  if (scenario === "cookie") {
    const firstReport = first.reports.get(scenario);
    const secondReport = second.reports.get(scenario);
    if (firstReport?.outcome === "blocked" || secondReport?.outcome === "blocked") {
      const passed = [firstReport, secondReport].every(
        (report) =>
          report?.outcome === "blocked" &&
          report.evidence.stored === null &&
          report.evidence.wideDomainCookieVisible === false,
      );
      return { label: `${scenario}-reload-continuity`, passed, detail: "unavailable" };
    }
  }
  const current = first.reports.get(scenario)?.evidence[currentKey];
  const previous = second.reports.get(scenario)?.evidence[previousKey];
  return {
    label: `${scenario}-reload-continuity`,
    passed: (typeof current === "string" || typeof current === "number") && previous === current,
    detail: `${String(current)}/${String(previous)}`,
  };
}

function peerStorageCheck(
  scenario: "local-storage" | "indexed-db" | "cookie",
  current: ProbeGeneration,
  peer: ProbeGeneration,
  peerScenario: "peer-local-storage" | "peer-indexed-db" | "peer-cookie",
): RunnerCheck {
  if (scenario === "cookie" && current.reports.get(scenario)?.outcome === "blocked") {
    const observed = peer.reports.get(peerScenario)?.evidence.value;
    return {
      label: `${scenario}-peer-sharing`,
      passed: observed === null,
      detail: `unavailable/${String(observed)}`,
    };
  }
  const expected = current.reports.get(scenario)?.evidence.current;
  const observed = peer.reports.get(peerScenario)?.evidence.value;
  return {
    label: `${scenario}-peer-sharing`,
    passed: (typeof expected === "string" || typeof expected === "number") && observed === expected,
    detail: `${String(expected)}/${String(observed)}`,
  };
}

function cookieAvailabilityCoherent(first: ProbeGeneration, second: ProbeGeneration): boolean {
  const reports = [first.reports.get("cookie"), second.reports.get("cookie")];
  if (reports.every((report) => report?.outcome === "passed")) return true;
  return reports.every(
    (report) =>
      report?.outcome === "blocked" &&
      report.evidence.stored === null &&
      report.evidence.wideDomainCookieVisible === false,
  );
}

function domainRelaxationPrevented(generation: ProbeGeneration): boolean {
  const report = generation.reports.get("document-domain-relaxation");
  const evidence = report?.evidence;
  return (
    typeof evidence?.domainBefore === "string" &&
    evidence.domainBefore !== "localhost" &&
    typeof evidence.domainAfter === "string" &&
    evidence.domainAfter !== "localhost" &&
    typeof evidence.originBefore === "string" &&
    evidence.originBefore === evidence.originAfter &&
    evidence.target === "localhost" &&
    (evidence.assignment === "accepted" ||
      evidence.assignment === "security-error" ||
      evidence.assignment === "not-supported-error" ||
      evidence.assignment === "type-error" ||
      evidence.assignment === "other-error")
  );
}

function parseEvidence(
  value: unknown,
): Readonly<Record<string, ProbeEvidenceValue | readonly ProbeEvidenceValue[]>> | null {
  if (!isRecord(value)) return null;
  const entries = Object.entries(value);
  if (entries.length > 16) return null;
  const evidence: Record<string, ProbeEvidenceValue | readonly ProbeEvidenceValue[]> = {};
  for (const [key, item] of entries) {
    if (!/^[a-zA-Z][a-zA-Z0-9]{0,63}$/.test(key)) return null;
    if (Array.isArray(item)) {
      if (item.length > 16 || item.some((entry) => !isEvidenceValue(entry))) return null;
      evidence[key] = item;
    } else if (isEvidenceValue(item)) {
      evidence[key] = item;
    } else return null;
  }
  return Object.freeze(evidence);
}

function isEvidenceValue(value: unknown): value is ProbeEvidenceValue {
  return (
    value === null ||
    typeof value === "boolean" ||
    (typeof value === "number" && Number.isFinite(value)) ||
    (typeof value === "string" && value.length <= 256)
  );
}
