import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  ANDROID_FIXTURE_ID_EXTRA,
  APK,
  APPLICATION_ID,
  PRODUCT_APPLICATION_ID,
  TERMINAL_FILE,
  adb,
  applicationSource as describeApplication,
  attachedDevice,
  buildFixtureApplication,
  cleanupDevice,
  deviceEnvironment,
  digestBytes,
  installedApplicationState,
  launchFixtureApplication,
  parseJson,
  parseWebViewPackage,
  readDeviceArtifact,
  restoreProductArtifact,
  stopFixtureApplication,
  validateDeviceEnvironment,
  waitForTerminalArtifact,
  writeJson,
} from "./android-fixture-runner.mjs";
import {
  classifyRunnerResult,
  FIXTURE_IDS,
  createRunnerEventCollector,
  evaluateStorageIsolationRelation,
  parseFixtureIds,
  validatePublicationObservationsArtifact,
} from "./fixture-runner-contract.mjs";

export { ANDROID_FIXTURE_ID_EXTRA, parseWebViewPackage };

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
let output;
const applicationId = APPLICATION_ID;
const productApplicationId = PRODUCT_APPLICATION_ID;
const terminalFile = TERMINAL_FILE;
const evidenceSchema = "elephant-fixture-runner-android-evidence-v1";

export function androidFixtureLaunchArguments(fixtureId) {
  const [validatedFixtureId] = parseFixtureIds([fixtureId]);
  return ["--es", ANDROID_FIXTURE_ID_EXTRA, validatedFixtureId];
}

export function relativeAndroidFixtureArtifact(fixtureArtifact, runOutput, suiteOutput) {
  return {
    ...fixtureArtifact,
    file: relative(runOutput, resolve(suiteOutput, fixtureArtifact.file)),
  };
}

