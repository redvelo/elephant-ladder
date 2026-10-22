import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ANDROID_FIXTURE_EPUB_EXTRA,
  ANDROID_FIXTURE_OBSERVATION_EXTRA,
  APK,
  APPLICATION_ID,
  PRODUCT_APPLICATION_ID,
  TERMINAL_FILE,
  adb,
  applicationSource,
  attachedDevice,
  buildFixtureApplication,
  cleanupDevice,
  clearDeviceLog,
  deviceEnvironment,
  digestBytes,
  installedApplicationState,
  launchFixtureApplication,
  parseJson,
  readDeviceArtifact,
  readDeviceLog,
  restoreProductArtifact,
  stopFixtureApplication,
  validateDeviceEnvironment,
  waitForTerminalArtifact,
  writeJson,
} from "./android-fixture-runner.mjs";
import { corpusEpubPath, UPSTREAM_COMMIT } from "./build-w3c-corpus.mjs";
import { selectTests } from "./collect-w3c-corpus-linux.mjs";
import {
  classifyRunnerResult,
  createRunnerEventCollector,
  evidenceFixtureIdentity,
  validateFixtureEvidenceArtifact,
} from "./fixture-runner-contract.mjs";
import { DEFAULT_OBSERVATION, judge, readCatalog, readExpectations } from "./w3c-corpus.mjs";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const evidenceSchema = "elephant-w3c-corpus-android-evidence-v1";
const maximumEvidenceBytes = 8 * 1_048_576;
const stagingDirectory = "/data/local/tmp/elephant-w3c-corpus";
const terminalTimeoutMs = (spineLength) => 90_000 + 15_000 * spineLength;

/**
 * The fixture package reads the publication from its own data directory: the EPUB
 * and the observation plan are pushed through shared storage and copied in as the
 * package, so the launch names paths the host can open.
 */
export function androidCorpusLaunchArguments(epubPath, planPath) {
  return [
    "--es",
    ANDROID_FIXTURE_EPUB_EXTRA,
    epubPath,
    "--es",
    ANDROID_FIXTURE_OBSERVATION_EXTRA,
    planPath,
  ];
}

