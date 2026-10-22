import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  symlinkSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { arch, homedir, platform, release, tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  classifyRunnerResult,
  FIXTURE_IDS,
  createRunnerEventCollector,
  evaluateStorageIsolationRelation,
  parseFixtureIds,
  validatePublicationObservationsArtifact,
} from "./fixture-runner-contract.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

/**
 * Each desktop platform declares the engine its evidence describes and how a run is kept clear
 * of the developer's own application data. A spike on a new platform adds its entry rather than
 * copying this collector.
 */
const fixtureRunnerIdentifier = "com.redvelo.elephant-ladder.fixture-runner";
const windowsAppData = () => resolve(process.env.APPDATA, fixtureRunnerIdentifier);
const clearedWindowsRuntimes = new Set();

const platforms = {
  linux: {
    engine: {
      name: "CEF",
      package: () => lockedPackageVersion("cef"),
      tauriRuntime: "tauri-runtime-cef 3.0.0-alpha.5",
    },
    // Tauri resolves the application directories from XDG here, and the webview needs a display.
    isolate: (runtime) => ({
      XDG_DATA_HOME: resolve(runtime, "data"),
      XDG_CACHE_HOME: resolve(runtime, "cache"),
      XDG_CONFIG_HOME: resolve(runtime, "config"),
      GDK_BACKEND: process.env.GDK_BACKEND ?? "x11",
    }),
    prepare: (runtime) => {
      for (const directory of ["data", "cache", "config"])
        mkdirSync(resolve(runtime, directory), { recursive: true });
    },
    appData: (runtime) => resolve(runtime, "data", fixtureRunnerIdentifier),
  },
  darwin: {
    engine: {
      name: "CEF",
      package: () => lockedPackageVersion("cef"),
      tauriRuntime: "tauri-runtime-cef 3.0.0-alpha.5",
    },
    // Tauri resolves ~/Library/Application Support from HOME here, so redirecting it is what
    // keeps a run clear of the developer's own library.
    isolate: (runtime) => ({ HOME: runtime }),
    // An isolated HOME has no login keychain, and macOS resolves it from $HOME/Library/Keychains:
    // without the real one linked back in every launch prompts for a keychain that is not there,
    // and cancelling fails Chromium's OSCrypt.
    prepare: (runtime) => {
      mkdirSync(resolve(runtime, "Library", "Application Support"), { recursive: true });
      mkdirSync(resolve(runtime, "Library", "Caches"), { recursive: true });
      const keychains = resolve(runtime, "Library", "Keychains");
      if (!existsSync(keychains)) {
        symlinkSync(resolve(homedir(), "Library", "Keychains"), keychains);
      }
    },
    appData: (runtime) =>
      resolve(runtime, "Library", "Application Support", fixtureRunnerIdentifier),
  },
  win32: {
    engine: {
      name: "CEF",
      package: () => lockedPackageVersion("cef"),
      tauriRuntime: "tauri-runtime-cef 3.0.0-alpha.5",
    },
    // Windows resolves the application data directory through `SHGetKnownFolderPath`, which
    // reads the user token and not the environment, so a run cannot be redirected the way it is
    // elsewhere. The fixture runner carries its own identifier, which is what keeps a run clear
    // of the developer's own application data; removing that directory is what makes it repeat.
    isolate: () => ({}),
    // Elsewhere a fresh runtime directory is empty by construction and preparing one again
    // changes nothing. Here the clean state is made by removing the application data, so it
    // is made once per runtime: a relation runs several fixtures against one runtime and
    // reads what the earlier ones stored.
    //
    // Windows also refuses to remove a directory any handle is still open on, and the fixture
    // before this one is often a few milliseconds from gone, so the removal waits it out.
    prepare: (runtime) => {
      if (clearedWindowsRuntimes.has(runtime)) return;
      clearedWindowsRuntimes.add(runtime);
      rmSync(windowsAppData(), {
        recursive: true,
        force: true,
        maxRetries: 20,
        retryDelay: 100,
      });
    },
    appData: windowsAppData,
  },
};
const host = platforms[platform()];
const executable = platform() === "win32" ? "elephant-ladder.exe" : "elephant-ladder";
const outputRoot = resolve(root, "test-results/fixture-runner", platform());
const binary =
  process.env.ELEPHANT_QUALIFICATION_BINARY === undefined
    ? resolve(root, "src-tauri/target/fixture-runner/debug", executable)
    : resolve(process.env.ELEPHANT_QUALIFICATION_BINARY);
