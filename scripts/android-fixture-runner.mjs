// What every Android fixture-runner collector needs: the isolated fixture package, the
// device it runs on, the APK that carries it, and the artifacts it leaves behind.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const FIXTURE_CONFIG = "src-tauri/tauri.fixture-runner.android.conf.json";
export const APK = resolve(
  root,
  "src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk",
);
export const APPLICATION_ID = "com.redvelo.elephant_ladder.fixture_runner";
export const PRODUCT_APPLICATION_ID = "com.redvelo.elephant_ladder";
export const ACTIVITY = `${APPLICATION_ID}/com.redvelo.elephant_ladder.MainActivity`;
export const TERMINAL_FILE = "fixture-run-finished.json";
export const ANDROID_FIXTURE_ID_EXTRA = `${APPLICATION_ID}.FIXTURE_ID`;
export const ANDROID_FIXTURE_EPUB_EXTRA = `${APPLICATION_ID}.FIXTURE_EPUB`;
export const ANDROID_FIXTURE_OBSERVATION_EXTRA = `${APPLICATION_ID}.FIXTURE_OBSERVATION`;
// Android runs the Wry runtime; the CEF one is Linux only. Recorded in evidence so a
// run names the runtime it was taken against.
export const TAURI_RUNTIME = "tauri-runtime-wry 3.0.0-alpha.4";
const maximumArtifactBytes = 1024 * 1024;
const fixtureCargoTarget = resolve(root, "src-tauri/target/fixture-runner-android");
const sdkRoots = [
  process.env.ANDROID_HOME?.trim(),
  process.env.ANDROID_SDK_ROOT?.trim(),
  process.env.HOME === undefined ? undefined : resolve(process.env.HOME, "Android/Sdk"),
].filter((value, index, values) => value !== undefined && values.indexOf(value) === index);
const aapt =
  sdkRoots
    .map((sdkRoot) => resolve(sdkRoot, "build-tools/35.0.0/aapt"))
    .find((candidate) => existsSync(candidate)) ??
  resolve(sdkRoots[0] ?? ".", "build-tools/35.0.0/aapt");

export function parseWebViewPackage(value) {
  const match = value.match(/Current WebView package.*\(([^,]+),\s*([^)]+)\)/u);
  return match === null
    ? { package: "unavailable", version: "unavailable" }
    : {
        package: match[1].trim(),
        version: match[2].trim(),
      };
}

export function buildFixtureApplication(output, supportedFixtures) {
  rmSync(APK, { force: true });
  run(
    "pnpm",
    [
      "tauri",
      "android",
      "build",
      "--debug",
      "--apk",
      "--target",
      "aarch64",
      "--ci",
      "--config",
      FIXTURE_CONFIG,
      "--features",
      "fixture-runner",
    ],
    {
      capture: false,
      env: {
        ...process.env,
        CARGO_TARGET_DIR: fixtureCargoTarget,
      },
    },
  );
  if (!existsSync(APK)) throw new Error(`Fixture APK is missing: ${APK}`);
  const metadata = run(aapt, ["dump", "badging", APK]);
  const apkApplicationId = metadata.match(/^package: name='([^']+)'/m)?.[1];
  const debuggable = metadata.includes("application-debuggable");
  if (apkApplicationId !== APPLICATION_ID || debuggable !== true) {
    throw new Error("Built APK does not have the isolated debuggable fixture identity");
  }
  const retainedApk = resolve(output, "fixture-runner.apk");
  copyFileSync(APK, retainedApk);
  chmodSync(retainedApk, 0o600);
  return {
    file: basename(retainedApk),
    sha256: digestBytes(readFileSync(retainedApk)),
    supportedFixtures,
    packageId: apkApplicationId,
    debuggable: true,
  };
}

const maximumDeviceLogBytes = 16 * 1024 * 1024;

/// Drops whatever the device has buffered so a run's log contains only its own output.
export function clearDeviceLog(serial) {
  adb(serial, ["logcat", "-c"], { allowFailure: true });
}

/// What the host printed while this process ran, which is where its own structured
/// events land on Android. Returns an empty string when the log cannot be read, since
/// a missing log must never fail a run that otherwise recorded its result.
export function readDeviceLog(serial, pid) {
  if (pid === undefined) return "";
  const result = spawnSync("adb", ["-s", serial, "logcat", "-d", `--pid=${pid}`, "-v", "brief"], {
    encoding: "utf8",
    maxBuffer: maximumDeviceLogBytes,
  });
  if (result.status !== 0) return "";
  return result.stdout ?? "";
}

