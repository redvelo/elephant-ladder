import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { arch, platform, release, tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { corpusEpubPath, UPSTREAM_COMMIT } from "./build-w3c-corpus.mjs";
import {
  classifyRunnerResult,
  createRunnerEventCollector,
  evidenceFixtureIdentity,
  validateFixtureEvidenceArtifact,
} from "./fixture-runner-contract.mjs";
import {
  DEFAULT_OBSERVATION,
  isDeprecated,
  judge,
  readCatalog,
  readExpectations,
  recordedSurfaces,
} from "./w3c-corpus.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const outputRoot = resolve(root, "test-results/w3c-corpus/linux");
const binary =
  process.env.ELEPHANT_QUALIFICATION_BINARY === undefined
    ? resolve(root, "src-tauri/target/fixture-runner/debug/elephant-ladder")
    : resolve(process.env.ELEPHANT_QUALIFICATION_BINARY);
const maximumLogBytes = 1024 * 1024;
const evidenceSchema = "elephant-w3c-corpus-linux-evidence-v1";
const processTimeoutMs = (spineLength) => 60_000 + 15_000 * spineLength;
// A landscape window large enough to show the runner's stated publication surface.
// The surface itself is pinned in the runner's own stylesheet, so evidence no longer
// depends on what the window manager does with this request.
const windowSize = "1400x900";
// Each fixture is a windowed application launch, so a run used to take the display for
// as long as it lasted. Nothing about the evidence needs a visible window, so the run
// goes inside a headless nested compositor where one is available, and then several
// fixtures can run at once: each already has its own XDG directories and temporary
// profile, so they share nothing.
const headless = process.env.ELEPHANT_CORPUS_HEADLESS !== "0";
const workers = Math.max(1, Number.parseInt(process.env.ELEPHANT_CORPUS_WORKERS ?? "4", 10) || 1);
// Device pixel ratio is stated for the same reason the surface is: it moves sub-pixel
// geometry and the roll runtime's visible range, and a headless output does not have
// the scale of whichever display the run would otherwise have inherited.
const deviceScale = process.env.ELEPHANT_CORPUS_SCALE ?? "2";

/**
 * Which tests to run: ids, `--coverage <name>` groups, or everything when nothing is
 * named. Templates never run, and the upstream `deprecated` collection runs only
 * when named by id or asked for with `--deprecated`, as upstream's own report
 * generator leaves it out unless told otherwise.
 */
function headlessCompositor() {
  if (!headless) return null;
  const probe = spawnSync("gamescope", ["--help"], { stdio: "ignore" });
  return probe.error === undefined && probe.status !== null ? "gamescope" : null;
}

function launch(compositor, args) {
  const [width, height] = windowSize.split("x");
  return compositor === null
    ? { command: binary, args }
    : {
        command: compositor,
        args: [
          "--backend",
          "headless",
          "--xwayland-count",
          "1",
          "-W",
          width,
          "-H",
          height,
          "--",
          binary,
          ...args,
        ],
      };
}

export function selectTests(catalog, args) {
  const ids = [];
  const coverages = [];
  let deprecated = false;
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--coverage") {
      coverages.push(args[index + 1]);
      index += 1;
    } else if (args[index] === "--deprecated") deprecated = true;
    else ids.push(args[index]);
  }
  const current = (test) => deprecated || !isDeprecated(test);
  const byId = new Map(catalog.tests.map((test) => [test.id, test]));
  if (ids.length === 0 && coverages.length === 0) {
    return catalog.tests.filter((test) => !test.template && current(test));
  }
  const selected = new Map();
  for (const id of ids) {
    const test = byId.get(id);
    if (test === undefined) throw new Error(`Unknown W3C test id: ${id}`);
    selected.set(id, test);
  }
  for (const coverage of coverages) {
    const group = catalog.tests.filter((test) => test.coverage === coverage && current(test));
    if (group.length === 0) throw new Error(`Unknown coverage group: ${coverage}`);
    for (const test of group) selected.set(test.id, test);
  }
  return [...selected.values()];
}

