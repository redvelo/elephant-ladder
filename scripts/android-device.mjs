import { spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const applicationId = "com.redvelo.elephant_ladder";
const activity = `${applicationId}/.MainActivity`;
const debugApk = resolve(
  root,
  "src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk",
);
const releaseOutputDirectory = resolve(
  root,
  "src-tauri/gen/android/app/build/outputs/apk/universal/release",
);
const unsignedReleaseApk = resolve(releaseOutputDirectory, "app-universal-release-unsigned.apk");
const signedReleaseApk = resolve(releaseOutputDirectory, "app-universal-release-signed.apk");

function fail(message) {
  throw new Error(message);
}

export function parseApkApplicationId(value) {
  return value.match(/^package: name='([^']+)'/mu)?.[1] ?? null;
}

function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    stdio: options.capture === false ? "inherit" : "pipe",
  });
  if (result.status !== 0 && !options.allowFailure) {
    const detail = result.stderr?.trim() || result.stdout?.trim();
    fail(`${program} ${args.join(" ")} failed${detail ? `: ${detail}` : ""}`);
  }
  return (result.stdout ?? "").trim();
}

function adb(serial, args, options) {
  return run("adb", ["-s", serial, ...args], options);
}

function androidTool(name) {
  const locator = process.platform === "win32" ? "where" : "which";
  const installed = run(locator, [name], { allowFailure: true }).split(/\r?\n/u)[0] ?? "";
  if (installed !== "") return installed;
  let sdk = process.env.ANDROID_HOME ?? process.env.ANDROID_SDK_ROOT;
  if (sdk === undefined) {
    const properties = resolve(root, "src-tauri/gen/android/local.properties");
    if (existsSync(properties)) {
      const match = readFileSync(properties, "utf8").match(/^sdk\.dir=(.+)$/mu);
      sdk = match?.[1]?.replaceAll("\\\\", "\\").replaceAll("\\:", ":");
    }
  }
  if (sdk === undefined) fail(`could not locate Android SDK tool ${name}`);
  const directory = resolve(sdk, "build-tools");
  const versions = readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }));
  for (const version of versions) {
    const candidate = resolve(directory, version, name);
    if (existsSync(candidate)) return candidate;
  }
  fail(`could not locate Android SDK tool ${name}`);
}

function signReleaseArtifact() {
  if (!existsSync(unsignedReleaseApk)) fail(`release APK is missing: ${unsignedReleaseApk}`);
  const keystore = resolve(homedir(), ".android/debug.keystore");
  if (!existsSync(keystore)) fail(`Android debug keystore is missing: ${keystore}`);
  const apksigner = androidTool("apksigner");
  run(apksigner, [
    "sign",
    "--ks",
    keystore,
    "--ks-key-alias",
    "androiddebugkey",
    "--ks-pass",
    "pass:android",
    "--key-pass",
    "pass:android",
    "--out",
    signedReleaseApk,
    unsignedReleaseApk,
  ]);
  run(apksigner, ["verify", "--verbose", signedReleaseApk]);
  return signedReleaseApk;
}

function sleep(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}

async function retry(action, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await action();
      if (value !== null && value !== undefined && value !== "") return value;
    } catch (cause) {
      lastError = cause;
    }
    await sleep(250);
  }
  throw new Error(
    `${description} timed out${lastError instanceof Error ? `: ${lastError.message}` : ""}`,
  );
}

function attachedDevice() {
  const requested = process.env.ANDROID_SERIAL;
  const devices = run("adb", ["devices"])
    .split("\n")
    .slice(1)
    .map((line) => line.trim().split(/\s+/, 2))
    .filter(([, state]) => state === "device")
    .map(([serial]) => serial);
  if (requested !== undefined) {
    if (!devices.includes(requested)) fail("ANDROID_SERIAL is not an attached authorized device");
    return requested;
  }
  if (devices.length !== 1) fail("Attach exactly one authorized device or set ANDROID_SERIAL");
  return devices[0];
}

/**
 * The featured control is a node in the WebView's DOM, and this device never exposes the
 * WebView's accessibility tree to `uiautomator dump`: the hierarchy is a bare FrameLayout and

 * WebView. The application's own DevTools endpoint is the only way to reach it.
 */
