// Runs the engine-support probe and prints what the engine answered.
//
// The probe is an ordinary publication, so a platform with no harness can be measured
// by opening it by hand. This is the same questions asked where a harness exists.
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { arch, platform, release, tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ANDROID_FIXTURE_EPUB_EXTRA,
  ANDROID_FIXTURE_OBSERVATION_EXTRA,
  APK,
  APPLICATION_ID,
  TERMINAL_FILE,
  adb,
  applicationSource,
  attachedDevice,
  buildFixtureApplication,
  cleanupDevice,
  deviceEnvironment,
  digestBytes,
  launchFixtureApplication,
  parseJson,
  readDeviceArtifact,
  restoreProductArtifact,
  stopFixtureApplication,
  validateDeviceEnvironment,
  waitForTerminalArtifact,
  writeJson,
} from "./android-fixture-runner.mjs";
import {
  createRunnerEventCollector,
  evidenceFixtureIdentity,
  validateFixtureEvidenceArtifact,
} from "./fixture-runner-contract.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixture = resolve(root, "tests/fixtures/engine-support/elephant-engine-support.epub");
// The runner names a publication by its file stem, so the staged copy keeps that name.
const fixtureId = basename(fixture, ".epub");
const plan = resolve(root, "tests/fixtures/engine-support/observation.json");
const evidenceSchema = "elephant-engine-support-evidence-v1";
const binary =
  process.env.ELEPHANT_QUALIFICATION_BINARY === undefined
    ? resolve(root, "src-tauri/target/fixture-runner/debug/elephant-ladder")
    : resolve(process.env.ELEPHANT_QUALIFICATION_BINARY);
const windowSize = "1400x900";
const timeoutMs = 120_000;

/** The probe's answers, as the groups it reports them in. */
export function readAnswers(evidence) {
  const queries = evidence.documents[0]?.observation?.queries ?? [];
  const answers = {};
  for (const query of queries) {
    const text = query.first?.text ?? "";
    answers[query.id] =
      query.id === "status"
        ? (query.first?.attributes?.["data-state"] ?? text)
        : Object.fromEntries(
            text
              .split(";")
              .filter((entry) => entry.includes("="))
              .map((entry) => {
                const separator = entry.lastIndexOf("=");
                return [entry.slice(0, separator), entry.slice(separator + 1)];
              }),
          );
  }
  return answers;
}

function print(answers) {
  if (answers.status !== "complete") {
    process.stdout.write(`Resource probes did not settle: ${String(answers.status)}\n`);
  }
  for (const [group, entries] of Object.entries(answers)) {
    if (group === "status" || typeof entries !== "object") continue;
    process.stdout.write(`\n## ${group}\n`);
    const width = Math.max(...Object.keys(entries).map((name) => name.length));
    for (const [name, value] of Object.entries(entries)) {
      process.stdout.write(`  ${name.padEnd(width)}  ${value}\n`);
    }
  }
}

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

async function collectLinux(output) {
  if (!existsSync(binary)) throw new Error(`Fixture-runner application is missing: ${binary}`);
  const runtime = mkdtempSync(resolve(tmpdir(), "elephant-engine-support-"));
  try {
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
    const child = spawn(
      binary,
      [
        "--fixture-epub",
        fixture,
        "--observation",
        plan,
        "--window",
        windowSize,
        "--exit-after-run",
      ],
      { cwd: root, env: environment, stdio: ["ignore", "ignore", "ignore"] },
    );
    const exited = await new Promise((settle) => {
      const timer = setTimeout(() => settle("timeout"), timeoutMs);
      child.once("exit", (code) => {
        clearTimeout(timer);
        settle(code === 0 ? "exited" : `status ${String(code)}`);
      });
    });
    if (exited !== "exited") {
      child.kill("SIGKILL");
      throw new Error(`The probe did not finish: ${exited}`);
    }
    const appData = resolve(runtime, "data", "com.redvelo.elephant-ladder.fixture-runner");
    const evidence = JSON.parse(readFileSync(resolve(appData, "fixture-evidence.json"), "utf8"));
    writeFileSync(resolve(output, "fixture-evidence.json"), JSON.stringify(evidence, null, 2));
    return {
      environment: {
        os: { platform: platform(), release: release(), architecture: arch() },
        browserEngine: { name: "CEF" },
      },
      evidence,
    };
  } finally {
    rmSync(runtime, { recursive: true, force: true });
  }
}

