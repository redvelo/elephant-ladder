// The W3C corpus as the collectors see it: the committed catalog, the expectation
// table that turns one run's evidence into a verdict, and the upstream report shape.
// Nothing here runs a publication; everything here is a pure function of files.
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { readCatalog, W3C_CORPUS_SCHEMA } from "./build-w3c-corpus.mjs";

export { readCatalog, W3C_CORPUS_SCHEMA };

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const EXPECTATIONS_PATH = resolve(root, "tests/fixtures/w3c-corpus/expectations.json");
export const ADJUDICATIONS_PATH = resolve(root, "tests/fixtures/w3c-corpus/adjudications.json");
export const EXPECTATIONS_SCHEMA = "elephant-w3c-expectations-v1";
export const ADJUDICATIONS_SCHEMA = "elephant-w3c-adjudications-v1";
/**
 * The verdicts an adjudication may record. Upstream's report takes `true`, `false` and
 * `"n/a"`; `"todo"` is this store's own, for a feature we have decided is absent rather
 * than failing. The report leaves those tests out, which upstream renders as `todo`.
 */
export const VERDICTS = Object.freeze([true, false, "n/a", "todo"]);

/** What every document is asked unless the test's expectation row asks otherwise. */
export const DEFAULT_OBSERVATION = Object.freeze({
  queries: [
    { id: "body", selector: "body", text: true, styles: ["direction", "writing-mode"] },
    { id: "title", selector: "title", text: true },
  ],
  resources: true,
  fonts: true,
});

/** Upstream's `deprecated` collection: superseded tests its report generator leaves out. */
export function isDeprecated(test) {
  return test.collections.includes("deprecated");
}

export function readExpectations(path = EXPECTATIONS_PATH) {
  const value = JSON.parse(readFileSync(path, "utf8"));
  if (value.schema !== EXPECTATIONS_SCHEMA || !isRecord(value.tests))
    throw new Error(`${path} is not a ${EXPECTATIONS_SCHEMA} table`);
  for (const [id, row] of Object.entries(value.tests)) validateExpectation(id, row);
  return value;
}

export function readAdjudications(path = ADJUDICATIONS_PATH) {
  const value = JSON.parse(readFileSync(path, "utf8"));
  if (value.schema !== ADJUDICATIONS_SCHEMA || !isRecord(value.tests))
    throw new Error(`${path} is not a ${ADJUDICATIONS_SCHEMA} store`);
  for (const [id, row] of Object.entries(value.tests)) {
    if (
      !isRecord(row) ||
      !VERDICTS.includes(row.result) ||
      typeof row.evidence !== "string" ||
      typeof row.date !== "string" ||
      typeof row.note !== "string"
    )
      throw new Error(`adjudication ${id} must carry result, evidence, date, and note`);
  }
  return value;
}

export function validateExpectation(id, row) {
  if (!isRecord(row)) throw new Error(`expectation ${id} is not an object`);
  const keys = new Set(Object.keys(row));
  for (const key of keys) {
    if (!["observation", "verdict", "note"].includes(key))
      throw new Error(`expectation ${id} has an unknown field ${key}`);
  }
  if (row.observation !== undefined && !isRecord(row.observation))
    throw new Error(`expectation ${id} observation must be a plan object`);
  if (row.verdict === "adjudicate" || row.verdict === "n/a") return;
  validateRule(id, row.verdict);
}

function validateRule(id, rule) {
  if (!isRecord(rule) || Object.keys(rule).length !== 1)
    throw new Error(`expectation ${id} has a rule that is not exactly one operator`);
  const [operator, operand] = Object.entries(rule)[0];
  switch (operator) {
    case "all":
    case "any":
      if (!Array.isArray(operand) || operand.length === 0)
        throw new Error(`expectation ${id} ${operator} needs a non-empty list`);
      for (const inner of operand) validateRule(id, inner);
      return;
    case "not":
      validateRule(id, operand);
      return;
    case "query":
    case "document":
    case "resource":
    case "font":
    case "path":
    case "settled":
      if (!isRecord(operand)) throw new Error(`expectation ${id} ${operator} needs an object`);
      return;
    case "same":
      if (!Array.isArray(operand) || operand.length < 2 || !operand.every(isRecord))
        throw new Error(`expectation ${id} same needs at least two places to compare`);
      return;
    case "every":
      validateRule(id, operand);
      return;
    case "when":
      if (!isRecord(operand) || operand.premise === undefined || operand.rule === undefined)
        throw new Error(`expectation ${id} when needs a premise and a rule`);
      validateRule(id, operand.premise);
      validateRule(id, operand.rule);
      return;
    default:
      throw new Error(`expectation ${id} uses an unknown operator ${operator}`);
  }
}

