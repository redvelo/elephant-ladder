import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PRODUCT_APPLICATION_ID = "com.redvelo.elephant_ladder";

const activity = `${PRODUCT_APPLICATION_ID}/.MainActivity`;
const hierarchy = "/data/local/tmp/elephant-ladder-media-overlays.xml";
const maximumCommandBytes = 4 * 1024 * 1024;
const lockedListeningMs = 45_000;
const root = resolve(fileURLToPath(new URL("..", import.meta.url)));

export function countBackgroundMutes(audioDump, packageName = PRODUCT_APPLICATION_ID) {
  return audioDump
    .split("\n")
    .filter((line) => line.includes(`AudioHardening background playback muted for ${packageName} `))
    .length;
}

export function narrationServiceForeground(servicesDump) {
  const start = servicesDump.indexOf(`${PRODUCT_APPLICATION_ID}/.NarrationService`);
  if (start < 0) return false;
  const next = servicesDump.indexOf("* ServiceRecord", start + 1);
  const record = servicesDump.slice(start, next < 0 ? undefined : next);
  return /isForeground=true\b/u.test(record) && /\btypes=0x0*2\b/u.test(record);
}

export function mediaSessionState(sessionDump, packageName = PRODUCT_APPLICATION_ID) {
  const start = sessionDump.indexOf(`package=${packageName}`);
  if (start < 0) return null;
  const match = sessionDump
    .slice(start, start + 4000)
    .match(/state=PlaybackState \{state=([A-Z_]+)\(/u);
  return match?.[1] ?? null;
}

export function mediaSessionMetadata(sessionDump, packageName = PRODUCT_APPLICATION_ID) {
  const start = sessionDump.indexOf(`package=${packageName}`);
  if (start < 0) return null;
  const match = sessionDump
    .slice(start, start + 4000)
    .match(/metadata: size=(\d+), description=(.*)/u);
  if (match === null) return null;
  const fields = match[2].split(", ").filter((field) => field !== "null" && field !== "");
  return { size: Number(match[1]), fields };
}

export function playerStarted(audioDump, uid) {
  return new RegExp(`u/pid:${uid}/\\d+ state:started\\b`, "u").test(audioDump);
}

function run(program, args, { allowFailure = false, serial } = {}) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: maximumCommandBytes + 1,
  });
  if (result.error?.code === "ENOBUFS") throw new Error(`${program} output exceeded 4 MiB`);
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

async function waitFor(check, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = check();
    if (value !== null && value !== false) return value;
    await sleep(500);
  }
  throw new Error(`${description} timed out`);
}

function uiNodes(serial) {
  adb(serial, ["shell", "uiautomator", "dump", "--compressed", hierarchy], { allowFailure: true });
  const xml = adb(serial, ["shell", "cat", hierarchy], { allowFailure: true });
  return (xml.match(/<node\b[^>]*>/gu) ?? []).map((node) =>
    Object.fromEntries(
      [...node.matchAll(/([\w-]+)="([^"]*)"/gu)].map((match) => [match[1], match[2]]),
    ),
  );
}

function bounds(node) {
  const match = node.bounds?.match(/^\[(\d+),(\d+)\]\[(\d+),(\d+)\]$/u);
  if (match === null || match === undefined) return null;
  const [, left, top, right, bottom] = match.map(Number);
  return right > left && bottom > top ? { left, top, right, bottom } : null;
}

function findLabeled(serial, label) {
  for (const node of uiNodes(serial)) {
    if (!label.test(node.text ?? "") && !label.test(node["content-desc"] ?? "")) continue;
    const box = bounds(node);
    if (box !== null) return box;
  }
  return null;
}

function tap(serial, x, y) {
  adb(serial, ["shell", "input", "tap", String(Math.round(x)), String(Math.round(y))]);
}

const audioDump = (serial) => adb(serial, ["shell", "dumpsys", "audio"]);
const sessionDump = (serial) => adb(serial, ["shell", "dumpsys", "media_session"]);
const sessionState = (serial) => mediaSessionState(sessionDump(serial));
const serviceForeground = (serial) =>
  narrationServiceForeground(
    adb(serial, ["shell", "dumpsys", "activity", "services", PRODUCT_APPLICATION_ID]),
  );