let fixtureIds = [];
const maximumLogBytes = 1024 * 1024;
const evidenceSchema = `elephant-fixture-runner-${platform()}-evidence-v4`;

mkdirSync(outputRoot, { recursive: true });
const output = mkdtempSync(
  resolve(outputRoot, `${new Date().toISOString().replaceAll(":", "-")}-`),
);
process.stdout.write(`Fixture runner evidence: ${output}\n`);

let environmentEvidence = null;
let application = null;
const runs = [];
const relations = [];
let relationError;
let suiteError;
try {
  fixtureIds = parseFixtureIds(process.argv.slice(2));
  if (host === undefined) {
    throw new Error(
      `fixture-runner qualification has no entry for ${platform()}; declare its engine and run ` +
        "isolation in collect-fixture-runner-desktop.mjs",
    );
  }
  if (!existsSync(binary)) throw new Error(`Fixture-runner application is missing: ${binary}`);
  environmentEvidence = {
    os: { platform: platform(), release: release(), architecture: arch() },
    browserEngine: {
      name: host.engine.name,
      package: host.engine.package(),
      tauriRuntime: host.engine.tauriRuntime,
    },
  };
  application = {
    binary: basename(binary),
    sha256: digest(binary),
    source: sourceState(root),
    navigator: sourceState(resolve(root, "../navigator")),
    stack: sourceState(resolve(root, "../stack")),
  };
  for (const fixtureId of fixtureIds) runs.push(await collectFixture(fixtureId));
  const relationFixtureIds = ["active-probe", "active-probe-twin"];
  if (relationFixtureIds.every((id) => fixtureIds.includes(id))) {
    try {
      await collectRelations((relation) => relations.push(relation));
    } catch (cause) {
      relationError = cause instanceof Error ? cause.message : String(cause);
    }
  }
} catch (cause) {
  suiteError = cause instanceof Error ? cause.message : String(cause);
}

const suite = {
  schema: evidenceSchema,
  generatedAt: new Date().toISOString(),
  environment: environmentEvidence,
  application,
  fixtures: fixtureIds,
  runs: runs.map(({ fixtureId, status, error }) => ({
    fixtureId,
    status,
    manifest: `${fixtureId}/manifest.json`,
    ...(error === undefined ? {} : { error }),
  })),
  relations: relations.map(({ id, status, manifestSha256 }) => ({
    id,
    status,
    manifest: `relations/${id}/manifest.json`,
    manifestSha256,
  })),
  ...(relationError === undefined ? {} : { relationError }),
  ...(suiteError === undefined ? {} : { suiteError }),
};
writeJson(resolve(output, "manifest.json"), suite);

const failed = runs.filter((run) => run.status !== "automated-pass");
const failedRelations = relations.filter((relation) => relation.status !== "automated-pass");
if (
  failed.length === 0 &&
  failedRelations.length === 0 &&
  relationError === undefined &&
  suiteError === undefined
) {
  process.stdout.write(
    `Fixture runner suite passed ${runs.length}/${FIXTURE_IDS.length} fixtures (${fixtureIds.join(", ")}); evidence: ${output}\n`,
  );
} else {
  process.stderr.write(
    `Fixture runner suite has ${failed.length}/${runs.length} unsuccessful runs and ${failedRelations.length}/${relations.length} unsuccessful relations${relationError === undefined ? "" : `; relation error: ${relationError}`}${suiteError === undefined ? "" : `; suite error: ${suiteError}`}; evidence: ${output}\n`,
  );
  process.exitCode = 1;
}

