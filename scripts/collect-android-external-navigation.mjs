import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PRODUCT_APPLICATION_ID = "com.redvelo.elephant_ladder";
export const CATEGORY_BROWSABLE = "android.intent.category.BROWSABLE";

const actionView = "android.intent.action.VIEW";
const activity = `${PRODUCT_APPLICATION_ID}/.MainActivity`;
const maximumCommandBytes = 1024 * 1024;
const maximumObservations = 128;
const maximumUrlCharacters = 2048;
const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

function normalizedHttpUrl(value) {
  if (typeof value !== "string" || value.length > maximumUrlCharacters) {
    throw new Error("Expected external URL must be an absolute HTTP(S) URL within 2048 characters");
  }
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error("Expected external URL must be an absolute HTTP(S) URL");
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.hostname === "" ||
    url.username !== "" ||
    url.password !== ""
  ) {
    throw new Error("Expected external URL must be an absolute HTTP(S) URL without credentials");
  }
  return url;
}

function urlEvidence(url) {
  return {
    scheme: url.protocol.slice(0, -1),
    host: url.hostname.toLowerCase(),
    port: url.port || null,
    path: url.pathname,
  };
}

export function createBrowsableIntentExpectation(value) {
  return {
    action: actionView,
    category: CATEGORY_BROWSABLE,
    uri: urlEvidence(normalizedHttpUrl(value)),
  };
}

function safeComponent(packageName, activityName, taskId) {
  if (
    packageName.length > 255 ||
    activityName.length > 255 ||
    !/^[A-Za-z0-9_.]+$/u.test(packageName) ||
    !/^[A-Za-z0-9_.$]+$/u.test(activityName)
  ) {
    return null;
  }
  return {
    packageName,
    activityName,
    component: `${packageName}/${activityName}`,
    taskId: Number.isSafeInteger(taskId) && taskId >= 0 ? taskId : null,
  };
}

/** Parse only the current foreground component from cross-version activity dumps. */
export function parseForegroundActivity(dump) {
  const labels = ["topResumedActivity", "mResumedActivity", "mFocusedApp"];
  for (const label of labels) {
    const line = dump.split("\n").find((candidate) => candidate.includes(label));
    if (line === undefined) continue;
    const component =
      line.match(/\bu\d+\s+([A-Za-z0-9_.]+)\/([A-Za-z0-9_.$]+)\b/u) ??
      line.match(/\b([A-Za-z0-9_.]+)\/([A-Za-z0-9_.$]+)\b/u);
    if (component === null) continue;
    const task = line.match(/\bt(\d+)\b/u) ?? line.match(/\btaskId=(\d+)\b/u);
    return safeComponent(component[1], component[2], task === null ? null : Number(task[1]));
  }
  return null;
}

function parseIntentUri(value) {
  try {
    return urlEvidence(normalizedHttpUrl(value));
  } catch {
    return null;
  }
}

/** Parse bounded, normalized task intent facts without retaining a raw dumpsys artifact. */
export function parseActivityTaskEvidence(dump) {
  const browsableIntents = [];
  let taskId = null;
  for (const line of dump.split("\n")) {
    const task =
      line.match(/\bTask\{[^}]*\s#(\d+)\b/u) ??
      line.match(/\btaskId=(\d+)\b/u) ??
      line.match(/\bTask\s+#(\d+)\b/u);
    if (task !== null) taskId = Number(task[1]);
    const body = line.match(/(?:\bIntent\s+)?\{([^}\n]*)\}/u)?.[1];
    if (body === undefined) continue;
    const action = body.match(/\bact=([^\s}]+)/u)?.[1];
    const categories = body.match(/\bcat=\[([^\]]*)\]/u)?.[1]?.split(/[,\s]+/u) ?? [];
    const data = body.match(/\bdat=([^\s}]+)/u)?.[1];
    if (action !== actionView || !categories.includes(CATEGORY_BROWSABLE) || data === undefined)
      continue;
    const uri = parseIntentUri(data);
    if (uri === null || browsableIntents.length >= 8) continue;
    const componentText = body.match(/\bcmp=([A-Za-z0-9_.]+)\/([A-Za-z0-9_.$]+)/u);
    const component =
      componentText === null ? null : safeComponent(componentText[1], componentText[2], taskId);
    browsableIntents.push({
      action,
      categories: [CATEGORY_BROWSABLE],
      uri,
      taskId: Number.isSafeInteger(taskId) ? taskId : null,
      ...(component === null ? {} : { component: component.component }),
    });
  }
  return { foreground: parseForegroundActivity(dump), browsableIntents };
}