/**
 * The verdict one test's evidence supports: `true`, `false`, `"n/a"`, or `null`
 * when the table leaves it to a person or the evidence cannot answer the rule.
 * Every outcome also says why, so a manifest reader can follow the judgement.
 */
export function judge(row, evidence) {
  if (row === undefined) return { verdict: null, reason: "no expectation row" };
  if (row.verdict === "adjudicate") return { verdict: null, reason: "left to adjudication" };
  if (row.verdict === "n/a") return { verdict: "n/a", reason: row.note ?? "not applicable" };
  const outcome = evaluate(row.verdict, evidence);
  return outcome.value === null
    ? { verdict: null, reason: outcome.reason }
    : { verdict: outcome.value, reason: outcome.reason };
}

function evaluate(rule, evidence) {
  const [operator, operand] = Object.entries(rule)[0];
  switch (operator) {
    case "all": {
      const outcomes = operand.map((inner) => evaluate(inner, evidence));
      const failed = outcomes.find((outcome) => outcome.value === false);
      if (failed !== undefined) return failed;
      const unknown = outcomes.find((outcome) => outcome.value === null);
      if (unknown !== undefined) return unknown;
      return { value: true, reason: outcomes.map((outcome) => outcome.reason).join("; ") };
    }
    case "any": {
      const outcomes = operand.map((inner) => evaluate(inner, evidence));
      const passed = outcomes.find((outcome) => outcome.value === true);
      if (passed !== undefined) return passed;
      const unknown = outcomes.find((outcome) => outcome.value === null);
      if (unknown !== undefined) return unknown;
      return { value: false, reason: outcomes.map((outcome) => outcome.reason).join("; ") };
    }
    case "not": {
      const outcome = evaluate(operand, evidence);
      return outcome.value === null
        ? outcome
        : { value: !outcome.value, reason: `not: ${outcome.reason}` };
    }
    case "settled": {
      const document = documentOf(evidence, operand.document);
      if (document === null) return { value: null, reason: `document ${operand.document} missing` };
      return {
        value: document.settled === true,
        reason: `document ${operand.document} ${document.settled ? "settled" : "did not settle"}`,
      };
    }
    case "document": {
      const document = documentOf(evidence, operand.document);
      if (document === null) return { value: null, reason: `document ${operand.document} missing` };
      if (document.observation === null)
        return { value: null, reason: `document ${operand.document} was not observed` };
      return compare(
        `document ${operand.document} ${operand.field}`,
        document.observation.document[operand.field],
        operand,
      );
    }
    case "query": {
      const document = documentOf(evidence, operand.document);
      if (document === null) return { value: null, reason: `document ${operand.document} missing` };
      if (document.observation === null)
        return { value: null, reason: `document ${operand.document} was not observed` };
      const result = document.observation.queries.find((query) => query.id === operand.id);
      if (result === undefined) return { value: null, reason: `query ${operand.id} not asked` };
      const label = `query ${operand.id}`;
      if (result.inaccessible !== undefined)
        return { value: null, reason: `${label} inaccessible (${result.inaccessible})` };
      if (operand.matches !== undefined) {
        const outcome = compare(`${label} matches`, result.matches, operand.matches);
        if (outcome.value !== true) return outcome;
      }
      if (result.first === null) {
        if (
          operand.matches !== undefined &&
          operand.text === undefined &&
          operand.style === undefined
        )
          return { value: true, reason: `${label} matched ${result.matches}` };
        return { value: false, reason: `${label} matched nothing` };
      }
      if (operand.text !== undefined) {
        const outcome = compare(`${label} text`, result.first.text, operand.text);
        if (outcome.value !== true) return outcome;
      }
      if (operand.style !== undefined) {
        for (const [property, expected] of Object.entries(operand.style)) {
          const outcome = compare(
            `${label} ${property}`,
            result.first.styles?.[property],
            typeof expected === "string" ? { equals: expected } : expected,
          );
          if (outcome.value !== true) return outcome;
        }
      }
      if (operand.attributes !== undefined) {
        for (const [name, expected] of Object.entries(operand.attributes)) {
          const outcome = compare(
            `${label} @${name}`,
            result.first.attributes?.[name],
            typeof expected === "string" ? { equals: expected } : expected,
          );
          if (outcome.value !== true) return outcome;
        }
      }
      if (operand.rect !== undefined) {
        for (const [side, expected] of Object.entries(operand.rect)) {
          const outcome = compare(`${label} rect.${side}`, result.first.rect?.[side], expected);
          if (outcome.value !== true) return outcome;
        }
      }
      return { value: true, reason: `${label} as expected` };
    }
    case "when": {
      const premise = evaluate(operand.premise, evidence);
      if (premise.value !== true)
        return { value: null, reason: `premise not met: ${premise.reason}` };
      return evaluate(operand.rule, evidence);
    }
    case "every": {
      const documents = Array.isArray(evidence.documents) ? evidence.documents : [];
      if (documents.length === 0) return { value: null, reason: "no documents" };
      const outcomes = documents.map((_, index) => evaluate(forDocument(operand, index), evidence));
      const failed = outcomes.find((outcome) => outcome.value === false);
      if (failed !== undefined) return failed;
      const unknown = outcomes.find((outcome) => outcome.value === null);
      if (unknown !== undefined) return unknown;
      return { value: true, reason: `every document: ${outcomes[0].reason}` };
    }
    case "same": {
      const values = [];
      for (const place of operand) {
        const found = valueOfPlace(evidence, place);
        if (found.value === null) return found;
        values.push(found.value);
      }
      const first = JSON.stringify(values[0]);
      return {
        value: values.every((value) => JSON.stringify(value) === first),
        reason: `same: ${values.map((value) => JSON.stringify(value)).join(" | ")}`,
      };
    }
    case "resource": {
      const document = documentOf(evidence, operand.document);
      if (document === null) return { value: null, reason: `document ${operand.document} missing` };
      const resources = document.observation?.resources;
      if (!Array.isArray(resources))
        return { value: null, reason: `document ${operand.document} resources not observed` };
      const found = resources.filter((resource) => resource.source.includes(operand.source));
      if (found.length === 0)
        return { value: false, reason: `no resource names ${operand.source}` };
      const states = found.map((resource) => resource.state);
      return {
        value: states.every((state) => state === operand.state),
        reason: `${operand.source}: ${states.join(", ")}`,
      };
    }
    case "font": {
      const document = documentOf(evidence, operand.document);
      if (document === null) return { value: null, reason: `document ${operand.document} missing` };
      const fonts = document.observation?.fonts;
      if (fonts === undefined || fonts === null)
        return { value: null, reason: `document ${operand.document} fonts not observed` };
      const loaded = fonts.loaded.some((family) => family.replaceAll('"', "") === operand.loaded);
      return {
        value: loaded,
        reason: `fonts loaded: ${fonts.loaded.join(", ") || "(none)"}`,
      };
    }
    case "path": {
      const base =
        operand.document === undefined ? evidence : documentOf(evidence, operand.document);
      if (base === null) return { value: null, reason: `document ${operand.document} missing` };
      return compare(operand.path, valueAt(base, operand.path), operand);
    }
    default:
      return { value: null, reason: `unknown operator ${operator}` };
  }
}

