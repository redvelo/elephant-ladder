import { isRecord } from "./guards";
import {
  evaluateContainerConstrained,
  evaluateRemotePolicyDenied,
  parseContainerConstrainedReport,
  parseRemotePolicyReport,
} from "./probes";
import type {
  RunnerCheck,
  RunnerContractDefinition,
  SingleReport,
  SingleReportContractDefinition,
} from "./reports";
import {
  ACTIVE_PROBE_REPORT_TYPE,
  CONTAINER_CONSTRAINED_REPORT_TYPE,
  REMOTE_POLICY_REPORT_TYPE,
} from "./scenarios";

function defineSingleReportContract<T extends SingleReport>(
  reportType: T["type"],
  source: T["source"],
  parse: (value: unknown) => T | null,
  evaluate: (report: T) => RunnerCheck[],
): SingleReportContractDefinition {
  return Object.freeze({
    workflow: "single-report",
    reportType,
    source,
    parse,
    evaluate(report: SingleReport) {
      const parsed = parse(report);
      if (parsed === null) throw new Error(`Report does not match ${reportType}`);
      return evaluate(parsed);
    },
  });
}

export const RUNNER_CONTRACTS = Object.freeze({
  "active-probe-v1": Object.freeze({
    workflow: "active-probe",
    reportType: ACTIVE_PROBE_REPORT_TYPE,
  }),
  "container-constrained-v1": defineSingleReportContract(
    CONTAINER_CONSTRAINED_REPORT_TYPE,
    "EPUB/content.xhtml",
    parseContainerConstrainedReport,
    evaluateContainerConstrained,
  ),
  "remote-policy-denied-v1": defineSingleReportContract(
    REMOTE_POLICY_REPORT_TYPE,
    "EPUB/text/probe.xhtml",
    parseRemotePolicyReport,
    evaluateRemotePolicyDenied,
  ),
  "static-fxl-v1": Object.freeze({
    workflow: "static-fxl",
    reportType: "elephant-static-fxl-parent-v1",
  }),
  "evidence-v1": Object.freeze({
    workflow: "evidence",
    reportType: "elephant-evidence-v1",
  }),
}) satisfies Readonly<Record<string, RunnerContractDefinition>>;

export type RunnerResultContract = keyof typeof RUNNER_CONTRACTS;

const runnerReportTypes = new Set<string>(
  Object.values(RUNNER_CONTRACTS).map((definition) => definition.reportType),
);

export function runnerContract(value: unknown): RunnerContractDefinition | null {
  if (typeof value !== "string" || !Object.prototype.hasOwnProperty.call(RUNNER_CONTRACTS, value))
    return null;
  return RUNNER_CONTRACTS[value as RunnerResultContract];
}

export function isRecognizedRunnerMessage(value: unknown): boolean {
  return isRecord(value) && typeof value.type === "string" && runnerReportTypes.has(value.type);
}
