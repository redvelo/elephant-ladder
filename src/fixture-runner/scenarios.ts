export const ACTIVE_PROBE_REPORT_TYPE = "elephant-active-probe-result-v1";
export const CONTAINER_CONSTRAINED_REPORT_TYPE = "elephant-container-constrained-result-v1";
export const REMOTE_POLICY_REPORT_TYPE = "elephant-remote-policy-result-v1";

export const PRIMARY_PROBE_SCENARIOS = [
  "classic-script",
  "document-module-static-import",
  "document-module-dynamic-import",
  "relative-fetch",
  "root-relative-fetch",
  "remote-fetch",
  "dedicated-module-worker",
  "dedicated-worker-teardown",
  "shared-worker",
  "service-worker-registration",
  "stylesheet",
  "image",
  "scripted-svg",
  "wav-range",
  "local-storage",
  "indexed-db",
  "cookie",
  "document-domain-relaxation",
  "reader-isolation",
  "native-capability-isolation",
  "popup",
  "probe-complete",
] as const;

export const PEER_PROBE_SCENARIOS = [
  "peer-local-storage",
  "peer-indexed-db",
  "peer-cookie",
  "peer-worker-teardown",
  "peer-complete",
] as const;

const PROBE_SCENARIOS = [...PRIMARY_PROBE_SCENARIOS, ...PEER_PROBE_SCENARIOS] as const;
export const PROBE_OUTCOMES = ["passed", "failed", "blocked", "unsupported"] as const;

export type ProbeScenario = (typeof PROBE_SCENARIOS)[number];
export type ProbeOutcome = (typeof PROBE_OUTCOMES)[number];
export type ProbeSource = "probe.xhtml" | "peer.xhtml";
export type ProbeEvidenceValue = string | number | boolean | null;
