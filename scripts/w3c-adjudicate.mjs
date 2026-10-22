// The adjudication store, from the terminal: list what the latest run left open, or
// record one verdict against that run. Each entry names the run it was judged on, so
// a later run can show which adjudications predate the evidence they now sit beside.
import { writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { corpusEpubPath } from "./build-w3c-corpus.mjs";
import {
  ADJUDICATIONS_PATH,
  isDeprecated,
  judgeRun,
  readAdjudications,
  readCatalog,
  readExpectations,
  VERDICTS,
} from "./w3c-corpus.mjs";
import { latestRun } from "./w3c-report.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const productBinary = "src-tauri/target/debug/elephant-ladder";

function openTests(run) {
  const catalog = readCatalog();
  const expectations = readExpectations();
  const adjudications = readAdjudications();
  const { verdicts } = judgeRun(run, expectations);
  return catalog.tests
    .filter((test) => !test.template && !isDeprecated(test) && (verdicts[test.id] ?? null) === null)
    .map((test) => ({
      ...test,
      note: expectations.tests[test.id]?.note ?? "",
      adjudicated: adjudications.tests[test.id] ?? null,
    }));
}

function list(run) {
  const open = openTests(run);
  let coverage = null;
  for (const test of open) {
    if (test.coverage !== coverage) {
      coverage = test.coverage;
      process.stdout.write(`\n## ${coverage}\n`);
    }
    const state =
      test.adjudicated === null
        ? "open"
        : `${String(test.adjudicated.result)} (${test.adjudicated.date}, ${basename(test.adjudicated.evidence)})`;
    process.stdout.write(`\n${test.id} — ${state}\n  ${test.description.replace(/\s+/gu, " ")}\n`);
    if (test.note !== "") process.stdout.write(`  why open: ${test.note}\n`);
    process.stdout.write(`  pnpm corpus:w3c:inspect ${test.id}\n`);
    process.stdout.write(`  ${productBinary} ${corpusEpubPath(test.id)}\n`);
    process.stdout.write(`  pnpm corpus:w3c:adjudicate ${test.id}\n`);
  }
  const pending = open.filter((test) => test.adjudicated === null).length;
  process.stdout.write(
    `\n${open.length} open after ${basename(run)}; ${pending} without an adjudication\n`,
  );
}

function record(run, id, verdictText, note) {
  const verdict = verdictText === "true" ? true : verdictText === "false" ? false : verdictText;
  if (!VERDICTS.includes(verdict)) {
    throw new Error("verdict must be one of true, false, n/a, todo");
  }
  const reason = typeof note === "string" ? note.trim() : "";
  if (verdict !== true && reason === "") {
    throw new Error("a note is required for anything but a pass");
  }
  const open = openTests(run);
  if (!open.some((test) => test.id === id)) {
    throw new Error(`${id} is not open after ${basename(run)}; the evidence already decides it`);
  }
  const adjudications = readAdjudications();
  adjudications.tests[id] = {
    result: verdict,
    evidence: basename(run),
    date: new Date().toISOString().slice(0, 10),
    note: reason,
  };
  adjudications.tests = Object.fromEntries(
    Object.entries(adjudications.tests).sort(([left], [right]) => (left < right ? -1 : 1)),
  );
  writeFileSync(ADJUDICATIONS_PATH, `${JSON.stringify(adjudications, null, 2)}\n`);
  process.stdout.write(`${id}: ${String(verdict)} recorded against ${basename(run)}\n`);
}

const [, , first, second, ...rest] = process.argv;
try {
  const run = latestRun(resolve(root, "test-results/w3c-corpus/linux"));
  if (run === null) throw new Error("no Linux corpus run to adjudicate against");
  if (first === undefined) list(run);
  else record(run, first, second, rest.join(" "));
} catch (cause) {
  process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
  process.exitCode = 1;
}