async function main() {
  const catalog = readCatalog();
  const expectations = readExpectations();
  const tests = selectTests(catalog, process.argv.slice(2));
  if (platform() !== "linux") throw new Error("W3C corpus collection requires Linux");
  if (!existsSync(binary)) throw new Error(`Fixture-runner application is missing: ${binary}`);
  mkdirSync(outputRoot, { recursive: true });
  const output = mkdtempSync(
    resolve(outputRoot, `${new Date().toISOString().replaceAll(":", "-")}-`),
  );
  process.stdout.write(`W3C corpus evidence: ${output}\n`);
  const environment = {
    os: { platform: platform(), release: release(), architecture: arch() },
    browserEngine: {
      name: "CEF",
      package: lockedPackageVersion("cef"),
      tauriRuntime: "tauri-runtime-cef 3.0.0-alpha.5",
    },
  };
  const application = {
    binary: basename(binary),
    sha256: digest(binary),
    source: sourceState(root),
    // Navigator is built from the sibling checkout, so its commit is what ran. Stack is a
    // published dependency, so the released version is; naming a checkout would describe
    // something the binary did not use.
    navigator: sourceState(resolve(root, "../navigator")),
    stack: { version: lockedPackageVersion("epub-stack") },
  };
  const compositor = headlessCompositor();
  process.stdout.write(
    `${tests.length} fixtures, ${Math.min(workers, tests.length)} at a time, scale ${deviceScale}, ${compositor === null ? "on this display" : `headless under ${compositor}`}\n`,
  );
  const collected = new Map();
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(workers, tests.length) }, async () => {
      while (next < tests.length) {
        const test = tests[next];
        next += 1;
        const run = await collectTest(
          test,
          expectations.tests[test.id],
          output,
          environment,
          application,
          compositor,
        );
        collected.set(test.id, run);
        process.stdout.write(
          `${test.id}: ${run.status}${run.verdict === null ? "" : ` verdict=${String(run.verdict)}`}${run.error === undefined ? "" : ` (${run.error})`}\n`,
        );
      }
    }),
  );
  // Several fixtures at once can starve one of them of the CPU it needs to settle inside
  // its deadline. A fixture that recorded nothing is retried once, alone, so the run's
  // speed is never paid for in evidence.
  const starved = tests.filter((test) => collected.get(test.id)?.status !== "evidence-recorded");
  if (starved.length > 0 && workers > 1) {
    process.stdout.write(`retrying ${starved.length} without company\n`);
    for (const test of starved) {
      const run = await collectTest(
        test,
        expectations.tests[test.id],
        output,
        environment,
        application,
        compositor,
      );
      collected.set(test.id, run);
      process.stdout.write(
        `${test.id}: ${run.status}${run.verdict === null ? "" : ` verdict=${String(run.verdict)}`}${run.error === undefined ? "" : ` (${run.error})`}\n`,
      );
    }
  }
  // The manifest stays in catalog order however the work was scheduled.
  const runs = tests.map((test) => collected.get(test.id)).filter((run) => run !== undefined);
  const verdicts = Object.fromEntries(runs.map((run) => [run.id, run.verdict]));
  writeJson(resolve(output, "manifest.json"), {
    schema: evidenceSchema,
    generatedAt: new Date().toISOString(),
    upstream: { repository: catalog.upstream.repository, commit: UPSTREAM_COMMIT },
    environment,
    application,
    runs: runs.map(({ id, status, verdict, reason, error }) => ({
      id,
      status,
      verdict,
      reason,
      manifest: `${id}/manifest.json`,
      ...(error === undefined ? {} : { error }),
    })),
    verdicts,
  });
  const unrecorded = runs.filter((run) => run.status !== "evidence-recorded");
  const decided = runs.filter((run) => run.verdict !== null);
  process.stdout.write(
    `Recorded ${runs.length - unrecorded.length}/${runs.length} tests; ${decided.length} decided (${decided.filter((run) => run.verdict === true).length} pass, ${decided.filter((run) => run.verdict === false).length} fail, ${decided.filter((run) => run.verdict === "n/a").length} n/a); evidence: ${output}\n`,
  );
  if (unrecorded.length > 0) {
    process.stderr.write(`${unrecorded.length} tests recorded no evidence\n`);
    process.exitCode = 1;
  }
  const surfaces = recordedSurfaces(output);
  if (surfaces.size > 1) {
    process.stderr.write(
      `the publication surface was not one size, so this run is not comparable evidence:\n${[
        ...surfaces,
      ]
        .map(([size, tests]) => `  ${size}: ${tests.length} tests (${tests[0]})`)
        .join("\n")}\n`,
    );
    process.exitCode = 1;
  }
}

