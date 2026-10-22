import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { unzipSync, strFromU8 } from "fflate";
import { expect, test } from "vitest";
import {
  parseStaticFxlSnapshot,
  evaluateStaticFxlSnapshot,
} from "../../src/fixture-runner/static-fxl";
import { type StaticFxlSnapshot } from "../../src/fixture-runner/reports";
import {
  parseFixtureIds,
  createRunnerEventCollector,
} from "../../scripts/fixture-runner-contract.mjs";

function rect(x: number, y: number, width: number, height: number) {
  return { x, y, width, height, top: y, left: x, right: x + width, bottom: y + height };
}

function snapshot(phase: 0 | 1, rtl = false, mixed = false): StaticFxlSnapshot {
  return {
    kind: "elephant-static-fxl-parent-v1",
    documentGeneration: 3,
    primary: {
      occurrence: phase,
      resource: phase === 0 ? "EPUB/center.xhtml" : "EPUB/first.xhtml",
      adapter: "fixed-xhtml",
      frameGeneration: 3,
      layoutGeneration: 2,
      rect: rect(phase === 0 ? 150 : rtl ? 300 : 0, 0, 300, 400),
    },
    companion:
      phase === 0
        ? null
        : {
            occurrence: 2,
            resource: mixed ? "EPUB/second.svg" : "EPUB/second.xhtml",
            adapter: mixed ? "fixed-svg" : "fixed-xhtml",
            frameGeneration: 4,
            layoutGeneration: 2,
            ownerDocumentGeneration: 3,
            placement: rtl ? "left" : "right",
            rect: rect(rtl ? 0 : 300, 0, 300, 400),
          },
    visible: {
      primaryOccurrence: phase,
      firstOccurrence: phase,
      lastOccurrence: phase === 0 ? 0 : 2,
      companion: phase === 0 ? null : { occurrence: 2, placement: rtl ? "left" : "right" },
    },
    hostRect: rect(0, 0, 600, 400),
  };
}

test("static parent evidence is strictly parsed and evaluates observed sides/seams, not expected mock data", () => {
  for (const id of ["static-fxl-ltr", "static-fxl-rtl", "static-fxl-mixed-svg"]) {
    const checks = ([0, 1] as const).flatMap((phase) => {
      const value = snapshot(phase, id.endsWith("rtl"), id.endsWith("svg"));
      expect(parseStaticFxlSnapshot(value)).toEqual(value);
      return evaluateStaticFxlSnapshot(value, id, phase);
    });
    expect(checks).toHaveLength(8);
    expect(new Set(checks.map((check) => check.label)).size).toBe(8);
    expect(checks.every((check) => check.passed)).toBe(true);
  }
  const value = snapshot(1, false, true);
  for (const malformed of [
    { ...value, extra: true },
    { ...value, primary: { ...value.primary, adapter: "fixed-image" } },
    { ...value, hostRect: { ...value.hostRect, width: Infinity } },
    { ...value, visible: null },
    { ...value, companion: { ...value.companion, placement: "unknown" } },
  ])
    expect(parseStaticFxlSnapshot(malformed)).toBeNull();
  const moved = { ...value, companion: { ...value.companion!, rect: rect(310, 0, 300, 400) } };
  expect(
    evaluateStaticFxlSnapshot(moved, "static-fxl-mixed-svg", 1).find((check) =>
      check.label.endsWith("geometry"),
    )?.passed,
  ).toBe(false);
  expect(
    evaluateStaticFxlSnapshot({ ...value, companion: null }, "static-fxl-mixed-svg", 1).every(
      (check) => check.passed,
    ),
  ).toBe(false);
});

test("static archives, IDs, notice hashes and terminal counts are registered coherently", () => {
  execFileSync(process.execPath, [
    "tests/fixtures/active-publications/build-static-fxl.mjs",
    "--check",
  ]);
  const notice = readFileSync(
    new URL("../fixtures/active-publications/NOTICE.md", import.meta.url),
    "utf8",
  );
  const digests = [
    "2deec1b53dc04bffa8f6a9b97cc06a7754f0a502258bb6997a29fb3e590263d0",
    "2185a2ce98eedab16e02ba7f56ecf52ee77d0f1be22039efe64af324a3c57211",
    "42c9a9d713ab874f28d49c4636380c7616496d821771422a8492bf32cec2833d",
  ];
  const ids = ["static-fxl-ltr", "static-fxl-rtl", "static-fxl-mixed-svg"];
  expect(parseFixtureIds(ids)).toEqual(ids);
  for (const [index, id] of ids.entries()) {
    const bytes = readFileSync(
      new URL(`../fixtures/active-publications/${id}.epub`, import.meta.url),
    );
    const sha = createHash("sha256").update(bytes).digest("hex");
    expect(notice).toContain(sha);
    expect(notice).toContain(digests[index]);
    const files = unzipSync(bytes);
    expect(Object.keys(files)[0]).toBe("mimetype");
    expect(strFromU8(files.mimetype!)).toBe("application/epub+zip");
    for (const content of Object.values(files))
      expect(strFromU8(content)).not.toMatch(/<script\b|\bscripted\b|\bonload\s*=/i);
    const terminal = {
      event: "fixture_run_finished",
      fixture_id: id,
      fixture_blake3: digests[index],
      fixture_sha256: sha,
      result_contract: "static-fxl-v1",
      policy_profile: "active-enabled-offline-v1",
      run_status: "complete",
      publication_origin: "http://p-0123456789abcdef0123456789abcdef.localhost:43123",
      passed: 8,
      total: 8,
      failure_count: 0,
      failures: "[]",
      publication_observations_file: "fixture-publication-observations.json",
      publication_observations_sha256: "a".repeat(64),
      publication_observations_bytes: 1000,
      publication_observations_phase_count: 2,
    };
    expect(createRunnerEventCollector(id).consume(JSON.stringify(terminal)).total).toBe(8);
    expect(() =>
      createRunnerEventCollector(id).consume(JSON.stringify({ ...terminal, total: 7, passed: 7 })),
    ).toThrow();
    expect(() =>
      createRunnerEventCollector(id).consume(
        JSON.stringify({ ...terminal, publication_observations_phase_count: 1 }),
      ),
    ).toThrow();
  }
});
