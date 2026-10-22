import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { expect, test } from "vitest";

import {
  classifyRunnerResult,
  createRunnerEventCollector,
  evaluateStorageIsolationRelation,
  parseFixtureIds,
  validatePublicationObservationsArtifact,
} from "../../scripts/fixture-runner-contract.mjs";
import { relativeAndroidFixtureArtifact } from "../../scripts/collect-fixture-runner-android.mjs";

test.each(["linux", "android"])(
  "%s collector preserves prior and flat evidence on preflight failure",
  (platform) => {
    const sandbox = mkdtempSync(resolve(tmpdir(), "elephant-collector-test-"));
    try {
      const scripts = resolve(sandbox, "scripts");
      const outputRoot = resolve(sandbox, "test-results/fixture-runner", platform);
      mkdirSync(scripts);
      mkdirSync(resolve(outputRoot, "old-run", "active-probe"), { recursive: true });
      mkdirSync(resolve(outputRoot, "active-probe"));
      const sentinels = [
        "manifest.json",
        "active-probe/manifest.json",
        "old-run/active-probe/manifest.json",
      ];
      for (const file of sentinels)
        writeFileSync(resolve(outputRoot, file), `historical ${file}\n`);
      for (const file of [
        `collect-fixture-runner-${platform}.mjs`,
        "fixture-runner-contract.mjs",
        ...(platform === "android" ? ["android-fixture-runner.mjs"] : []),
      ]) {
        copyFileSync(new URL(`../../scripts/${file}`, import.meta.url), resolve(scripts, file));
      }

      const destinations = new Set<string>();
      for (const fixtureId of ["active-probe", "active-probe", "../invalid"]) {
        // No real app, adb, build tools, or device can be reached in this sandbox.
        const result = spawnSync(
          process.execPath,
          [resolve(scripts, `collect-fixture-runner-${platform}.mjs`), fixtureId],
          {
            cwd: sandbox,
            env: {
              ...process.env,
              PATH: scripts,
              ELEPHANT_QUALIFICATION_BINARY: resolve(sandbox, "missing-binary"),
            },
            encoding: "utf8",
            timeout: 10_000,
          },
        );
        expect(result.error).toBeUndefined();
        expect(result.status).toBe(1);
        const destination = result.stdout.match(/evidence: (.+)\n/u)?.[1];
        expect(destination).toBeDefined();
        expect(resolve(destination!, "..")).toBe(outputRoot);
        expect(basename(destination!)).toMatch(/^\d{4}-\d{2}-\d{2}T[\d.-]+Z-[A-Za-z0-9]{6}$/u);
        destinations.add(destination!);
        const manifest = JSON.parse(readFileSync(resolve(destination!, "manifest.json"), "utf8"));
        expect(manifest.runs).toStrictEqual([]);
        expect(manifest.relations).toStrictEqual([]);
        expect(manifest.suiteError).toMatch(
          fixtureId === "../invalid"
            ? /Unknown fixture id/u
            : platform === "linux"
              ? /application is missing|requires Linux/u
              : /adb devices failed/u,
        );
        if (platform === "android") {
          expect(manifest.cleanup.status).toBe("not-needed");
          expect(manifest.localProductArtifact.status).toBe("not-needed");
        }
        for (const file of sentinels)
          expect(readFileSync(resolve(outputRoot, file), "utf8")).toBe(`historical ${file}\n`);
      }
      expect(destinations.size).toBe(3);
      expect(readdirSync(outputRoot)).toHaveLength(6);
      for (const destination of destinations) {
        expect(readdirSync(destination)).toStrictEqual(["manifest.json"]);
        for (const child of ["active-probe", "relations/storage-isolation/primary-first"]) {
          const childOutput = resolve(destination, child);
          const artifact = relativeAndroidFixtureArtifact(
            { file: "fixture-runner.apk" },
            childOutput,
            destination,
          );
          expect(resolve(childOutput, artifact.file)).toBe(
            resolve(destination, "fixture-runner.apk"),
          );
        }
      }
    } finally {
      rmSync(sandbox, { recursive: true, force: true });
    }
  },
);

