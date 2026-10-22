import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { arch, platform, release, tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { createRunnerEventCollector } from "./fixture-runner-contract.mjs";
import {
  NATIVE_DURABLE_STORAGE_SCHEMA,
  evaluateNativeRestartRuns,
} from "./native-durable-storage-linux.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const binary =
  process.env.ELEPHANT_QUALIFICATION_BINARY === undefined
    ? resolve(root, "src-tauri/target/fixture-runner/debug/elephant-ladder")
    : resolve(process.env.ELEPHANT_QUALIFICATION_BINARY);
const output = resolve(root, "test-results/durable-storage/linux-native");
const fixtureId = "active-probe";
const maximumLogBytes = 1024 * 1024;

if (platform() !== "linux") throw new Error("native durable-storage qualification requires Linux");
if (!existsSync(binary)) throw new Error(`Fixture-runner application is missing: ${binary}`);
rmSync(output, { recursive: true, force: true });
mkdirSync(output, { recursive: true });
const runtime = mkdtempSync(resolve(tmpdir(), "elephant-native-durable-storage-"));
for (const directory of ["data", "cache", "config"])
  mkdirSync(resolve(runtime, directory), { recursive: true });

const environment = {
  ...process.env,
  XDG_DATA_HOME: resolve(runtime, "data"),
  XDG_CACHE_HOME: resolve(runtime, "cache"),
  XDG_CONFIG_HOME: resolve(runtime, "config"),
  GDK_BACKEND: process.env.GDK_BACKEND ?? "x11",
};
delete environment.TAURI_AUTOMATION;
delete environment.TAURI_WEBVIEW_AUTOMATION;

try {
  const runs = [];
  for (let ordinal = 1; ordinal <= 2; ordinal += 1) {
    runs.push(await runProcess(ordinal));
  }
  const evaluated = evaluateNativeRestartRuns(runs);
  const manifest = {
    schema: NATIVE_DURABLE_STORAGE_SCHEMA,
    status: "passed",
    generatedAt: new Date().toISOString(),
    environment: {
      os: { platform: platform(), release: release(), architecture: arch() },
      browserEngine: {
        name: "CEF",
        package: lockedPackageVersion("cef"),
        tauriRuntime: "tauri-runtime-cef 3.0.0-alpha.5",
      },
    },
    application: {
      binary: basename(binary),
      sha256: digest(binary),
      source: sourceState(root),
    },
    fixtureId,
    publicationOrigin: evaluated.publicationOrigin,
    observations: evaluated.observations,
    runs: runs.map((run, index) => ({
      processOrdinal: index + 1,
      terminal: run.terminal,
      terminalArtifact: run.terminalArtifact,
      publicationObservations: run.publicationObservations,
      log: run.log,
    })),
  };
  writeJsonAtomically(resolve(output, "manifest.json"), manifest);
  process.stdout.write(`${JSON.stringify(manifest)}\n`);
} finally {
  if (process.env.ELEPHANT_KEEP_QUALIFICATION_PROFILE === "1") {
    process.stderr.write(`Retained native qualification profile: ${runtime}\n`);
  } else {
    rmSync(runtime, { recursive: true, force: true });
  }
}

async function runProcess(ordinal) {
  const collector = createRunnerEventCollector(fixtureId);
  let eventError = null;
  const chunks = [];
  let logBytes = 0;
  let logTruncated = false;
  const child = spawn(binary, ["--fixture", fixtureId, "--exit-after-run"], {
    cwd: root,
    env: environment,
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
  const outcome = await processOutcome(child, 70_000);
  if (outcome.timeout) {
    child.kill("SIGKILL");
    throw new Error(`Native fixture process ${ordinal} timed out`);
  }
  for (const value of pending.values()) if (value !== "") consumeEvent(value);
  if (eventError !== null) throw eventError;
  if (outcome.error !== null) throw outcome.error;
  if (outcome.signal !== null)
    throw new Error(`Native fixture process ${ordinal} exited with ${outcome.signal}`);
  if (outcome.code !== 0)
    throw new Error(`Native fixture process ${ordinal} exited with status ${outcome.code}`);
  if (collector.terminal === null)
    throw new Error(`Native fixture process ${ordinal} supplied no terminal event`);

  const appData = resolve(runtime, "data", "com.redvelo.elephant-ladder.fixture-runner");
  const terminalSource = resolve(appData, "fixture-run-finished.json");
  const observationsSource = resolve(appData, collector.terminal.publicationObservations.file);
  const terminalValue = readJson(terminalSource, `process ${ordinal} terminal artifact`);
  const persisted = createRunnerEventCollector(fixtureId);
  persisted.consume(JSON.stringify(terminalValue));
  if (JSON.stringify(persisted.terminal) !== JSON.stringify(collector.terminal)) {
    throw new Error(`Native fixture process ${ordinal} terminal artifact does not match its trace`);
  }
  if (digest(observationsSource) !== collector.terminal.publicationObservations.sha256) {
    throw new Error(`Native fixture process ${ordinal} publication observations digest mismatch`);
  }
  const observations = readJson(observationsSource, `process ${ordinal} publication observations`);
  const terminalFile = `process-${ordinal}-terminal.json`;
  const observationsFile = `process-${ordinal}-publication-observations.json`;
  const logFile = `process-${ordinal}.log`;
  copyFileSync(terminalSource, resolve(output, terminalFile));
  copyFileSync(observationsSource, resolve(output, observationsFile));
  writeFileSync(resolve(output, logFile), Buffer.concat(chunks), { mode: 0o600 });
  return {
    terminal: collector.terminal,
    observations,
    terminalArtifact: { file: terminalFile, sha256: digest(resolve(output, terminalFile)) },
    publicationObservations: {
      file: observationsFile,
      sha256: digest(resolve(output, observationsFile)),
    },
    log: { file: logFile, truncated: logTruncated, sha256: digest(resolve(output, logFile)) },
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

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function lockedPackageVersion(name) {
  const lock = readFileSync(resolve(root, "src-tauri/Cargo.lock"), "utf8");
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const match = lock.match(
    new RegExp(`\\[\\[package\\]\\]\\nname = "${escaped}"\\nversion = "([^"]+)"`, "u"),
  );
  return match?.[1] ?? "unavailable";
}

function writeJsonAtomically(path, value) {
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporary, path);
}
