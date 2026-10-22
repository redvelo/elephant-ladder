import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { expect, test } from "vitest";

import { readCatalog, UPSTREAM_COMMIT } from "../../scripts/build-w3c-corpus.mjs";
import { selectTests } from "../../scripts/collect-w3c-corpus-linux.mjs";
import {
  buildReport,
  DEFAULT_OBSERVATION,
  judge,
  readAdjudications,
  readExpectations,
  recordedSurfaces,
  untestedCount,
  validateExpectation,
} from "../../scripts/w3c-corpus.mjs";

function evidence(documents: unknown[], publication: Record<string, unknown> = {}) {
  return { publication, documents };
}

function observed(queries: unknown[], extra: Record<string, unknown> = {}) {
  return {
    occurrence: 0,
    settled: true,
    observation: {
      document: { contentType: "application/xhtml+xml", lang: "en", dir: "", title: "T" },
      queries,
      ...extra,
    },
  };
}

test("the committed catalog names every upstream test once at the pinned commit", () => {
  const catalog = readCatalog();
  expect(catalog.upstream.commit).toBe(UPSTREAM_COMMIT);
  const ids = catalog.tests.map((test: { id: string }) => test.id);
  expect(new Set(ids).size).toBe(ids.length);
  expect(ids).toStrictEqual([...ids].sort((left, right) => (left < right ? -1 : 1)));
  for (const test of catalog.tests) {
    expect(test.sha256).toMatch(/^[0-9a-f]{64}$/u);
    expect(test.id).toMatch(/^[a-z0-9][a-z0-9_-]{0,79}$/u);
    expect(test.template).toBe(test.id.startsWith("xx-"));
  }
});

test("the expectation table and adjudication store validate and name only catalog tests", () => {
  const catalog = readCatalog();
  const ids = new Set(catalog.tests.map((test: { id: string }) => test.id));
  const expectations = readExpectations();
  for (const id of Object.keys(expectations.tests)) expect(ids.has(id), id).toBe(true);
  const adjudications = readAdjudications();
  for (const id of Object.keys(adjudications.tests)) expect(ids.has(id), id).toBe(true);
  expect(() => validateExpectation("x", { verdict: { nope: {} } })).toThrow(/unknown operator/u);
  expect(() => validateExpectation("x", { verdict: { all: [] } })).toThrow(/non-empty/u);
  expect(() => validateExpectation("x", { verdict: { same: [{}] } })).toThrow(/at least two/u);
  expect(() => validateExpectation("x", { extra: 1 })).toThrow(/unknown field/u);
  expect(DEFAULT_OBSERVATION.queries.length).toBeGreaterThan(0);
});

test("judge decides from query text, styles, resources, fonts, and paths", () => {
  const run = evidence(
    [
      observed(
        [
          { id: "r", matches: 1, first: { text: "Test passes.", styles: { display: "block" } } },
          { id: "none", matches: 0, first: null },
          { id: "box", matches: 1, first: { rect: { x: 0, y: 0, width: 300, height: 40 } } },
        ],
        {
          resources: [{ kind: "image", source: "img/001.png", state: "loaded" }],
          fonts: { status: "loaded", loaded: ['"Lobster"'] },
        },
      ),
    ],
    { kernel: { readingOrderLength: 1 } },
  );
  const verdict = (rule: unknown) => judge({ verdict: rule }, run).verdict;
  expect(verdict({ query: { document: 0, id: "r", text: { includes: "passes" } } })).toBe(true);
  expect(verdict({ query: { document: 0, id: "r", text: { matches: "^Test" } } })).toBe(true);
  expect(verdict({ query: { document: 0, id: "r", style: { display: "none" } } })).toBe(false);
  expect(verdict({ query: { document: 0, id: "none", matches: { max: 0 } } })).toBe(true);
  expect(verdict({ query: { document: 0, id: "none", text: { equals: "x" } } })).toBe(false);
  expect(verdict({ query: { document: 0, id: "box", rect: { height: { min: 1 } } } })).toBe(true);
  expect(verdict({ query: { document: 0, id: "missing", text: { equals: "x" } } })).toBeNull();
  expect(verdict({ resource: { document: 0, source: "001.png", state: "loaded" } })).toBe(true);
  expect(verdict({ resource: { document: 0, source: "001.png", state: "failed" } })).toBe(false);
  expect(verdict({ font: { document: 0, loaded: "Lobster" } })).toBe(true);
  expect(verdict({ font: { document: 0, loaded: "Macondo" } })).toBe(false);
  expect(verdict({ settled: { document: 0 } })).toBe(true);
  expect(verdict({ settled: { document: 1 } })).toBeNull();
  expect(verdict({ document: { document: 0, field: "lang", equals: "en" } })).toBe(true);
  expect(verdict({ path: { path: "publication.kernel.readingOrderLength", equals: 1 } })).toBe(
    true,
  );
  expect(verdict({ path: { document: 0, path: "settled", equals: true } })).toBe(true);
  expect(
    verdict({ path: { document: 0, path: "fixedCompanion.placement", equals: "left" } }),
  ).toBeNull();
  expect(
    judge(
      { verdict: { path: { document: 0, path: "fixedCompanion.placement", equals: "left" } } },
      evidence([{ occurrence: 0, fixedCompanion: null }]),
    ).verdict,
  ).toBe(false);
  expect(verdict({ path: { path: "runner.frame.landscape", exists: true } })).toBe(false);
  const gated = (premise: unknown) => ({
    when: { premise, rule: { path: { document: 0, path: "settled", equals: false } } },
  });
  expect(verdict(gated({ path: { path: "runner.frame.landscape", equals: true } }))).toBeNull();
  expect(verdict(gated({ settled: { document: 0 } }))).toBe(false);
  expect(() => validateExpectation("x", { verdict: { when: { premise: {} } } })).toThrow(/when/u);
  expect(
    verdict({
      all: [
        { settled: { document: 0 } },
        { not: { query: { document: 0, id: "r", text: { includes: "fails" } } } },
      ],
    }),
  ).toBe(true);
  expect(
    verdict({ any: [{ settled: { document: 1 } }, { font: { document: 0, loaded: "Lobster" } }] }),
  ).toBe(true);
  expect(
    verdict({ any: [{ settled: { document: 1 } }, { font: { document: 0, loaded: "X" } }] }),
  ).toBeNull();
});