/** The same rule, bound to one document: every operand that names a document gets this one. */
function forDocument(rule, index) {
  const [operator, operand] = Object.entries(rule)[0];
  if (operator === "all" || operator === "any")
    return { [operator]: operand.map((inner) => forDocument(inner, index)) };
  if (operator === "not") return { not: forDocument(operand, index) };
  if (operator === "same") return { same: operand.map((place) => ({ ...place, document: index })) };
  return { [operator]: { ...operand, document: index } };
}

/** One value a `same` rule compares: a query's text or style in a document, or a path. */
function valueOfPlace(evidence, place) {
  if (place.path !== undefined) {
    const base = place.document === undefined ? evidence : documentOf(evidence, place.document);
    if (base === null) return { value: null, reason: `document ${place.document} missing` };
    const value = valueAt(base, place.path);
    return value === undefined
      ? { value: null, reason: `${place.path} is absent` }
      : { value, reason: place.path };
  }
  const document = documentOf(evidence, place.document);
  if (document === null) return { value: null, reason: `document ${place.document} missing` };
  const result = document.observation?.queries.find((query) => query.id === place.query);
  if (result === undefined || result.first === null || result.first === undefined)
    return { value: null, reason: `query ${place.query} matched nothing in ${place.document}` };
  const value = place.style === undefined ? result.first.text : result.first.styles?.[place.style];
  return value === undefined
    ? { value: null, reason: `query ${place.query} has no ${place.style ?? "text"}` }
    : { value, reason: place.query };
}

function documentOf(evidence, index) {
  const document = evidence.documents?.[index];
  return isRecord(document) ? document : null;
}

/**
 * A path through a `null` reads as `null`: the evidence said there is nothing there.
 * A path through a missing key reads as `undefined`: the evidence did not say, and
 * a rule cannot decide on it.
 */