async function forwardDevTools(serial) {
  // The WebView publishes its socket after the process is up, and the process can still be
  // replaced in between, so the live process list is re-read on every attempt.
  const socket = await retry(
    async () => {
      const open = adb(serial, ["shell", "cat", "/proc/net/unix"], { allowFailure: true });
      return (
        adb(serial, ["shell", "pidof", applicationId], { allowFailure: true })
          .split(/\s+/u)
          .filter(Boolean)
          .map((processId) => `webview_devtools_remote_${processId}`)
          .find((name) => open.includes(name)) ?? null
      );
    },
    60_000,
    "application DevTools socket",
  );
  const port = Number(adb(serial, ["forward", "tcp:0", `localabstract:${socket}`]));
  if (!Number.isInteger(port) || port <= 0) fail(`could not forward ${socket}`);
  return port;
}

/**
 * The home screen's controls live in the WebView's DOM, which this device never exposes to
 * `uiautomator dump`, and the reader arbitrates touch rather than mouse. Both are reachable only
 * through the application's own DevTools endpoint.
 */
async function withReaderPage(serial, body) {
  const port = await forwardDevTools(serial);
  const { chromium } = await import("@playwright/test");
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  try {
    const page = browser.contexts().flatMap((context) => context.pages())[0];
    if (page === undefined) fail("the application exposed no DevTools page");
    const input = await page.context().newCDPSession(page);
    const tap = async (x, y) => {
      const touchPoints = [
        { x: Math.round(x), y: Math.round(y), radiusX: 4, radiusY: 4, force: 1, id: 1 },
      ];
      await input.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints });
      await sleep(60);
      await input.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    };
    const tapSelector = async (selector, description) => {
      const centre = await retry(
        async () => {
          const handle = await page.$(selector);
          const box = handle === null ? null : await handle.boundingBox();
          return box === null || box.width === 0 || box.height === 0
            ? null
            : { x: box.x + box.width / 2, y: box.y + box.height / 2 };
        },
        30_000,
        description,
      );
      await tap(centre.x, centre.y);
    };
    return await body({ page, tap, tapSelector });
  } finally {
    await browser.close();
    adb(serial, ["forward", "--remove", `tcp:${port}`], { allowFailure: true });
  }
}

/** The reader prints its position as "page / total" in its own chrome, outside the frame. */
function readerPosition(page) {
  return page.evaluate(() => {
    const reader = document.querySelector(".epub-navigator-reader");
    if (reader === null) return null;
    const match = /(\d+)\s*\/\s*~?\d+/u.exec(reader.textContent ?? "");
    return match === null ? null : Number(match[1]);
  });
}

async function openFeaturedPublication({ page, tapSelector }) {
  // The home screen takes a moment to render, and an empty library offers the bundled sample
  // instead of a featured publication, so wait for whichever of the two arrives.
  const entry = await retry(
    async () => {
      if ((await page.$(".hero-title-button")) !== null) return "featured";
      return (await page.$("text=Open the sample book")) !== null ? "sample" : null;
    },
    60_000,
    "the home screen offering a publication",
  );
  if (entry === "sample") {
    // Seeding imports the sample without opening it, which leaves the page cache untouched.
    await tapSelector("text=Open the sample book", "sample publication control");
    await retry(async () => page.$(".hero-title-button"), 60_000, "the seeded publication");
  }
  await tapSelector(".hero-title-button", "featured publication control");
  await retry(
    async () =>
      (await page.evaluate(
        () =>
          document.querySelector(".epub-navigator-reader")?.getAttribute("data-reader-state") ??
          null,
      )) === "reading" || null,
    60_000,
    "reader reaching its reading state",
  );
}

