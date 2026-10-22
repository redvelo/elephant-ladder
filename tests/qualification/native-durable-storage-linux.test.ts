import { expect, test } from "vitest";

import { evaluateNativeRestartRuns } from "../../scripts/native-durable-storage-linux.mjs";

const terminal = {
  fixtureId: "active-probe",
  runStatus: "complete",
  fixtureBlake3: "280c73da5f48add61ee295dec05e286c69b382810091579abcf3e6743dab26c9",
  fixtureSha256: "c9351ef5d4277e5506289734d339987e5d87e20de4312633a7d62798e74a96de",
  resultContract: "active-probe-v1",
  policyProfile: "active-enabled-offline-v1",
  publicationOrigin: "http://p-stable.localhost:43123",
  passed: 37,
  total: 37,
  failureCount: 0,
  failures: [],
  capture: { viewportWidth: 1100, viewportHeight: 760, x: 20, y: 100, width: 500, height: 500 },
  publicationObservations: {
    file: "fixture-publication-observations.json",
    sha256: "a".repeat(64),
    bytes: 1000,
    phaseCount: 3,
  },
};

test("native restart evidence requires continuous localStorage and IndexedDB values", () => {
  const runs = [
    run("http://p-stable.localhost:43123", null, 1),
    run("http://p-stable.localhost:43123", 2, 3),
  ];
  expect(evaluateNativeRestartRuns(runs)).toStrictEqual({
    publicationOrigin: "http://p-stable.localhost:43123",
    observations: [
      {
        processOrdinal: 1,
        localStorage: [[null, "1"], ["1", "2"], "2"],
        indexedDb: [[null, 1], [1, 2], 2],
      },
      {
        processOrdinal: 2,
        localStorage: [["2", "3"], ["3", "4"], "4"],
        indexedDb: [[2, 3], [3, 4], 4],
      },
    ],
  });
  const reset = structuredClone(runs);
  reset[1]!.observations.phases[0]!.reports[0]!.evidence = { previous: null, current: "1" };
  expect(() => evaluateNativeRestartRuns(reset)).toThrow(/restart continuity/);
  const moved = structuredClone(runs);
  moved[1]!.terminal = { ...terminal, publicationOrigin: "http://p-other.localhost:43123" };
  moved[1]!.observations.fixture.publicationOrigin = "http://p-other.localhost:43123";
  expect(() => evaluateNativeRestartRuns(moved)).toThrow(/publication origin/iu);
});

function run(origin: string, previous: number | null, current: number) {
  const second = current + 1;
  return {
    terminal: { ...terminal, publicationOrigin: origin },
    observations: {
      schema: "elephant-publication-observations-v2",
      fixture: {
        id: terminal.fixtureId,
        sha256: terminal.fixtureSha256,
        blake3: terminal.fixtureBlake3,
        resultContract: terminal.resultContract,
        policyProfile: terminal.policyProfile,
        publicationOrigin: origin,
      },
      phases: [
        phase([
          report("local-storage", {
            previous: previous === null ? null : String(previous),
            current: String(current),
          }),
          report("indexed-db", { previous, current }),
        ]),
        phase([
          report("local-storage", { previous: String(current), current: String(second) }),
          report("indexed-db", { previous: current, current: second }),
        ]),
        phase([
          report("peer-local-storage", { value: String(second) }),
          report("peer-indexed-db", { value: second }),
        ]),
      ],
    },
  };
}

function phase<Report>(reports: Report[]) {
  return { reports };
}

function report(scenario: string, evidence: unknown) {
  return { scenario, outcome: "passed", evidence };
}
