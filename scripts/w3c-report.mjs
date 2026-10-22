// Turns one Linux corpus run into the upstream implementation report: evidence
// verdicts first, adjudications for what the evidence left open, nothing for the
// rest. The report is what gets contributed to w3c/epub-tests.
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildReport,
  judgeRun,
  readAdjudications,
  readCatalog,
  readExpectations,
  untestedCount,
} from "./w3c-corpus.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const REPORTS_DIRECTORY = resolve(root, "tests/fixtures/w3c-corpus/reports");

const VARIANTS = Object.freeze({
  linux: {
    runs: resolve(root, "test-results/w3c-corpus/linux"),
    variant: "Linux (CEF)",
    file: "elephant-ladder-linux.json",
  },
  android: {
    runs: resolve(root, "test-results/w3c-corpus/android"),
    variant: "Android (WebView)",
    file: "elephant-ladder-android.json",
  },
});

export function latestRun(directory) {
  if (!existsSync(directory)) return null;
  const runs = readdirSync(directory)
    .filter((entry) => existsSync(resolve(directory, entry, "manifest.json")))
    .sort();
  return runs.length === 0 ? null : resolve(directory, runs[runs.length - 1]);
}

export function reportFromRun(runDirectory, variantKey, catalog, adjudications, expectations) {
  const { upstream, verdicts } = judgeRun(runDirectory, expectations);
  if (upstream?.commit !== catalog.upstream.commit) {
    throw new Error(
      `${runDirectory} was collected at ${upstream?.commit}, not the catalog's commit`,
    );
  }
  return buildReport({
    name: "Elephant Ladder",
    ref: "https://github.com/RougeEtoile/elephant-ladder",
    variant: VARIANTS[variantKey].variant,
    verdicts,
    adjudications,
    catalog,
  });
}

function main() {
  const variantKey = process.argv[2] ?? "linux";
  const settings = VARIANTS[variantKey];
  if (settings === undefined) throw new Error(`unknown variant ${variantKey}`);
  const run = process.argv[3] === undefined ? latestRun(settings.runs) : resolve(process.argv[3]);
  if (run === null) throw new Error(`no corpus run under ${settings.runs}`);
  const catalog = readCatalog();
  const report = reportFromRun(run, variantKey, catalog, readAdjudications(), readExpectations());
  mkdirSync(REPORTS_DIRECTORY, { recursive: true });
  const destination = resolve(REPORTS_DIRECTORY, settings.file);
  writeFileSync(destination, `${JSON.stringify(report, null, 2)}\n`);
  const results = Object.values(report.tests);
  const count = (value) => results.filter((result) => result === value).length;
  process.stdout.write(
    `${destination}: ${results.length} tests reported from ${run} (${count(true)} pass, ${count(false)} fail, ${count("n/a")} n/a, ${untestedCount(report, catalog)} todo)\n`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (cause) {
    process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
    process.exitCode = 1;
  }
}
