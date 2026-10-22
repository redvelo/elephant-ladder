import {
  classifyRunnerResult,
  validatePublicationObservationsArtifact,
} from "./fixture-runner-contract.mjs";

export const NATIVE_DURABLE_STORAGE_SCHEMA = "elephant-native-durable-storage-linux-evidence-v3";

export function evaluateNativeRestartRuns(runs) {
  if (!Array.isArray(runs) || runs.length !== 2) {
    throw new Error("Native durable-storage evidence requires exactly two process runs");
  }
  const expected = [
    { local: [[null, "1"], ["1", "2"], "2"], indexedDb: [[null, 1], [1, 2], 2] },
    { local: [["2", "3"], ["3", "4"], "4"], indexedDb: [[2, 3], [3, 4], 4] },
  ];
  let publicationOrigin = null;
  const observations = runs.map((run, index) => {
    const terminal = run?.terminal;
    if (terminal?.runStatus !== "complete" || classifyRunnerResult(terminal) !== "automated-pass") {
      throw new Error(`Native process ${index + 1} did not produce a passing terminal result`);
    }
    const artifact = validatePublicationObservationsArtifact(run.observations, terminal);
    const phases = artifact.phases;
    const origin = artifact.fixture.publicationOrigin;
    if (publicationOrigin === null) publicationOrigin = origin;
    else if (publicationOrigin !== origin)
      throw new Error("Publication origin changed across native process restart");

    const local = [
      transition(phases[0], "local-storage"),
      transition(phases[1], "local-storage"),
      value(phases[2], "peer-local-storage"),
    ];
    const indexedDb = [
      transition(phases[0], "indexed-db"),
      transition(phases[1], "indexed-db"),
      value(phases[2], "peer-indexed-db"),
    ];
    if (
      JSON.stringify(local) !== JSON.stringify(expected[index].local) ||
      JSON.stringify(indexedDb) !== JSON.stringify(expected[index].indexedDb)
    ) {
      throw new Error(
        `Native process ${index + 1} storage sequence does not prove restart continuity`,
      );
    }
    return { processOrdinal: index + 1, localStorage: local, indexedDb };
  });
  return { publicationOrigin, observations };
}

function transition(phase, scenario) {
  const evidence = reportEvidence(phase, scenario);
  if (!Object.hasOwn(evidence, "previous") || !Object.hasOwn(evidence, "current")) {
    throw new Error(`${scenario} transition evidence is incomplete`);
  }
  return [evidence.previous, evidence.current];
}

function value(phase, scenario) {
  const evidence = reportEvidence(phase, scenario);
  if (!Object.hasOwn(evidence, "value"))
    throw new Error(`${scenario} value evidence is incomplete`);
  return evidence.value;
}

function reportEvidence(phase, scenario) {
  if (typeof phase !== "object" || phase === null || !Array.isArray(phase.reports))
    throw new Error("Native publication phase is malformed");
  const reports = phase.reports.filter((report) => report?.scenario === scenario);
  if (
    reports.length !== 1 ||
    reports[0].outcome !== "passed" ||
    typeof reports[0].evidence !== "object" ||
    reports[0].evidence === null ||
    Array.isArray(reports[0].evidence)
  ) {
    throw new Error(`${scenario} report is malformed`);
  }
  return reports[0].evidence;
}