const capture = {
  viewportWidth: 1100,
  viewportHeight: 760,
  x: 24,
  y: 112,
  width: 573,
  height: 560,
};
const completeEvent = {
  event: "fixture_run_finished",
  run_status: "complete",
  fixture_id: "active-probe",
  fixture_blake3: "0302f52dedeb27cf981b51f885805d671bae7160bc0207efb9174739673803dc",
  fixture_sha256: "52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24",
  result_contract: "active-probe-v1",
  policy_profile: "active-enabled-offline-v1",
  passed: 37,
  total: 37,
  publication_origin: "http://p-0123456789abcdef0123456789abcdef.localhost:43123",
  failure_count: 0,
  failures: "[]",
  viewport_width: capture.viewportWidth,
  viewport_height: capture.viewportHeight,
  capture_x: capture.x,
  capture_y: capture.y,
  capture_width: capture.width,
  capture_height: capture.height,
  publication_observations_file: "fixture-publication-observations.json",
  publication_observations_sha256: "a".repeat(64),
  publication_observations_bytes: 4096,
  publication_observations_phase_count: 3,
};

function collect(event: Record<string, unknown> = completeEvent, requestedId = "active-probe") {
  return createRunnerEventCollector(requestedId).consume(JSON.stringify(event));
}

test("fixture selection confines output to one directory per allowlisted fixture", () => {
  expect(parseFixtureIds(["active-probe"])).toStrictEqual(["active-probe"]);
  expect(() => parseFixtureIds(["../output"])).toThrow(/Unknown fixture id/);
  expect(() => parseFixtureIds(["active-probe", "active-probe"])).toThrow(/Duplicate fixture id/);
  expect(parseFixtureIds(["static-fxl-ltr", "active-probe-twin"])).toStrictEqual([
    "static-fxl-ltr",
    "active-probe-twin",
  ]);
  expect(() => parseFixtureIds(["w3c-scr-support"])).toThrow(/Unknown fixture id/);
});

test("collector requires exact sibling storage isolation and primary resumption", () => {
  const relation = evaluateStorageIsolationRelation(
    storageArtifact("active-probe", [[null, 1], [1, 2], [2]]),
    storageArtifact("active-probe-twin", [[null, 1], [1, 2], [2]]),
    storageArtifact("active-probe", [[2, 3], [3, 4], [4]]),
  );
  expect(relation.status).toBe("automated-pass");
  expect(relation.checks).toHaveLength(12);
  expect(
    evaluateStorageIsolationRelation(
      storageArtifact("active-probe", [[null, 1], [1, 2], [2]]),
      storageArtifact("active-probe-twin", [[2, 3], [3, 4], [4]]),
      storageArtifact("active-probe", [[2, 3], [3, 4], [4]]),
    ).status,
  ).toBe("automated-fail");
  expect(
    evaluateStorageIsolationRelation(
      storageArtifact("active-probe", [[null, 1], [1, 2], [2]]),
      storageArtifact("active-probe-twin", [[null, 1], [1, 2], [2]], [[2, 3], [3, 4], [4]]),
      storageArtifact("active-probe", [[2, 3], [3, 4], [4]]),
    ).checks.find((check: { label: string }) => check.label === "cookie-twin-isolated")?.passed,
  ).toBe(false);
  expect(
    evaluateStorageIsolationRelation(
      storageArtifact("active-probe", [[null, 1], [1, 2], [2]], [[null, 1], [null, 1], [null]]),
      storageArtifact(
        "active-probe-twin",
        [[null, 1], [1, 2], [2]],
        [[null, 1], [null, 1], [null]],
      ),
      storageArtifact("active-probe", [[2, 3], [3, 4], [4]], [[null, 1], [null, 1], [null]]),
    ).status,
  ).toBe("automated-pass");
});

test("collector accepts complete and incomplete native terminal results", () => {
  const complete = collect();
  expect(complete?.capture).toStrictEqual(capture);
  expect(classifyRunnerResult(complete)).toBe("automated-pass");

  const incomplete = collect({
    ...completeEvent,
    run_status: "incomplete",
    passed: 0,
    failure_count: 1,
    failures: '["runner-execution"]',
    viewport_width: undefined,
    viewport_height: undefined,
    capture_x: undefined,
    capture_y: undefined,
    capture_width: undefined,
    capture_height: undefined,
    publication_observations_file: undefined,
    publication_observations_sha256: undefined,
    publication_observations_bytes: undefined,
    publication_observations_phase_count: undefined,
  });
  expect(incomplete?.capture).toBeNull();
  expect(classifyRunnerResult(incomplete)).toBe("automated-fail");
});