function valueAt(base, path) {
  let current = base;
  for (const segment of path.split(".")) {
    if (current === null) return null;
    if (typeof current !== "object") return undefined;
    current = current[segment];
  }
  return current;
}

function compare(label, actual, expectation) {
  const shown = JSON.stringify(actual);
  if (expectation.exists !== undefined) {
    const exists = actual !== undefined && actual !== null;
    return { value: exists === expectation.exists, reason: `${label} = ${shown}` };
  }
  if (actual === undefined) return { value: null, reason: `${label} is absent` };
  if (expectation.equals !== undefined) {
    return {
      value: JSON.stringify(actual) === JSON.stringify(expectation.equals),
      reason: `${label} = ${shown}`,
    };
  }
  if (expectation.oneOf !== undefined) {
    return {
      value: expectation.oneOf.some((option) => JSON.stringify(option) === JSON.stringify(actual)),
      reason: `${label} = ${shown}`,
    };
  }
  if (expectation.includes !== undefined) {
    const includes =
      typeof actual === "string"
        ? actual.includes(expectation.includes)
        : Array.isArray(actual) &&
          actual.some((item) => JSON.stringify(item) === JSON.stringify(expectation.includes));
    return { value: includes, reason: `${label} = ${shown}` };
  }
  if (expectation.matches !== undefined) {
    return {
      value: typeof actual === "string" && new RegExp(expectation.matches, "u").test(actual),
      reason: `${label} = ${shown}`,
    };
  }
  if (expectation.min !== undefined || expectation.max !== undefined) {
    const within =
      typeof actual === "number" &&
      (expectation.min === undefined || actual >= expectation.min) &&
      (expectation.max === undefined || actual <= expectation.max);
    return { value: within, reason: `${label} = ${shown}` };
  }
  return { value: null, reason: `${label}: nothing to compare` };
}

/**
 * Judges a collected run again with the expectation table as it is now. The run's
 * own manifest keeps the verdicts taken at collection time; a report always reflects
 * the current table, so a corrected row needs no new run.
 */
export function judgeRun(runDirectory, expectations) {
  const manifest = JSON.parse(readFileSync(resolve(runDirectory, "manifest.json"), "utf8"));
  const verdicts = {};
  for (const run of manifest.runs) {
    const testManifest = JSON.parse(
      readFileSync(resolve(runDirectory, run.id, "manifest.json"), "utf8"),
    );
    const evidencePath = resolve(runDirectory, run.id, "fixture-evidence.json");
    const evidence = existsSync(evidencePath)
      ? JSON.parse(readFileSync(evidencePath, "utf8"))
      : { documents: [] };
    verdicts[run.id] =
      testManifest.result === null
        ? null
        : judge(expectations.tests[run.id], { ...evidence, run: testManifest.result }).verdict;
  }
  return { upstream: manifest.upstream, verdicts };
}

/**
 * The upstream implementation-report shape: one entry per test the run decided,
 * adjudications filling in what the evidence could not. Undecided tests, and those
 * adjudicated `todo`, are left out, which upstream reads as untested.
 */
export function buildReport({ name, ref, variant, verdicts, adjudications, catalog }) {
  const tests = {};
  for (const test of catalog.tests) {
    if (test.template || isDeprecated(test)) continue;
    const judged = verdicts[test.id];
    const adjudicated = adjudications.tests[test.id];
    if (judged !== undefined && judged !== null) tests[test.id] = judged;
    else if (adjudicated !== undefined && adjudicated.result !== "todo") {
      tests[test.id] = adjudicated.result;
    }
  }
  return { name, ref, variant, "tested-by": "implementer", tests };
}

/**
 * The distinct publication surfaces a run recorded. Page counts, spread eligibility and
 * whether a section settles all move with the surface, so evidence only compares across
 * tests and across runs while it stays one size.
 */
export function recordedSurfaces(runDirectory) {
  const manifest = JSON.parse(readFileSync(resolve(runDirectory, "manifest.json"), "utf8"));
  const surfaces = new Map();
  for (const run of manifest.runs) {
    const path = resolve(runDirectory, run.id, "fixture-evidence.json");
    if (!existsSync(path)) continue;
    const frame = JSON.parse(readFileSync(path, "utf8")).runner?.frame;
    if (frame === undefined) continue;
    const key = `${frame.width}x${frame.height}`;
    surfaces.set(key, [...(surfaces.get(key) ?? []), run.id]);
  }
  return surfaces;
}

/** How many tests upstream will render as `todo`: every one the report leaves out. */
export function untestedCount(report, catalog) {
  return catalog.tests.filter(
    (test) => !test.template && !isDeprecated(test) && report.tests[test.id] === undefined,
  ).length;
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