async function main() {
  const catalog = readCatalog();
  const expectations = readExpectations();
  const tests = selectTests(catalog, process.argv.slice(2));
  const outputRoot = resolve(root, "test-results/w3c-corpus/android");
  mkdirSync(outputRoot, { recursive: true });
  const output = mkdtempSync(
    resolve(outputRoot, `${new Date().toISOString().replaceAll(":", "-")}-`),
  );
  process.stdout.write(`W3C corpus Android evidence: ${output}\n`);
  let serial;
  let environment = null;
  let productBefore;
  let application = null;
  let previousStayAwake;
  let cleanup = { status: "not-needed" };
  let localProductArtifact = { status: "not-needed" };
  let suiteError;
  const runs = [];
  try {
    if (APPLICATION_ID === PRODUCT_APPLICATION_ID)
      throw new Error("Fixture package must not use the product application ID");
    for (const test of tests) {
      const epub = corpusEpubPath(test.id);
      if (!existsSync(epub) || digestBytes(readFileSync(epub)) !== test.sha256) {
        throw new Error(`Cached corpus EPUB is missing or stale; run pnpm corpus:w3c (${epub})`);
      }
    }
    serial = attachedDevice();
    environment = deviceEnvironment(serial);
    validateDeviceEnvironment(environment);
    productBefore = installedApplicationState(serial, PRODUCT_APPLICATION_ID);
    application = applicationSource(
      JSON.parse(readFileSync(resolve(root, "package.json"), "utf8")).version,
    );
    previousStayAwake = adb(serial, [
      "shell",
      "settings",
      "get",
      "global",
      "stay_on_while_plugged_in",
    ]);
    adb(serial, ["shell", "svc", "power", "stayon", "true"]);
    adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
    adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
    application.artifact = buildFixtureApplication(
      output,
      tests.map((test) => test.id),
    );
    adb(serial, ["install", "-r", APK], { capture: false });
    adb(serial, ["shell", "mkdir", "-p", stagingDirectory]);
    const packageDirectory = adb(serial, ["shell", "run-as", APPLICATION_ID, "pwd"]);
    if (!packageDirectory.startsWith("/"))
      throw new Error("Could not resolve the package directory");
    for (const test of tests) {
      const run = await collectTest(
        serial,
        test,
        expectations.tests[test.id],
        output,
        environment,
        application,
        packageDirectory,
      );
      runs.push(run);
      process.stdout.write(
        `${test.id}: ${run.status}${run.verdict === null ? "" : ` verdict=${String(run.verdict)}`}${run.error === undefined ? "" : ` (${run.error})`}\n`,
      );
    }
  } catch (cause) {
    suiteError = cause instanceof Error ? cause.message : String(cause);
  } finally {
    if (previousStayAwake !== undefined) {
      adb(serial, ["shell", "rm", "-rf", stagingDirectory], { allowFailure: true });
      try {
        await cleanupDevice(serial, previousStayAwake);
        cleanup = { status: "complete" };
      } catch (cause) {
        cleanup = {
          status: "failed",
          error: cause instanceof Error ? cause.message : String(cause),
        };
      }
      const productAfter = installedApplicationState(serial, PRODUCT_APPLICATION_ID);
      if (JSON.stringify(productAfter) !== JSON.stringify(productBefore)) {
        const error = "Production application state changed during corpus collection";
        cleanup = {
          ...cleanup,
          status: "failed",
          error: cleanup.error === undefined ? error : `${cleanup.error}; ${error}`,
        };
      }
      try {
        localProductArtifact = restoreProductArtifact();
      } catch (cause) {
        localProductArtifact = {
          status: "failed",
          error: cause instanceof Error ? cause.message : String(cause),
        };
      }
    }
  }
  const verdicts = Object.fromEntries(runs.map((run) => [run.id, run.verdict]));
  writeJson(resolve(output, "manifest.json"), {
    schema: evidenceSchema,
    generatedAt: new Date().toISOString(),
    upstream: { repository: catalog.upstream.repository, commit: UPSTREAM_COMMIT },
    environment,
    application,
    cleanup,
    localProductArtifact,
    runs: runs.map(({ id, status, verdict, reason, error }) => ({
      id,
      status,
      verdict,
      reason,
      manifest: `${id}/manifest.json`,
      ...(error === undefined ? {} : { error }),
    })),
    verdicts,
    ...(suiteError === undefined ? {} : { suiteError }),
  });
  const unrecorded = runs.filter((run) => run.status !== "evidence-recorded");
  const decided = runs.filter((run) => run.verdict !== null);
  process.stdout.write(
    `Recorded ${runs.length - unrecorded.length}/${runs.length} tests; ${decided.length} decided (${decided.filter((run) => run.verdict === true).length} pass, ${decided.filter((run) => run.verdict === false).length} fail, ${decided.filter((run) => run.verdict === "n/a").length} n/a); evidence: ${output}\n`,
  );
  if (suiteError !== undefined) throw new Error(`${suiteError}; evidence: ${output}`);
  if (cleanup.status !== "complete") throw new Error(`Android cleanup failed; evidence: ${output}`);
  if (localProductArtifact.status !== "restored")
    throw new Error(`Product Android artifact restoration failed; evidence: ${output}`);
  if (unrecorded.length > 0) {
    process.stderr.write(`${unrecorded.length} tests recorded no evidence\n`);
    process.exitCode = 1;
  }
}

