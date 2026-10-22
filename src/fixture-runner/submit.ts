import type { EvidenceSubmission } from "./evidence";
import { captureGeometry } from "./frames";
import type { PublicationObservation, RunnerCheck } from "./reports";

export async function authenticatedSubmit(
  token: string,
  checks: readonly RunnerCheck[],
  capture: ReturnType<typeof captureGeometry>,
  publicationObservations: readonly PublicationObservation[],
  signal?: AbortSignal,
): Promise<void> {
  const response = await fetch("/native/conformance", {
    method: "POST",
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    ...(signal === undefined ? {} : { signal }),
    body: JSON.stringify({
      passed: checks.filter((check) => check.passed).length,
      total: checks.length,
      failures: checks.filter((check) => !check.passed).map((check) => check.label),
      capture,
      publicationObservations,
    }),
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new Error(
      `Terminal summary submission failed with HTTP ${response.status}${detail === "" ? "" : `: ${detail}`}`,
    );
  }
}

export async function authenticatedReportRunnerError(
  token: string,
  message: string,
): Promise<void> {
  await fetch("/native/client-error", {
    method: "POST",
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ message: message.slice(0, 1000) }),
  });
}

export async function authenticatedSubmitIncomplete(
  token: string,
  expectedTotal: number,
  reason: string,
): Promise<void> {
  const response = await fetch("/native/conformance", {
    method: "POST",
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      passed: 0,
      total: expectedTotal,
      failures: ["runner-execution"],
      capture: null,
      publicationObservations: [],
      incompleteReason: reason.slice(0, 1000),
    }),
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new Error(
      `Incomplete result submission failed with HTTP ${response.status}${detail === "" ? "" : `: ${detail}`}`,
    );
  }
}

export async function authenticatedSubmitEvidence(
  token: string,
  evidence: EvidenceSubmission,
  signal?: AbortSignal,
): Promise<void> {
  const response = await fetch("/native/conformance/evidence", {
    method: "POST",
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    ...(signal === undefined ? {} : { signal }),
    body: JSON.stringify(evidence),
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new Error(
      `Evidence submission failed with HTTP ${response.status}${detail === "" ? "" : `: ${detail}`}`,
    );
  }
}

export async function authenticatedSubmitIncompleteEvidence(
  token: string,
  reason: string,
): Promise<void> {
  const response = await fetch("/native/conformance/evidence", {
    method: "POST",
    cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify({
      runner: {},
      publication: {},
      documents: [],
      incompleteReason: reason.slice(0, 1000),
    }),
  });
  if (!response.ok) {
    const detail = (await response.text()).trim();
    throw new Error(
      `Incomplete evidence submission failed with HTTP ${response.status}${detail === "" ? "" : `: ${detail}`}`,
    );
  }
}