function sameUri(left, right) {
  return (
    left.scheme === right.scheme &&
    left.host === right.host &&
    left.port === right.port &&
    left.path === right.path
  );
}

function matchingIntent(observation, expectation) {
  return observation.browsableIntents.some(
    (intent) =>
      intent.action === expectation.action &&
      intent.categories.includes(expectation.category) &&
      sameUri(intent.uri, expectation.uri),
  );
}

export function assessReturnContinuity(
  observations,
  expectation,
  applicationId = PRODUCT_APPLICATION_ID,
) {
  const initialIndex = observations.findIndex(
    ({ foreground }) => foreground?.packageName === applicationId && foreground.taskId !== null,
  );
  const initial = observations[initialIndex];
  const externalIndex =
    initialIndex < 0
      ? -1
      : observations.findIndex(
          ({ foreground }, index) =>
            index > initialIndex && foreground !== null && foreground.packageName !== applicationId,
        );
  const returnedIndex =
    externalIndex < 0
      ? -1
      : observations.findIndex(
          ({ foreground }, index) =>
            index > externalIndex && foreground?.packageName === applicationId,
        );
  const intentObserved =
    externalIndex >= 0 &&
    observations
      .slice(initialIndex + 1, returnedIndex < 0 ? undefined : returnedIndex + 1)
      .some((observation) => matchingIntent(observation, expectation));
  const sameAppTask =
    returnedIndex >= 0 &&
    observations[returnedIndex].foreground.taskId !== null &&
    observations[returnedIndex].foreground.taskId === initial.foreground.taskId;
  const checks = {
    appInitiallyForeground: initialIndex >= 0,
    matchingBrowsableIntent: intentObserved,
    externalForeground: externalIndex >= 0,
    appReturned: returnedIndex >= 0,
    sameAppTask,
  };
  return {
    status: Object.values(checks).every(Boolean) ? "pass" : "incomplete",
    checks,
    transition: {
      initialIndex,
      externalIndex,
      returnedIndex,
      initialTaskId: initial?.foreground.taskId ?? null,
      returnedTaskId: returnedIndex < 0 ? null : observations[returnedIndex].foreground.taskId,
    },
  };
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: maximumCommandBytes + 1,
  });
  if (result.error?.code === "ENOBUFS")
    throw new Error(`${program} output exceeded the evidence bound`);
  if (result.status !== 0 && !options.allowFailure) {
    const detail = (result.stderr?.trim() || result.stdout?.trim() || "").slice(0, 500);
    const sanitized =
      options.serial === undefined ? detail : detail.replaceAll(options.serial, "[device]");
    throw new Error(
      `${program} ${args.filter((arg) => arg !== options.serial).join(" ")} failed${sanitized ? `: ${sanitized}` : ""}`,
    );
  }
  return (result.stdout ?? "").trim();
}

function adb(serial, args, options = {}) {
  return run("adb", ["-s", serial, ...args], { ...options, serial });
}

function attachedDevice() {
  const requested = process.env.ANDROID_SERIAL;
  const devices = run("adb", ["devices"])
    .split("\n")
    .slice(1)
    .map((line) => line.trim().split(/\s+/u, 2))
    .filter(([, state]) => state === "device")
    .map(([serial]) => serial);
  if (requested !== undefined) {
    if (!devices.includes(requested))
      throw new Error("ANDROID_SERIAL is not an attached authorized device");
    return requested;
  }
  if (devices.length !== 1)
    throw new Error("Attach exactly one authorized device or set ANDROID_SERIAL");
  return devices[0];
}