async function main() {
  const outputRoot = resolve(root, "test-results/fixture-runner/android");
  mkdirSync(outputRoot, { recursive: true });
  output = mkdtempSync(resolve(outputRoot, `${new Date().toISOString().replaceAll(":", "-")}-`));
  console.log(`Android fixture evidence: ${output}`);
  let fixtureIds = [];
  let serial;
  let environment = null;
  let productBefore;
  let applicationSource = null;
  let previousStayAwake;
  const runs = [];
  const relations = [];
  let fixtureArtifact = null;
  let suiteError;
  let cleanup = { status: "not-needed" };
  let localProductArtifact = { status: "not-needed" };
  try {
    if (applicationId === productApplicationId)
      throw new Error("Fixture package must not use the product application ID");
    fixtureIds = parseFixtureIds(process.argv.slice(2));
    serial = attachedDevice();
    environment = deviceEnvironment(serial);
    validateDeviceEnvironment(environment);
    productBefore = installedApplicationState(serial, productApplicationId);
    applicationSource = describeApplication(
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
    fixtureArtifact = buildFixtureApplication(output, [...FIXTURE_IDS]);
    adb(serial, ["install", "-r", APK], { capture: false });
    for (const fixtureId of fixtureIds) {
      runs.push(
        await collectFixture(serial, fixtureId, environment, applicationSource, fixtureArtifact),
      );
    }
    const relationFixtureIds = ["active-probe", "active-probe-twin"];
    if (relationFixtureIds.every((id) => fixtureIds.includes(id))) {
      await collectRelations(serial, environment, applicationSource, fixtureArtifact, (relation) =>
        relations.push(relation),
      );
    }
  } catch (cause) {
    suiteError = cause instanceof Error ? cause.message : String(cause);
  } finally {
    // Preflight failures have not changed the device or local APK.
    if (previousStayAwake !== undefined) {
      try {
        await cleanupDevice(serial, previousStayAwake);
        cleanup = { status: "complete" };
      } catch (cause) {
        cleanup = {
          status: "failed",
          error: cause instanceof Error ? cause.message : String(cause),
        };
      }
      const productAfter = installedApplicationState(serial, productApplicationId);
      if (JSON.stringify(productAfter) === JSON.stringify(productBefore)) {
        cleanup = { ...cleanup, productionApplicationUnchanged: true };
      } else {
        const error = "Production application state changed during fixture qualification";
        cleanup = {
          ...cleanup,
          status: "failed",
          error: cleanup.error === undefined ? error : `${cleanup.error}; ${error}`,
          productionApplicationUnchanged: false,
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

  writeJson(resolve(output, "manifest.json"), {
    schema: evidenceSchema,
    generatedAt: new Date().toISOString(),
    environment,
    application: {
      ...applicationSource,
      ...(fixtureArtifact === null ? {} : { artifact: fixtureArtifact }),
    },
    cleanup,
    localProductArtifact,
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
    ...(suiteError === undefined ? {} : { suiteError }),
  });

  const failed = runs.filter(({ status }) => status !== "automated-pass");
  if (
    suiteError !== undefined ||
    cleanup.status !== "complete" ||
    localProductArtifact.status !== "restored" ||
    failed.length > 0 ||
    relations.some(({ status }) => status !== "automated-pass")
  ) {
    if (suiteError !== undefined) throw new Error(`${suiteError}; evidence: ${output}`);
    if (cleanup.status !== "complete")
      throw new Error(`Android fixture cleanup failed; evidence: ${output}`);
    if (localProductArtifact.status !== "restored") {
      throw new Error(`Product Android artifact restoration failed; evidence: ${output}`);
    }
    const failedRelation = relations.find(({ status }) => status !== "automated-pass");
    if (failedRelation !== undefined) {
      throw new Error(`Android fixture relation ${failedRelation.id} failed; evidence: ${output}`);
    }
    throw new Error(
      `Android fixture suite has ${failed.length}/${runs.length} unsuccessful runs; evidence: ${output}`,
    );
  }
  console.log(
    `Android fixture suite passed ${runs.length}/${FIXTURE_IDS.length} fixtures (${fixtureIds.join(", ")}); evidence: ${output}`,
  );
}

async function collectFixture(
  serial,
  fixtureId,
  environment,
  applicationSource,
  fixtureArtifact,
  options = {},
) {
  const runOutput = options.runOutput ?? resolve(output, fixtureId);
  mkdirSync(runOutput, { recursive: true });
  let status = "infrastructure-failure";
  let error;
  let result = null;
  let terminalArtifact = null;
  let publicationObservations = null;
  let observationValue = null;

  try {
    await stopFixtureApplication(serial);
    if (
      options.clearData !== false &&
      adb(serial, ["shell", "pm", "clear", applicationId]) !== "Success"
    ) {
      throw new Error("Could not clear the isolated fixture package data");
    }
    adb(serial, ["shell", "pm", "enable", "--user", "0", applicationId], { allowFailure: true });
    adb(
      serial,
      [
        "shell",
        "run-as",
        applicationId,
        "rm",
        "-f",
        terminalFile,
        "fixture-publication-observations.json",
      ],
      { allowFailure: true },
    );
    adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
    adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
    await launchFixtureApplication(serial, androidFixtureLaunchArguments(fixtureId));
    await waitForTerminalArtifact(serial, 180_000);

    const terminalBytes = readDeviceArtifact(serial, terminalFile);
    const terminalValue = parseJson(terminalBytes, "terminal artifact");
    const collector = createRunnerEventCollector(fixtureId);
    collector.consume(JSON.stringify(terminalValue));
    result = collector.terminal;
    if (result === null) throw new Error("Native fixture supplied no terminal result");
    writeFileSync(resolve(runOutput, terminalFile), terminalBytes, { mode: 0o600 });
    terminalArtifact = { file: terminalFile, sha256: digestBytes(terminalBytes) };

    if (result.runStatus === "complete") {
      const observationsBytes = readDeviceArtifact(serial, result.publicationObservations.file);
      if (observationsBytes.byteLength !== result.publicationObservations.bytes) {
        throw new Error("Publication observations byte count does not match terminal metadata");
      }
      if (digestBytes(observationsBytes) !== result.publicationObservations.sha256) {
        throw new Error("Publication observations digest does not match terminal metadata");
      }
      const observations = parseJson(observationsBytes, "publication observations");
      validatePublicationObservationsArtifact(observations, result);
      observationValue = observations;
      writeFileSync(resolve(runOutput, result.publicationObservations.file), observationsBytes, {
        mode: 0o600,
      });
      publicationObservations = {
        file: result.publicationObservations.file,
        sha256: digestBytes(observationsBytes),
      };
    }
    status = classifyRunnerResult(result);
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

  const manifestPath = resolve(runOutput, "manifest.json");
  const runArtifact = relativeAndroidFixtureArtifact(fixtureArtifact, runOutput, output);
  writeJson(manifestPath, {
    schema: evidenceSchema,
    fixtureId,
    status,
    environment,
    application: { ...applicationSource, artifact: runArtifact, selectedFixture: fixtureId },
    result,
    terminalArtifact,
    publicationObservations,
    ...(error === undefined ? {} : { error }),
  });
  return {
    fixtureId,
    status,
    observationValue,
    evidence: {
      manifestSha256: digestBytes(readFileSync(manifestPath)),
      observationsSha256: publicationObservations?.sha256 ?? null,
    },
    ...(error === undefined ? {} : { error }),
  };
}

async function collectRelations(
  serial,
  environment,
  applicationSource,
  fixtureArtifact,
  onComplete,
) {
  const relationRoot = resolve(output, "relations");
  const runRelationFixture = (fixtureId, relationId, runId, clearData = false) =>
    collectFixture(serial, fixtureId, environment, applicationSource, fixtureArtifact, {
      clearData,
      runOutput: resolve(relationRoot, relationId, runId),
    });
  const primaryFirst = await runRelationFixture(
    "active-probe",
    "storage-isolation",
    "primary-first",
    true,
  );
  const twin = await runRelationFixture("active-probe-twin", "storage-isolation", "twin");
  const primarySecond = await runRelationFixture(
    "active-probe",
    "storage-isolation",
    "primary-second",
  );
  requirePassingRuns([primaryFirst, twin, primarySecond]);
  const storageRelation = evaluateStorageIsolationRelation(
    primaryFirst.observationValue,
    twin.observationValue,
    primarySecond.observationValue,
  );
  mkdirSync(resolve(relationRoot, storageRelation.id), { recursive: true });
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
  onComplete({ ...storageRelation, manifestSha256: digestBytes(readFileSync(storageManifest)) });
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

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  });
}