async function collectAndroid(output) {
  const serial = attachedDevice();
  const environment = deviceEnvironment(serial);
  validateDeviceEnvironment(environment);
  const previousStayAwake = adb(serial, [
    "shell",
    "settings",
    "get",
    "global",
    "stay_on_while_plugged_in",
  ]);
  adb(serial, ["shell", "svc", "power", "stayon", "true"]);
  try {
    buildFixtureApplication(output, [fixtureId]);
    adb(serial, ["install", "-r", APK], { capture: false });
    await stopFixtureApplication(serial);
    if (adb(serial, ["shell", "pm", "clear", APPLICATION_ID]) !== "Success")
      throw new Error("Could not clear the isolated fixture package data");
    adb(serial, ["shell", "pm", "enable", "--user", "0", APPLICATION_ID], { allowFailure: true });
    const staging = "/data/local/tmp/elephant-engine-support";
    adb(serial, ["shell", "mkdir", "-p", staging]);
    adb(serial, ["push", fixture, `${staging}/${fixtureId}.epub`], { capture: false });
    adb(serial, ["push", plan, `${staging}/observation.json`], { capture: false });
    const packageDirectory = adb(serial, ["shell", "run-as", APPLICATION_ID, "pwd"]);
    const epubPath = `${packageDirectory}/${fixtureId}.epub`;
    const planPath = `${packageDirectory}/observation.json`;
    adb(serial, [
      "shell",
      "run-as",
      APPLICATION_ID,
      "sh",
      "-c",
      `'cp ${staging}/${fixtureId}.epub ${epubPath} && cp ${staging}/observation.json ${planPath}'`,
    ]);
    adb(serial, ["shell", "rm", "-rf", staging]);
    adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
    adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
    await launchFixtureApplication(serial, [
      "--es",
      ANDROID_FIXTURE_EPUB_EXTRA,
      epubPath,
      "--es",
      ANDROID_FIXTURE_OBSERVATION_EXTRA,
      planPath,
    ]);
    await waitForTerminalArtifact(serial, timeoutMs);
    const terminal = parseJson(readDeviceArtifact(serial, TERMINAL_FILE), "terminal artifact");
    const collector = createRunnerEventCollector(
      fixtureId,
      evidenceFixtureIdentity(digest(fixture)),
    );
    collector.consume(JSON.stringify(terminal));
    if (collector.terminal === null) throw new Error("The probe supplied no terminal result");
    if (collector.terminal.runStatus !== "complete")
      throw new Error(`The probe did not finish: ${collector.terminal.incompleteReason}`);
    const bytes = readDeviceArtifact(serial, collector.terminal.evidence.file, 8 * 1_048_576);
    if (digestBytes(bytes) !== collector.terminal.evidence.sha256)
      throw new Error("Probe evidence does not match its terminal metadata");
    const evidence = validateFixtureEvidenceArtifact(
      parseJson(bytes, "fixture evidence"),
      collector.terminal,
    );
    writeFileSync(resolve(output, "fixture-evidence.json"), bytes, { mode: 0o600 });
    return { environment, evidence };
  } finally {
    await cleanupDevice(serial, previousStayAwake).catch(() => undefined);
    restoreProductArtifact();
  }
}

async function main() {
  const android = process.argv.includes("--android");
  const unknown = process.argv.slice(2).filter((argument) => argument !== "--android");
  if (unknown.length > 0) throw new Error(`Unknown arguments: ${unknown.join(" ")}`);
  if (!existsSync(fixture))
    throw new Error("The probe fixture is missing; run pnpm fixtures:engine-support");
  const outputRoot = resolve(root, "test-results/engine-support", android ? "android" : "linux");
  mkdirSync(outputRoot, { recursive: true });
  const output = mkdtempSync(
    resolve(outputRoot, `${new Date().toISOString().replaceAll(":", "-")}-`),
  );
  const collected = android ? await collectAndroid(output) : await collectLinux(output);
  const answers = readAnswers(collected.evidence);
  writeJson(resolve(output, "manifest.json"), {
    schema: evidenceSchema,
    generatedAt: new Date().toISOString(),
    fixture: { file: basename(fixture), sha256: digest(fixture) },
    environment: collected.environment,
    application: android ? applicationSource(version()) : { binary: basename(binary) },
    answers,
  });
  print(answers);
  process.stdout.write(`\nEvidence: ${output}\n`);
}

function version() {
  return JSON.parse(readFileSync(resolve(root, "package.json"), "utf8")).version;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
    process.exitCode = 1;
  });
}