async function main() {
  const serial = attachedDevice();
  const uid = adb(serial, ["shell", "pm", "list", "packages", "-U", PRODUCT_APPLICATION_ID]).match(
    /uid:(\d+)/u,
  )?.[1];
  if (uid === undefined)
    throw new Error(`Product application ${PRODUCT_APPLICATION_ID} is not installed`);
  adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"]);
  if (/isKeyguardShowing=true/u.test(adb(serial, ["shell", "dumpsys", "window"]))) {
    throw new Error("Unlock the device before narration qualification");
  }

  const checks = {};
  adb(serial, ["shell", "am", "force-stop", PRODUCT_APPLICATION_ID]);
  adb(serial, ["shell", "am", "start", "-W", "-n", activity]);
  const publication = await waitFor(
    () => findLabeled(serial, /^(Continue reading|Start reading)$/u),
    30_000,
    "featured publication action",
  );
  tap(
    serial,
    (publication.left + publication.right) / 2,
    (publication.top + publication.bottom) / 2,
  );
  const options = await waitFor(
    () => findLabeled(serial, /^Narration options$/u),
    90_000,
    "narration controls",
  );
  const width = options.right - options.left;
  tap(serial, options.left - width / 2 - 4, (options.top + options.bottom) / 2);
  await waitFor(
    () => serviceForeground(serial) && sessionState(serial) === "PLAYING",
    30_000,
    "narration playback service",
  );
  checks.foregroundServiceStarted = true;
  const metadata = mediaSessionMetadata(sessionDump(serial));
  checks.controlsCarrySectionAndTitle =
    metadata !== null && metadata.size >= 3 && new Set(metadata.fields).size >= 2;

  const mutesBeforeLock = countBackgroundMutes(audioDump(serial));
  adb(serial, ["shell", "input", "keyevent", "KEYCODE_SLEEP"]);
  await sleep(lockedListeningMs);
  const lockedAudio = audioDump(serial);
  checks.noBackgroundMuteWhileLocked = countBackgroundMutes(lockedAudio) === mutesBeforeLock;
  checks.playerStartedWhileLocked = playerStarted(lockedAudio, uid);
  checks.sessionPlayingWhileLocked = sessionState(serial) === "PLAYING";

  adb(serial, ["shell", "cmd", "media_session", "dispatch", "pause"]);
  checks.mediaPauseReachedReader = await waitFor(
    () => sessionState(serial) === "PAUSED",
    15_000,
    "media pause",
  )
    .then(() => true)
    .catch(() => false);
  adb(serial, ["shell", "cmd", "media_session", "dispatch", "play"]);
  checks.mediaPlayReachedReader = await waitFor(
    () => sessionState(serial) === "PLAYING",
    15_000,
    "media play",
  )
    .then(() => true)
    .catch(() => false);
  adb(serial, ["shell", "cmd", "media_session", "dispatch", "pause"]);
  adb(serial, ["shell", "rm", "-f", hierarchy], { allowFailure: true });

  const passed = Object.values(checks).every(Boolean);
  const outputDirectory = resolve(root, "test-results/android-media-overlays");
  mkdirSync(outputDirectory, { recursive: true });
  const output = resolve(outputDirectory, "manifest.json");
  writeFileSync(
    output,
    `${JSON.stringify(
      {
        schema: "elephant-ladder-android-media-overlays-v1",
        generatedAt: new Date().toISOString(),
        environment: {
          platform: "android",
          release: boundedProperty(serial, "ro.build.version.release"),
          apiLevel: Number(boundedProperty(serial, "ro.build.version.sdk")),
          manufacturer: boundedProperty(serial, "ro.product.manufacturer"),
          model: boundedProperty(serial, "ro.product.model"),
        },
        applicationId: PRODUCT_APPLICATION_ID,
        lockedListeningMs,
        status: passed ? "qualified" : "failed",
        checks,
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  if (!passed) throw new Error(`Android narration qualification failed; evidence: ${output}`);
  console.log(`Android narration qualified; evidence: ${output}. Unlock the device to continue.`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