async function collectFixture(fixtureId, options = {}) {
  const runOutput = options.runOutput ?? resolve(output, fixtureId);
  const ownsRuntime = options.runtime === undefined;
  const runtime =
    options.runtime ?? mkdtempSync(resolve(tmpdir(), `elephant-fixture-runner-${fixtureId}-`));
  mkdirSync(runOutput, { recursive: true });
  const environment = { ...process.env, ...host.isolate(runtime) };
  delete environment.TAURI_AUTOMATION;
  delete environment.TAURI_WEBVIEW_AUTOMATION;

  const collector = createRunnerEventCollector(fixtureId);
  const chunks = [];
  let logBytes = 0;
  let logTruncated = false;
  let eventError = null;
  let status = "infrastructure-failure";
  let error;
  let terminalArtifact = null;
  let publicationObservations = null;
  let observationValue = null;

  try {
    host.prepare(runtime);
    const arguments_ = ["--fixture", fixtureId, "--exit-after-run"];
    const child = spawn(binary, arguments_, {
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

    const appData = host.appData(runtime);
    const terminalSource = resolve(appData, "fixture-run-finished.json");
    const terminalValue = readJson(terminalSource, "terminal artifact");
    const persisted = createRunnerEventCollector(fixtureId);
    persisted.consume(JSON.stringify(terminalValue));
    if (JSON.stringify(persisted.terminal) !== JSON.stringify(collector.terminal)) {
      throw new Error("Native terminal artifact does not match its trace");
    }
    copyFileSync(terminalSource, resolve(runOutput, "fixture-run-finished.json"));
    terminalArtifact = {
      file: "fixture-run-finished.json",
      sha256: digest(resolve(runOutput, "fixture-run-finished.json")),
    };

    if (collector.terminal.runStatus === "complete") {
      const observationsSource = resolve(appData, collector.terminal.publicationObservations.file);
      if (digest(observationsSource) !== collector.terminal.publicationObservations.sha256) {
        throw new Error("Publication observations digest does not match terminal metadata");
      }
      const observations = readJson(observationsSource, "publication observations");
      validatePublicationObservationsArtifact(observations, collector.terminal);
      copyFileSync(
        observationsSource,
        resolve(runOutput, collector.terminal.publicationObservations.file),
      );
      publicationObservations = {
        file: collector.terminal.publicationObservations.file,
        sha256: digest(resolve(runOutput, collector.terminal.publicationObservations.file)),
      };
      observationValue = observations;
    }
    status = classifyRunnerResult(collector.terminal);
  } catch (cause) {
    error = cause instanceof Error ? cause.message : String(cause);
  } finally {
    if (ownsRuntime) rmSync(runtime, { recursive: true, force: true });
  }

  const logPath = resolve(runOutput, "runner.log");
  writeFileSync(logPath, Buffer.concat(chunks), { mode: 0o600 });
  const manifestPath = resolve(runOutput, "manifest.json");
  writeJson(manifestPath, {
    schema: evidenceSchema,
    fixtureId,
    status,
    environment: environmentEvidence,
    application,
    result: collector.terminal,
    terminalArtifact,
    publicationObservations,
    log: { file: "runner.log", truncated: logTruncated, sha256: digest(logPath) },
    ...(error === undefined ? {} : { error }),
  });
  return {
    fixtureId,
    status,
    observationValue,
    evidence: {
      manifestSha256: digest(manifestPath),
      observationsSha256: publicationObservations?.sha256 ?? null,
    },
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

async function collectRelations(onComplete) {
  const relationRoot = resolve(output, "relations");
  const runtime = mkdtempSync(resolve(tmpdir(), "elephant-fixture-runner-relations-"));
  try {
    host.prepare(runtime);
    const primaryFirst = await collectFixture("active-probe", {
      runtime,
      runOutput: resolve(relationRoot, "storage-isolation", "primary-first"),
    });
    const twin = await collectFixture("active-probe-twin", {
      runtime,
      runOutput: resolve(relationRoot, "storage-isolation", "twin"),
    });
    const primarySecond = await collectFixture("active-probe", {
      runtime,
      runOutput: resolve(relationRoot, "storage-isolation", "primary-second"),
    });
    requirePassingRuns([primaryFirst, twin, primarySecond]);
    const storageRelation = evaluateStorageIsolationRelation(
      primaryFirst.observationValue,
      twin.observationValue,
      primarySecond.observationValue,
    );
    const storageManifest = resolve(relationRoot, storageRelation.id, "manifest.json");
    writeJson(storageManifest, {
      schema: "elephant-fixture-runner-relation-v2",
      inputs: relationInputs([
        ["primary-first", primaryFirst],
        ["twin", twin],
        ["primary-second", primarySecond],
      ]),
      ...storageRelation,
    });
    onComplete({ ...storageRelation, manifestSha256: digest(storageManifest) });
  } finally {
    rmSync(runtime, { recursive: true, force: true });
  }
}

function requirePassingRuns(runs) {
  const failed = runs.find(
    (run) => run.status !== "automated-pass" || run.observationValue === null,
  );
  if (failed !== undefined)
    throw new Error(
      `Relational fixture run failed: ${failed.fixtureId}: ${failed.error ?? failed.status}`,
    );
}

function relationInputs(entries) {
  return entries.map(([id, run]) => ({
    id,
    fixtureId: run.fixtureId,
    manifest: `${id}/manifest.json`,
    manifestSha256: run.evidence.manifestSha256,
    publicationObservations: `./${id}/fixture-publication-observations.json`,
    publicationObservationsSha256: run.evidence.observationsSha256,
  }));
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
