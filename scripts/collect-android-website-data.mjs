import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PRODUCT_APPLICATION_ID = "com.redvelo.elephant_ladder";

const activity = `${PRODUCT_APPLICATION_ID}/.MainActivity`;
const maximumCommandBytes = 1024 * 1024;
const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

export function assessWebsiteDataPurgeLogs(log) {
  const platformAttempt = log.includes("publication-origin WebStorage purge attempted");
  const hostPartial = log
    .split("\n")
    .some(
      (line) =>
        line.includes('"event":"publication_website_data_reset"') &&
        line.includes('"physical_purge":"partial"'),
    );
  const promotedToPurged = log
    .split("\n")
    .some(
      (line) =>
        line.includes('"event":"publication_website_data_reset"') &&
        line.includes('"physical_purge":"purged"'),
    );
  const checks = {
    platformAttempt,
    hostPartial,
    neverPromotedToPurged: !promotedToPurged,
  };
  return {
    status: Object.values(checks).every(Boolean) ? "attempt-qualified" : "incomplete",
    checks,
  };
}

function run(program, args, { allowFailure = false, serial } = {}) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: maximumCommandBytes + 1,
  });
  if (result.error?.code === "ENOBUFS") throw new Error(`${program} output exceeded 1 MiB`);
  if (result.status !== 0 && !allowFailure) {
    const detail = (result.stderr?.trim() || result.stdout?.trim() || "").slice(0, 500);
    const sanitized = serial === undefined ? detail : detail.replaceAll(serial, "[device]");
    throw new Error(sanitized || `${program} failed`);
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
  return (
    adb(serial, ["shell", "getprop", name])
      .replace(/[^\x20-\x7e]/gu, "")
      .slice(0, 120) || "unavailable"
  );
}

async function sleep(milliseconds) {
  await new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

async function main() {
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
  const launch = adb(serial, ["shell", "am", "start", "-W", "-n", activity]);
  if (launch.includes("Error type") || launch.includes("Error:")) {
    throw new Error("Android did not launch the Elephant Ladder activity");
  }
  const pid = adb(serial, ["shell", "pidof", "-s", PRODUCT_APPLICATION_ID]);
  if (!/^\d+$/u.test(pid)) throw new Error("Could not identify the Elephant Ladder process");
  const baselineLog = adb(serial, ["logcat", "-d", `--pid=${pid}`, "-v", "brief"]);

  console.log("Reset website data for a closed test publication in Elephant Ladder.");
  const deadline = Date.now() + timeoutMs;
  let assessment = assessWebsiteDataPurgeLogs("");
  while (Date.now() < deadline) {
    const log = adb(serial, ["logcat", "-d", `--pid=${pid}`, "-v", "brief"]);
    assessment = assessWebsiteDataPurgeLogs(
      log.startsWith(baselineLog) ? log.slice(baselineLog.length) : "",
    );
    if (assessment.status === "attempt-qualified") break;
    await sleep(500);
  }

  const outputDirectory = resolve(root, "test-results/android-website-data");
  mkdirSync(outputDirectory, { recursive: true });
  const output = resolve(outputDirectory, "manifest.json");
  const evidence = `${JSON.stringify(
    {
      schema: "elephant-ladder-android-website-data-attempt-v1",
      generatedAt: new Date().toISOString(),
      environment: {
        platform: "android",
        release: boundedProperty(serial, "ro.build.version.release"),
        apiLevel: Number(boundedProperty(serial, "ro.build.version.sdk")),
        manufacturer: boundedProperty(serial, "ro.product.manufacturer"),
        model: boundedProperty(serial, "ro.product.model"),
      },
      applicationId: PRODUCT_APPLICATION_ID,
      claim: "per-origin-webstorage-attempt-only",
      unsupportedCategories: [
        "cookies",
        "cache-storage",
        "worker-storage",
        "http-authentication",
        "form-data",
        "opfs",
      ],
      assessment,
    },
    null,
    2,
  )}\n`;
  writeFileSync(output, evidence, { mode: 0o600 });
  if (assessment.status !== "attempt-qualified") {
    throw new Error(`Website-data purge attempt qualification is incomplete; evidence: ${output}`);
  }
  console.log(`Website-data purge attempt qualified; evidence: ${output}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  });
}