export async function waitForTerminalArtifact(serial, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const probe = adb(serial, ["shell", "run-as", APPLICATION_ID, "ls", TERMINAL_FILE], {
      allowFailure: true,
    });
    if (probe === TERMINAL_FILE) return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 250));
  }
  throw new Error("Android fixture terminal artifact timed out");
}

export async function launchFixtureApplication(serial, launchArguments) {
  let failure = "";
  for (let attempt = 0; attempt < 3; attempt += 1) {
    // Some devices disable the package again after a process death, and `am start`
    // reports a missing activity on stderr while still exiting zero, so the launch has
    // to be enabled immediately beforehand and confirmed by the process afterwards.
    adb(serial, ["shell", "pm", "enable", "--user", "0", APPLICATION_ID], { allowFailure: true });
    const started = adb(
      serial,
      ["shell", "am", "start", "-W", "-f", "0x10008000", "-n", ACTIVITY, ...launchArguments],
      { allowFailure: true, stderr: true },
    );
    if (started.includes("Status: ok") && !/^Error/mu.test(started)) {
      const deadline = Date.now() + 10_000;
      while (Date.now() < deadline) {
        const pid = adb(serial, ["shell", "pidof", APPLICATION_ID], { allowFailure: true });
        if (pid !== "") return pid.split(/\s+/u)[0];
        await new Promise((settle) => setTimeout(settle, 100));
      }
      failure = "the fixture process did not appear";
    } else {
      failure = started === "" ? "am start reported nothing" : started.replaceAll("\n", " ");
    }
  }
  throw new Error(`Could not launch the Android fixture application: ${failure}`);
}

export async function stopFixtureApplication(serial) {
  adb(serial, ["shell", "am", "force-stop", APPLICATION_ID]);
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (adb(serial, ["shell", "pidof", APPLICATION_ID], { allowFailure: true }) === "") return;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 100));
  }
  throw new Error("Android fixture process did not stop");
}

export function readDeviceArtifact(serial, file, maximumBytes = maximumArtifactBytes) {
  const result = spawnSync(
    "adb",
    ["-s", serial, "exec-out", "run-as", APPLICATION_ID, "cat", file],
    {
      encoding: null,
      maxBuffer: maximumBytes + 1,
    },
  );
  if (result.status !== 0) throw new Error(`Could not read Android fixture artifact ${file}`);
  const bytes = Buffer.from(result.stdout ?? []);
  if (bytes.byteLength < 1 || bytes.byteLength > maximumBytes) {
    throw new Error(`Android fixture artifact ${file} has an invalid size`);
  }
  return bytes;
}

export function parseJson(bytes, label) {
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch {
    throw new Error(`${label} is not valid JSON`);
  }
}