test("collector rejects malformed or misbound terminal results", () => {
  const invalid: readonly [Record<string, unknown>, RegExp][] = [
    [{ fixture_blake3: "bad" }, /digest/],
    [{ fixture_sha256: undefined }, /identity/],
    [{ passed: 17 }, /counts/],
    [{ total: 65, passed: 65 }, /counts/],
    [{ failure_count: 1 }, /counts|failures/],
    [{ capture_width: 0 }, /capture/],
    [{ publication_observations_file: "other.json" }, /observations/],
    [{ publication_observations_bytes: 0 }, /observations/],
    [{ fixture_id: "active-probe-twin" }, /identity mismatch/],
    [{ fixture_blake3: "a".repeat(64) }, /BLAKE3/],
    [{ fixture_sha256: "a".repeat(64) }, /SHA-256/],
    [{ result_contract: "other" }, /result contract/],
    [{ total: 22, passed: 22 }, /total/],
    [{ policy_profile: "other" }, /policy profile/],
    [{ publication_origin: "p-1.localhost:43123" }, /origin/],
    [{ publication_origin: undefined }, /origin/],
  ];
  for (const [override, message] of invalid) {
    expect(() => collect({ ...completeEvent, ...override })).toThrow(message);
  }
});

test("collector binds observations and accepts exactly one terminal result", () => {
  const collector = createRunnerEventCollector("active-probe");
  expect(collector.consume("ordinary output")).toBeNull();
  const terminal = collector.consume(JSON.stringify(completeEvent))!;
  const artifact = {
    schema: "elephant-publication-observations-v2",
    fixture: {
      id: terminal.fixtureId,
      sha256: terminal.fixtureSha256,
      blake3: terminal.fixtureBlake3,
      resultContract: terminal.resultContract,
      policyProfile: terminal.policyProfile,
      publicationOrigin: terminal.publicationOrigin,
    },
    phases: [{}, {}, {}],
  };
  expect(validatePublicationObservationsArtifact(artifact, terminal)).toBe(artifact);
  expect(() =>
    validatePublicationObservationsArtifact({ ...artifact, schema: "other" }, terminal),
  ).toThrow(/identity/);
  expect(() =>
    validatePublicationObservationsArtifact({ ...artifact, phases: [{}] }, terminal),
  ).toThrow(/identity/);
  expect(() =>
    validatePublicationObservationsArtifact(
      {
        ...artifact,
        fixture: { ...artifact.fixture, id: "active-probe-twin" },
      },
      terminal,
    ),
  ).toThrow(/identity/);
  expect(() => collector.consume(JSON.stringify(completeEvent))).toThrow(/Duplicate terminal/);
});

function storageArtifact(
  id: string,
  values: readonly [readonly [number | null, number], readonly [number, number], readonly [number]],
  cookieValues: readonly [
    readonly [number | null, number],
    readonly [number | null, number],
    readonly [number | null],
  ] = values,
) {
  const scenarios = (
    previous: number | null,
    current: number,
    cookiePrevious: number | null,
    cookieCurrent: number,
  ) => [
    {
      scenario: "local-storage",
      evidence: { previous: previous === null ? null : String(previous), current: String(current) },
    },
    { scenario: "indexed-db", evidence: { previous, current } },
    {
      scenario: "cookie",
      evidence: {
        previous: cookiePrevious === null ? null : String(cookiePrevious),
        current: String(cookieCurrent),
      },
    },
  ];
  const origin = `http://p-${id}.localhost:43123`;
  return {
    schema: "elephant-publication-observations-v2",
    fixture: { id, publicationOrigin: origin },
    phases: [
      {
        reports: scenarios(values[0][0], values[0][1], cookieValues[0][0], cookieValues[0][1]),
      },
      {
        reports: scenarios(values[1][0], values[1][1], cookieValues[1][0], cookieValues[1][1]),
      },
      {
        reports: [
          { scenario: "peer-local-storage", evidence: { value: String(values[2][0]) } },
          { scenario: "peer-indexed-db", evidence: { value: values[2][0] } },
          {
            scenario: "peer-cookie",
            evidence: { value: cookieValues[2][0] === null ? null : String(cookieValues[2][0]) },
          },
        ],
      },
    ],
  };
}
