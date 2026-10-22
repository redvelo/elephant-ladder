import type {
  ContainerConstrainedReport,
  ObservationSource,
  ProbeGeneration,
  PublicationObservation,
  RemotePolicyReport,
} from "./reports";
import { generationComplete } from "./probe";
import {
  CONTAINER_CONSTRAINED_REPORT_TYPE,
  PEER_PROBE_SCENARIOS,
  PRIMARY_PROBE_SCENARIOS,
  REMOTE_POLICY_REPORT_TYPE,
} from "./scenarios";

export function createPublicationObservation(
  generation: 1 | 2 | 3,
  source: ObservationSource,
  reports: ProbeGeneration | ContainerConstrainedReport | RemotePolicyReport,
): PublicationObservation {
  if ("reports" in reports) {
    const reportSource =
      source === "EPUB/text/probe.xhtml"
        ? "probe.xhtml"
        : source === "EPUB/text/peer.xhtml"
          ? "peer.xhtml"
          : null;
    if (
      reports.generation !== generation ||
      reports.source !== reportSource ||
      !generationComplete(reports)
    ) {
      throw new Error("Probe reports do not match a complete publication phase");
    }
    const expected =
      reports.source === "probe.xhtml" ? PRIMARY_PROBE_SCENARIOS : PEER_PROBE_SCENARIOS;
    const orderedReports = expected.map((scenario) => reports.reports.get(scenario)!);
    if (source === "EPUB/text/probe.xhtml" && (generation === 1 || generation === 2)) {
      return { generation, source, reports: orderedReports };
    }
    if (source === "EPUB/text/peer.xhtml" && generation === 3)
      return { generation, source, reports: orderedReports };
    throw new Error("Probe source is invalid for publication phase generation");
  }
  if (reports.source !== source || generation !== 1) {
    throw new Error("Report does not match its publication phase");
  }
  if (source === "EPUB/content.xhtml" && reports.type === CONTAINER_CONSTRAINED_REPORT_TYPE) {
    return { generation, source, reports: [reports] };
  }
  if (source === "EPUB/text/probe.xhtml" && reports.type === REMOTE_POLICY_REPORT_TYPE) {
    return { generation, source, reports: [reports] };
  }
  throw new Error("Report does not match its publication phase");
}