async function smoke(serial, skipBuild, buildMode, turns) {
  if (!skipBuild) {
    const arguments_ = ["tauri", "android", "build", "--apk", "--target", "aarch64", "--ci"];
    if (buildMode === "debug") arguments_.splice(3, 0, "--debug");
    run("pnpm", arguments_, { capture: false });
  }
  const apk = buildMode === "debug" ? debugApk : signReleaseArtifact();
  if (parseApkApplicationId(run(androidTool("aapt"), ["dump", "badging", apk])) !== applicationId) {
    fail("Android smoke APK has the wrong application ID");
  }
  adb(serial, ["install", "-r", apk], { capture: false });
  adb(serial, ["shell", "pm", "enable", "--user", "0", applicationId], { allowFailure: true });
  adb(serial, ["shell", "input", "keyevent", "KEYCODE_WAKEUP"], { allowFailure: true });
  adb(serial, ["shell", "wm", "dismiss-keyguard"], { allowFailure: true });
  adb(serial, ["shell", "am", "force-stop", applicationId]);
  adb(serial, ["shell", "am", "start", "-W", "-n", activity]);
  await withReaderPage(serial, async (session) => {
    const { page, tap } = session;
    await openFeaturedPublication(session);
    const { width, height } = await page.evaluate(() => ({
      width: window.innerWidth,
      height: window.innerHeight,
    }));
    const start = await retry(async () => readerPosition(page), 60_000, "a reading position");
    /** Taps and waits for the page to change; null means this direction had nowhere to go. */
    const turn = async (x, previous) => {
      await tap(x, height * 0.5);
      for (let waited = 0; waited < 15_000; waited += 250) {
        const current = await readerPosition(page);
        if (current !== null && current !== previous) return current;
        await sleep(250);
      }
      return null;
    };

    // A publication resumed on its last page cannot turn forward, so take whichever direction
    // moves rather than working it out from the page count, which is still settling.
    const forward = width * 0.88;
    const backward = width * 0.12;
    let lead = forward;
    let first = await turn(lead, start);
    if (first === null) {
      lead = backward;
      first = await turn(lead, start);
    }
    if (first === null) fail(`neither direction turned a page from ${start}`);
    const trail = lead === forward ? backward : forward;
    const heading = Math.sign(first - start);

    const visited = [start, first];
    const step = async (x, expected) => {
      const next = await turn(x, visited.at(-1));
      if (next === null) fail(`pages stopped turning: ${visited.join(" -> ")}`);
      if (Math.sign(next - visited.at(-1)) !== expected) {
        fail(`pages turned the wrong way: ${[...visited, next].join(" -> ")}`);
      }
      visited.push(next);
    };
    for (let remaining = turns - 1; remaining > 0; remaining -= 1) await step(lead, heading);
    for (let remaining = turns; remaining > 0; remaining -= 1) await step(trail, -heading);
    if (visited.at(-1) !== start) {
      fail(`reading position did not return to ${start}: ${visited.join(" -> ")}`);
    }
    console.log(`Android smoke passed; reading positions ${visited.join(" -> ")}`);
  });
}

function doctor(serial) {
  const model = adb(serial, ["shell", "getprop", "ro.product.model"]);
  const api = adb(serial, ["shell", "getprop", "ro.build.version.sdk"]);
  const webview = adb(serial, ["shell", "dumpsys", "webviewupdate"])
    .split("\n")
    .find((line) => line.includes("Current WebView package"))
    ?.trim();
  console.log(`Device ready: model=${model || "unknown"} api=${api || "unknown"}`);
  if (webview !== undefined) console.log(webview);
}

async function main() {
  const commandName = process.argv[2] ?? "help";
  const skipBuild = process.argv.includes("--skip-build");
  const buildMode = process.argv.includes("--release") ? "release" : "debug";
  const turnsArgument = process.argv.find((argument) => argument.startsWith("--turns="));
  const turns = Number(turnsArgument?.split("=", 2)[1] ?? "2");
  if (!Number.isInteger(turns) || turns < 1) fail("--turns must be a positive integer");
  if (commandName === "doctor") doctor(attachedDevice());
  else if (commandName === "smoke") await smoke(attachedDevice(), skipBuild, buildMode, turns);
  else {
    console.log(`Usage:
  node scripts/android-device.mjs doctor
  node scripts/android-device.mjs smoke [--release] [--skip-build] [--turns=N]

Set ANDROID_SERIAL when more than one device is attached.`);
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exitCode = 1;
  });
}
