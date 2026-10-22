import { nativeRequest } from "../host/requests";
import { runnerContract, type RunnerResultContract } from "./contracts";
import { hasExactKeys } from "./guards";

export interface RunnerFixtureDescriptor {
  readonly id: string;
  readonly label: string;
  readonly resultContract: RunnerResultContract;
  readonly expectedTotal: number;
  readonly expectedSha256: string;
  readonly blake3: string;
  readonly policyProfile: "active-enabled-offline-v1";
}

export interface RunnerFixture {
  readonly descriptor: RunnerFixtureDescriptor;
  readonly notice: string;
  /** The observation plan an evidence run was started with, or null. */
  readonly observation: unknown;
}

/**
 * Which fixture this process runs, from the one route the runner build adds for it. The
 * first call also makes the host offer the fixture file, exactly as the platform would hand
 * over a book.
 */
export async function loadFixture(sessionToken: string): Promise<RunnerFixture> {
  const response = await nativeRequest("/native/conformance/fixture", {
    sessionToken,
    trusted: true,
  });
  if (!response.ok) throw new Error(`Fixture route answered HTTP ${response.status}`);
  const value: unknown = await response.json();
  if (
    !hasExactKeys(value, new Set(["descriptor", "notice", "observation"])) ||
    !isRunnerFixtureDescriptor(value.descriptor) ||
    typeof value.notice !== "string"
  )
    throw new Error("Fixture descriptor is invalid");
  return value as unknown as RunnerFixture;
}

export function isRunnerFixtureDescriptor(value: unknown): value is RunnerFixtureDescriptor {
  return (
    hasExactKeys(
      value,
      new Set([
        "id",
        "label",
        "resultContract",
        "expectedTotal",
        "expectedSha256",
        "blake3",
        "policyProfile",
      ]),
    ) &&
    typeof value.id === "string" &&
    /^[a-z0-9][a-z0-9_-]{0,79}$/.test(value.id) &&
    runnerContract(value.resultContract) !== null &&
    Number.isInteger(value.expectedTotal) &&
    Number(value.expectedTotal) >= (value.resultContract === "evidence-v1" ? 0 : 1) &&
    Number(value.expectedTotal) <= (value.resultContract === "evidence-v1" ? 0 : 64) &&
    typeof value.label === "string" &&
    value.label.length > 0 &&
    value.label.length <= 80 &&
    typeof value.expectedSha256 === "string" &&
    /^[0-9a-f]{64}$/.test(value.expectedSha256) &&
    typeof value.blake3 === "string" &&
    /^[0-9a-f]{64}$/.test(value.blake3) &&
    value.policyProfile === "active-enabled-offline-v1"
  );
}