test("judge compares places across documents and honours adjudicate and n/a rows", () => {
  const run = evidence([
    observed([{ id: "origin", matches: 1, first: { text: "http://p-a.localhost:1" } }]),
    {
      ...observed([{ id: "origin", matches: 1, first: { text: "http://p-a.localhost:1" } }]),
      occurrence: 1,
    },
  ]);
  expect(
    judge(
      {
        verdict: {
          same: [
            { document: 0, query: "origin" },
            { document: 1, query: "origin" },
          ],
        },
      },
      run,
    ).verdict,
  ).toBe(true);
  expect(
    judge(
      { verdict: { same: [{ document: 0, query: "origin" }, { path: "documents.1.settled" }] } },
      run,
    ).verdict,
  ).toBe(false);
  expect(judge({ verdict: "adjudicate" }, run).verdict).toBeNull();
  expect(judge({ verdict: "n/a", note: "no playback" }, run)).toStrictEqual({
    verdict: "n/a",
    reason: "no playback",
  });
  expect(judge(undefined, run).verdict).toBeNull();
});

test("the upstream report carries decided and adjudicated tests only, never templates or deprecated tests", () => {
  const catalog = {
    tests: [
      { id: "a", template: false, collections: [] },
      { id: "b", template: false, collections: ["should"] },
      { id: "c", template: false, collections: [] },
      { id: "d", template: false, collections: ["deprecated"] },
      { id: "e", template: false, collections: [] },
      { id: "f", template: false, collections: [] },
      { id: "xx-t", template: true, collections: [] },
    ],
  };
  const report = buildReport({
    name: "Elephant Ladder",
    ref: "https://example.test",
    variant: "Linux",
    verdicts: { a: true, b: null, d: true, e: null, f: null, "xx-t": true },
    adjudications: {
      tests: { b: { result: "n/a" }, c: { result: false }, e: { result: "todo" } },
    },
    catalog,
  });
  expect(report).toStrictEqual({
    name: "Elephant Ladder",
    ref: "https://example.test",
    variant: "Linux",
    "tested-by": "implementer",
    tests: { a: true, b: "n/a", c: false },
  });
  // A deliberate `todo` and a test nobody has judged are the same absence upstream.
  expect(untestedCount(report, catalog)).toBe(2);
});

test("a default selection leaves out templates and the deprecated collection", () => {
  const catalog = readCatalog();
  const all = selectTests(catalog, []);
  expect(all.length).toBe(179);
  expect(all.some((test: { id: string }) => test.id.startsWith("lay-fxl-"))).toBe(false);
  expect(selectTests(catalog, ["--deprecated"]).length).toBe(206);
  expect(
    selectTests(catalog, ["lay-fxl-spread-auto"]).map((test: { id: string }) => test.id),
  ).toStrictEqual(["lay-fxl-spread-auto"]);
  expect(selectTests(catalog, ["--coverage", "Pre-paginated Layout"]).length).toBe(25);
  expect(() => selectTests(catalog, ["nope"])).toThrow(/Unknown W3C test id/u);
});

test("a run is only comparable evidence while every test recorded one surface", () => {
  const run = mkdtempSync(resolve(tmpdir(), "elephant-w3c-surfaces-"));
  const frames: Record<string, { width: number; height: number } | null> = {
    wide: { width: 892, height: 403 },
    "also-wide": { width: 892, height: 403 },
    stretched: { width: 892, height: 6497 },
    unrecorded: null,
  };
  writeFileSync(
    resolve(run, "manifest.json"),
    JSON.stringify({ runs: Object.keys(frames).map((id) => ({ id })) }),
  );
  for (const [id, frame] of Object.entries(frames)) {
    mkdirSync(resolve(run, id));
    if (frame === null) continue;
    writeFileSync(
      resolve(run, id, "fixture-evidence.json"),
      JSON.stringify({ runner: { frame }, documents: [] }),
    );
  }
  expect([...recordedSurfaces(run)]).toStrictEqual([
    ["892x403", ["wide", "also-wide"]],
    ["892x6497", ["stretched"]],
  ]);
});