export function attachedDevice() {
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

export function deviceEnvironment(serial) {
  const webView = parseWebViewPackage(adb(serial, ["shell", "dumpsys", "webviewupdate"]));
  return {
    os: {
      platform: "android",
      release: adb(serial, ["shell", "getprop", "ro.build.version.release"]),
      apiLevel: Number(adb(serial, ["shell", "getprop", "ro.build.version.sdk"])),
      architecture: adb(serial, ["shell", "getprop", "ro.product.cpu.abi"]),
    },
    device: {
      manufacturer: adb(serial, ["shell", "getprop", "ro.product.manufacturer"]),
      model: adb(serial, ["shell", "getprop", "ro.product.model"]),
    },
    browserEngine: {
      name: "Android System WebView",
      package: webView.package,
      version: webView.version,
    },
  };
}

export function validateDeviceEnvironment(environment) {
  if (
    !Number.isInteger(environment.os.apiLevel) ||
    environment.os.apiLevel < 24 ||
    environment.os.architecture !== "arm64-v8a" ||
    environment.browserEngine.package === "unavailable" ||
    environment.browserEngine.version === "unavailable"
  ) {
    throw new Error(
      "Attached device does not provide the required arm64 Android/WebView environment",
    );
  }
}

export function installedApplicationState(serial, packageId) {
  const dump = adb(serial, ["shell", "dumpsys", "package", packageId], { allowFailure: true });
  if (!dump.includes(`Package [${packageId}]`)) return { installed: false };
  return {
    installed: true,
    versionName: dump.match(/versionName=([^\s]+)/u)?.[1] ?? "unavailable",
    dataIdentity: dump.match(/ceDataInode=(\d+)/u)?.[1] ?? "unavailable",
    debuggable: /flags=\[[^\]]*DEBUGGABLE/u.test(dump),
  };
}

export async function cleanupDevice(serial, previousStayAwake) {
  let cleanupError;
  try {
    await stopFixtureApplication(serial);
    if (adb(serial, ["shell", "pm", "clear", APPLICATION_ID]) !== "Success") {
      throw new Error("Could not clear the isolated fixture package after qualification");
    }
    const staleArtifact = adb(serial, ["shell", "run-as", APPLICATION_ID, "ls", TERMINAL_FILE], {
      allowFailure: true,
    });
    if (staleArtifact === TERMINAL_FILE) {
      throw new Error("Fixture package cleanup did not remove artifacts");
    }
  } catch (cause) {
    cleanupError = cause instanceof Error ? cause.message : String(cause);
  }
  try {
    const expectedStayAwake = previousStayAwake === "null" ? "0" : previousStayAwake;
    adb(serial, [
      "shell",
      "settings",
      "put",
      "global",
      "stay_on_while_plugged_in",
      expectedStayAwake,
    ]);
    const restoredStayAwake = adb(serial, [
      "shell",
      "settings",
      "get",
      "global",
      "stay_on_while_plugged_in",
    ]);
    if (restoredStayAwake !== expectedStayAwake) {
      throw new Error("Could not restore the previous USB stay-awake setting");
    }
  } catch (cause) {
    const restoreError = cause instanceof Error ? cause.message : String(cause);
    cleanupError = cleanupError === undefined ? restoreError : `${cleanupError}; ${restoreError}`;
  }
  if (cleanupError !== undefined) throw new Error(cleanupError);
}

export function restoreProductArtifact() {
  rmSync(APK, { force: true });
  run("pnpm", ["tauri", "android", "build", "--debug", "--apk", "--target", "aarch64", "--ci"], {
    capture: false,
  });
  if (!existsSync(APK)) throw new Error(`Product APK is missing after restoration: ${APK}`);
  const metadata = run(aapt, ["dump", "badging", APK]);
  const packageId = metadata.match(/^package: name='([^']+)'/m)?.[1];
  const debuggable = metadata.includes("application-debuggable");
  if (packageId !== PRODUCT_APPLICATION_ID || debuggable !== true) {
    throw new Error("Restored Android artifact does not have the production debug identity");
  }
  return {
    status: "restored",
    file: basename(APK),
    sha256: digestBytes(readFileSync(APK)),
    packageId,
    debuggable: true,
  };
}

export function adb(serial, args, options = {}) {
  const result = spawnSync("adb", ["-s", serial, ...args], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: maximumArtifactBytes + 1,
    stdio: options.capture === false ? "inherit" : "pipe",
  });
  if (result.status !== 0 && !options.allowFailure) {
    const detail = result.stderr?.trim() || result.stdout?.trim();
    throw new Error(`adb ${args.join(" ")} failed${detail ? `: ${detail}` : ""}`);
  }
  const output =
    options.stderr === true
      ? `${result.stdout ?? ""}${result.stderr ?? ""}`
      : (result.stdout ?? "");
  return output.trim();
}

export function run(program, args, options = {}) {
  const result = spawnSync(program, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: maximumArtifactBytes + 1,
    stdio: options.capture === false ? "inherit" : "pipe",
    env: options.env ?? process.env,
  });
  if (result.status !== 0 && !options.allowFailure) {
    const detail = result.stderr?.trim() || result.stdout?.trim();
    throw new Error(`${program} ${args.join(" ")} failed${detail ? `: ${detail}` : ""}`);
  }
  return (result.stdout ?? "").trim();
}

export function sourceState(directory) {
  const commit = run("git", ["-C", directory, "rev-parse", "HEAD"]);
  const status = spawnSync("git", ["-C", directory, "status", "--porcelain"], { encoding: "utf8" });
  return { commit, dirty: status.status === 0 ? status.stdout.trim() !== "" : null };
}

export function digestBytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function writeJson(path, value) {
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

export function applicationSource(version) {
  return {
    version,
    packageId: APPLICATION_ID,
    tauriRuntime: TAURI_RUNTIME,
    source: {
      elephantLadder: sourceState(root),
      navigator: sourceState(resolve(root, "../navigator")),
      stack: sourceState(resolve(root, "../stack")),
    },
  };
}
