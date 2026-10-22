// Opens one corpus test in the fixture runner and leaves the window up: evidence is
// collected and recorded as in a run, then the book is handed to the person at the
// keyboard for the judgement the evidence could not make.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { corpusEpubPath } from "./build-w3c-corpus.mjs";
import { DEFAULT_OBSERVATION, readCatalog, readExpectations } from "./w3c-corpus.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const binary =
  process.env.ELEPHANT_QUALIFICATION_BINARY === undefined
    ? resolve(root, "src-tauri/target/fixture-runner/debug/elephant-ladder")
    : resolve(process.env.ELEPHANT_QUALIFICATION_BINARY);

const id = process.argv[2];
const windowSize = process.argv[3] ?? "1400x900";
if (id === undefined) {
  process.stderr.write("usage: w3c-inspect <test id> [<width>x<height>]\n");
  process.exit(2);
}
const test = readCatalog().tests.find((candidate) => candidate.id === id);
if (test === undefined) throw new Error(`Unknown W3C test id: ${id}`);
const epub = corpusEpubPath(id);
if (!existsSync(epub))
  throw new Error(`Cached corpus EPUB is missing; run pnpm corpus:w3c (${epub})`);
if (!existsSync(binary)) throw new Error(`Fixture-runner application is missing: ${binary}`);
const runtime = mkdtempSync(resolve(tmpdir(), `elephant-w3c-inspect-${id}-`));
for (const directory of ["data", "cache", "config"])
  mkdirSync(resolve(runtime, directory), { recursive: true });
const plan = resolve(runtime, "observation.json");
writeFileSync(
  plan,
  `${JSON.stringify(readExpectations().tests[id]?.observation ?? DEFAULT_OBSERVATION, null, 2)}\n`,
);
process.stdout.write(`${id}: ${test.description.replace(/\s+/gu, " ")}\n`);
process.stdout.write(`Arrow keys or page clicks turn pages once the run reads "recorded".\n`);
const environment = {
  ...process.env,
  XDG_DATA_HOME: resolve(runtime, "data"),
  XDG_CACHE_HOME: resolve(runtime, "cache"),
  XDG_CONFIG_HOME: resolve(runtime, "config"),
  GDK_BACKEND: process.env.GDK_BACKEND ?? "x11",
};
delete environment.TAURI_AUTOMATION;
delete environment.TAURI_WEBVIEW_AUTOMATION;
const child = spawn(
  binary,
  ["--fixture-epub", epub, "--observation", plan, "--window", windowSize],
  { cwd: root, env: environment, stdio: ["ignore", "ignore", "inherit"] },
);
child.on("exit", (code) => {
  process.exitCode = code ?? 1;
});