async function collectTest(
  serial,
  test,
  expectation,
  output,
  environment,
  application,
  packageDirectory,
) {
  const runOutput = resolve(output, test.id);
  mkdirSync(runOutput, { recursive: true });
  const observation = expectation?.observation ?? DEFAULT_OBSERVATION;
  const plan = resolve(runOutput, "observation.json");
  writeJson(plan, observation);
  const collector = createRunnerEventCollector(test.id, evidenceFixtureIdentity(test.sha256));
  let status = "infrastructure-failure";
  let error;
  let evidence = null;
  let judgement = { verdict: null, reason: "no evidence" };
  try {
    await stopFixtureApplication(serial);
    if (adb(serial, ["shell", "pm", "clear", APPLICATION_ID]) !== "Success")
      throw new Error("Could not clear the isolated fixture package data");
    adb(serial, ["shell", "pm", "enable", "--user", "0", APPLICATION_ID], { allowFailure: true });
    const stagedEpub = `${stagingDirectory}/${test.id}.epub`;
    const stagedPlan = `${stagingDirectory}/${test.id}.observation.json`;
    adb(serial, ["push", corpusEpubPath(test.id), stagedEpub], { capture: false });
    adb(serial, ["push", plan, stagedPlan], { capture: false });
    const epubPath = `${packageDirectory}/corpus/${test.id}.epub`;
    const planPath = `${packageDirectory}/corpus/${test.id}.observation.json`;
    // adb shell joins its arguments and the device shell splits them again, so the
    // script travels quoted; nothing in these paths needs escaping.
    adb(serial, [
      "shell",
      "run-as",
      APPLICATION_ID,
      "sh",
      "-c",
      `'mkdir -p corpus && cp ${stagedEpub} ${epubPath} && cp ${stagedPlan} ${planPath}'`,
    ]);
    adb(serial, ["shell", "rm", "-f", stagedEpub, stagedPlan]);
    adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
    adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
    clearDeviceLog(serial);
    const pid = await launchFixtureApplication(
      serial,
      androidCorpusLaunchArguments(epubPath, planPath),
    );
    await waitForTerminalArtifact(serial, terminalTimeoutMs(test.spineLength));
    // Read before the process is stopped, since the host's own events live in its log.
    writeFileSync(resolve(runOutput, "runner.log"), readDeviceLog(serial, pid), { mode: 0o600 });
    const terminalBytes = readDeviceArtifact(serial, TERMINAL_FILE);
    collector.consume(JSON.stringify(parseJson(terminalBytes, "terminal artifact")));
    if (collector.terminal === null) throw new Error("Native fixture supplied no terminal result");
    writeFileSync(resolve(runOutput, TERMINAL_FILE), terminalBytes, { mode: 0o600 });
    status = classifyRunnerResult(collector.terminal);
    if (collector.terminal.runStatus === "complete") {
      const evidenceBytes = readDeviceArtifact(
        serial,
        collector.terminal.evidence.file,
        maximumEvidenceBytes,
      );
      if (
        evidenceBytes.byteLength !== collector.terminal.evidence.bytes ||
        digestBytes(evidenceBytes) !== collector.terminal.evidence.sha256
      ) {
        throw new Error("Fixture evidence does not match terminal metadata");
      }
      const evidenceValue = validateFixtureEvidenceArtifact(
        parseJson(evidenceBytes, "fixture evidence"),
        collector.terminal,
      );
      writeFileSync(resolve(runOutput, collector.terminal.evidence.file), evidenceBytes, {
        mode: 0o600,
      });
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
    try {
      await stopFixtureApplication(serial);
    } catch (cause) {
      const stopError = cause instanceof Error ? cause.message : String(cause);
      error = error === undefined ? stopError : `${error}; ${stopError}`;
      status = "infrastructure-failure";
    }
  }
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
    observation: {
      file: "observation.json",
      sha256: createHash("sha256").update(readFileSync(plan)).digest("hex"),
    },
    environment,
    application: { ...application, selectedTest: test.id },
    result: collector.terminal,
    evidence,
    ...(error === undefined ? {} : { error }),
  });
  return {
    id: test.id,
    status,
    verdict: judgement.verdict,
    reason: judgement.reason,
    ...(error === undefined ? {} : { error }),
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
    process.exitCode = 1;
  });
}