async function collectTest(test, expectation, output, environment, application, compositor) {
  const runOutput = resolve(output, test.id);
  mkdirSync(runOutput, { recursive: true });
  const runtime = mkdtempSync(resolve(tmpdir(), `elephant-w3c-${test.id}-`));
  const collector = createRunnerEventCollector(test.id, evidenceFixtureIdentity(test.sha256));
  const chunks = [];
  let logBytes = 0;
  let logTruncated = false;
  let eventError = null;
  let status = "infrastructure-failure";
  let error;
  let evidence = null;
  let evidenceValue = null;
  let judgement = { verdict: null, reason: "no evidence" };
  const observation = expectation?.observation ?? DEFAULT_OBSERVATION;
  const plan = resolve(runOutput, "observation.json");
  writeJson(plan, observation);
  try {
    const epub = corpusEpubPath(test.id);
    if (!existsSync(epub) || digest(epub) !== test.sha256) {
      throw new Error(`Cached corpus EPUB is missing or stale; run pnpm corpus:w3c (${epub})`);
    }
    for (const directory of ["data", "cache", "config"])
      mkdirSync(resolve(runtime, directory), { recursive: true });
    const environmentVariables = {
      ...process.env,
      XDG_DATA_HOME: resolve(runtime, "data"),
      XDG_CACHE_HOME: resolve(runtime, "cache"),
      XDG_CONFIG_HOME: resolve(runtime, "config"),
      GDK_BACKEND: process.env.GDK_BACKEND ?? "x11",
      GDK_SCALE: deviceScale,
    };
    delete environmentVariables.TAURI_AUTOMATION;
    delete environmentVariables.TAURI_WEBVIEW_AUTOMATION;
    const started = launch(compositor, [
      "--fixture-epub",
      epub,
      "--observation",
      plan,
      "--window",
      windowSize,
      "--exit-after-run",
    ]);
    const child = spawn(started.command, started.args, {
      cwd: root,
      env: environmentVariables,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const pending = new Map([
      ["stdout", ""],
      ["stderr", ""],
    ]);
    for (const stream of ["stdout", "stderr"]) {
      child[stream].on("data", (chunk) => {
        const bytes = Buffer.from(chunk);
        const remaining = maximumLogBytes - logBytes;
        if (remaining > 0) {
          const bounded = bytes.subarray(0, remaining);
          chunks.push(bounded);
          logBytes += bounded.length;
        }
        if (bytes.length > remaining) logTruncated = true;
        const lines = `${pending.get(stream)}${bytes.toString("utf8")}`.split("\n");
        pending.set(stream, lines.pop() ?? "");
        for (const line of lines) consumeEvent(line);
      });
    }
    const outcome = await processOutcome(child, processTimeoutMs(test.spineLength));
    if (outcome.timeout) {
      child.kill("SIGKILL");
      throw new Error("Native fixture process timed out");
    }
    for (const value of pending.values()) if (value !== "") consumeEvent(value);
    if (eventError !== null) throw eventError;
    if (outcome.error !== null) throw outcome.error;
    if (outcome.signal !== null)
      throw new Error(`Native fixture process exited with ${outcome.signal}`);
    if (outcome.code !== 0)
      throw new Error(`Native fixture process exited with status ${outcome.code}`);
    if (collector.terminal === null)
      throw new Error("Native fixture process supplied no terminal event");

    const appData = resolve(runtime, "data", "com.redvelo.elephant-ladder.fixture-runner");
    const terminalSource = resolve(appData, "fixture-run-finished.json");
    const persisted = createRunnerEventCollector(test.id, evidenceFixtureIdentity(test.sha256));
    persisted.consume(JSON.stringify(readJson(terminalSource, "terminal artifact")));
    if (JSON.stringify(persisted.terminal) !== JSON.stringify(collector.terminal)) {
      throw new Error("Native terminal artifact does not match its trace");
    }
    copyFileSync(terminalSource, resolve(runOutput, "fixture-run-finished.json"));
    status = classifyRunnerResult(collector.terminal);
    if (collector.terminal.runStatus === "complete") {
      const evidenceSource = resolve(appData, collector.terminal.evidence.file);
      if (digest(evidenceSource) !== collector.terminal.evidence.sha256) {
        throw new Error("Fixture evidence digest does not match terminal metadata");
      }
      evidenceValue = validateFixtureEvidenceArtifact(
        readJson(evidenceSource, "fixture evidence"),
        collector.terminal,
      );
      copyFileSync(evidenceSource, resolve(runOutput, collector.terminal.evidence.file));
      evidence = {
        file: collector.terminal.evidence.file,
        sha256: collector.terminal.evidence.sha256,
        documents: collector.terminal.evidence.documents,
      };
      judgement = judge(expectation, { ...evidenceValue, run: collector.terminal });
    } else {
      error = collector.terminal.incompleteReason;
      judgement = judge(expectation, { documents: [], run: collector.terminal });
    }
  } catch (cause) {
    error = cause instanceof Error ? cause.message : String(cause);
  } finally {
    rmSync(runtime, { recursive: true, force: true });
  }
  const logPath = resolve(runOutput, "runner.log");
  writeFileSync(logPath, Buffer.concat(chunks), { mode: 0o600 });
  writeJson(resolve(runOutput, "manifest.json"), {
    schema: evidenceSchema,
    test: {
      id: test.id,
      identifier: test.identifier,
      coverage: test.coverage,
      sha256: test.sha256,
      upstreamCommit: UPSTREAM_COMMIT,
    },
    status,
    verdict: judgement.verdict,
    reason: judgement.reason,
    expectation: expectation ?? null,
    observation: { file: "observation.json", sha256: digest(plan) },
    window: windowSize,
    deviceScale,
    headless: compositor ?? false,
    environment,
    application,
    result: collector.terminal,
    evidence,
    log: { file: "runner.log", truncated: logTruncated, sha256: digest(logPath) },
    ...(error === undefined ? {} : { error }),
  });
  return {
    id: test.id,
    status,
    verdict: judgement.verdict,
    reason: judgement.reason,
    ...(error === undefined ? {} : { error }),
  };

  function consumeEvent(line) {
    if (eventError !== null) return;
    try {
      collector.consume(line);
    } catch (cause) {
      eventError = cause;
    }
  }
}

function processOutcome(child, timeout) {
  return new Promise((resolveOutcome) => {
    let error = null;
    let settled = false;
    const timer = setTimeout(
      () => settle({ code: null, signal: null, error, timeout: true }),
      timeout,
    );
    child.once("error", (cause) => {
      error = cause;
    });
    child.once("exit", (code, signal) => settle({ code, signal, error, timeout: false }));
    function settle(value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolveOutcome(value);
    }
  });
}

function readJson(path, label) {
  if (!existsSync(path)) throw new Error(`${label} is missing`);
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new Error(`${label} is not valid JSON`);
  }
}

function sourceState(directory) {
  const commit = commandOutput("git", ["-C", directory, "rev-parse", "HEAD"]);
  const status = spawnSync("git", ["-C", directory, "status", "--porcelain"], { encoding: "utf8" });
  return { commit, dirty: status.status === 0 ? status.stdout.trim() !== "" : null };
}

function commandOutput(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  const value = `${result.stdout ?? ""}${result.stderr ?? ""}`.trim();
  return result.status === 0 && value !== "" ? value.slice(0, 500) : "unavailable";
}

function lockedPackageVersion(name) {
  const lock = readFileSync(resolve(root, "src-tauri/Cargo.lock"), "utf8");
  const match = lock.match(new RegExp(`name = "${name}"\\nversion = "([^"]+)"`));
  return match?.[1] ?? "unavailable";
}

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
    process.exitCode = 1;
  });
}