function boundedProperty(serial, name) {
  const value = adb(serial, ["shell", "getprop", name]);
  return value.replace(/[^\x20-\x7e]/gu, "").slice(0, 120) || "unavailable";
}

function observationKey(value) {
  return JSON.stringify(value);
}

async function sleep(milliseconds) {
  await new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

async function main() {
  const expectedArgument = process.argv
    .slice(2)
    .find((argument) => argument.startsWith("--expected-url="));
  if (expectedArgument === undefined) {
    throw new Error(
      "Usage: node scripts/collect-android-external-navigation.mjs --expected-url=https://example.com/path [--timeout-seconds=180]",
    );
  }
  const expectedUrl = expectedArgument.slice("--expected-url=".length);
  const expectation = createBrowsableIntentExpectation(expectedUrl);
  const timeoutArgument = process.argv
    .slice(2)
    .find((argument) => argument.startsWith("--timeout-seconds="));
  const timeoutMs = Number(timeoutArgument?.slice("--timeout-seconds=".length) ?? "180") * 1000;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 10_000 || timeoutMs > 900_000) {
    throw new Error("--timeout-seconds must be between 10 and 900");
  }

  const serial = attachedDevice();
  if (adb(serial, ["shell", "pm", "path", PRODUCT_APPLICATION_ID], { allowFailure: true }) === "") {
    throw new Error(`Product application ${PRODUCT_APPLICATION_ID} is not installed`);
  }
  adb(serial, ["shell", "pm", "enable", "--user", "0", PRODUCT_APPLICATION_ID]);
  adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
  adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
  const launch = adb(serial, ["shell", "am", "start", "-W", "-n", activity]);
  if (launch.includes("Error type") || launch.includes("Error:")) {
    throw new Error("Android did not launch the Elephant Ladder activity");
  }

  const observations = [];
  const startedAt = new Date().toISOString();
  const deadline = Date.now() + timeoutMs;
  let lastKey = "";
  let announcedExternal = false;
  console.log(`Open a book, activate the link to ${expectedUrl}, then return to Elephant Ladder.`);
  while (Date.now() < deadline && observations.length < maximumObservations) {
    const parsed = parseActivityTaskEvidence(
      adb(serial, ["shell", "dumpsys", "activity", "activities"]),
    );
    const key = observationKey(parsed);
    if (key !== lastKey) {
      observations.push({ sequence: observations.length + 1, ...parsed });
      lastKey = key;
    }
    const assessment = assessReturnContinuity(observations, expectation);
    if (assessment.checks.externalForeground && !announcedExternal) {
      console.log("External browser or chooser observed; return to Elephant Ladder now.");
      announcedExternal = true;
    }
    if (assessment.status === "pass") break;
    await sleep(250);
  }

  const assessment = assessReturnContinuity(observations, expectation);
  const outputDirectory = resolve(root, "test-results/android-external-navigation");
  mkdirSync(outputDirectory, { recursive: true });
  const output = resolve(outputDirectory, "manifest.json");
  const evidence = `${JSON.stringify(
    {
      schema: "elephant-ladder-android-external-navigation-v1",
      startedAt,
      finishedAt: new Date().toISOString(),
      environment: {
        platform: "android",
        release: boundedProperty(serial, "ro.build.version.release"),
        apiLevel: Number(boundedProperty(serial, "ro.build.version.sdk")),
        manufacturer: boundedProperty(serial, "ro.product.manufacturer"),
        model: boundedProperty(serial, "ro.product.model"),
      },
      applicationId: PRODUCT_APPLICATION_ID,
      expectation,
      observations,
      assessment,
    },
    null,
    2,
  )}\n`;
  if (Buffer.byteLength(evidence) > maximumCommandBytes) {
    throw new Error("Sanitized external navigation evidence exceeded 1 MiB");
  }
  writeFileSync(output, evidence, { mode: 0o600 });
  if (assessment.status !== "pass") {
    throw new Error(`External navigation qualification is incomplete; evidence: ${output}`);
  }
  console.log(`External navigation qualification passed; evidence: ${output}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  });
}
