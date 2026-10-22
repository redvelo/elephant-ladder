import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import path from "node:path";

// The NDK supplies clang, libc++ and the linker for every native library the app ships, so a
// newer one installed beside it would quietly change the binaries. Keep in step with
// ndkVersion in src-tauri/gen/android/app/build.gradle.kts, which the qualification suite
// checks.
export const NDK_VERSION = "29.0.14206865";

export function androidJavaEnv(
  args,
  env = process.env,
  { run = spawnSync, access = accessSync, platform = process.platform } = {},
) {
  if (args[0] !== "android") return env;

  const paths = platform === "win32" ? path.win32 : path.posix;
  const suffix = platform === "win32" ? ".exe" : "";
  const explicitHome = env.JAVA_HOME?.trim() ? env.JAVA_HOME : undefined;
  const source = explicitHome ? "JAVA_HOME" : "PATH";
  const failure = (reason) =>
    new Error(
      `Android Java (${source}): ${reason}. Set JAVA_HOME to a usable JDK 17 through 26 (Gradle 9.5.0).`,
    );
  let result;
  try {
    result = run(
      explicitHome ? paths.join(explicitHome, "bin", `java${suffix}`) : `java${suffix}`,
      ["-XshowSettings:properties", "-version"],
      { env, encoding: "utf8", timeout: 10_000, maxBuffer: 1024 * 1024, windowsHide: true },
    );
  } catch {
    throw failure("could not run java");
  }
  if (!result || result.error || result.signal || result.status !== 0) {
    throw failure("java is missing, unusable, or timed out");
  }

  const output = `${result.stderr ?? ""}\n${result.stdout ?? ""}`;
  const property = (name) => {
    const values = [
      ...output.matchAll(
        new RegExp(`^[^\\S\\r\\n]*${name}[^\\S\\r\\n]*=[^\\S\\r\\n]*([^\\r\\n]*)$`, "gm"),
      ),
    ];
    return values.length === 1 ? values[0][1].trim() : "";
  };
  const detectedHome = property("java\\.home");
  const version = property("java\\.specification\\.version");
  if (!paths.isAbsolute(detectedHome) || !/^(?:1\.[1-8]|[1-9]\d*)$/.test(version)) {
    throw failure("java returned missing or malformed home/version properties");
  }
  const major = Number(version.replace(/^1\./, ""));
  // Keep this runtime range aligned with the pinned Gradle wrapper.
  if (major < 17 || major > 26) throw failure("unsupported Java major (required: 17 through 26)");

  const home = explicitHome ?? detectedHome;
  if (!paths.isAbsolute(home)) throw failure("JAVA_HOME must be an absolute JDK path");
  try {
    access(paths.join(home, "bin", `javac${suffix}`), constants.X_OK);
  } catch {
    throw failure("JDK compiler javac is missing or not executable");
  }
  const sdkRoots = [
    env.ANDROID_HOME?.trim(),
    env.ANDROID_SDK_ROOT?.trim(),
    env.HOME?.trim() ? path.join(env.HOME, "Android", "Sdk") : undefined,
  ].filter((root, index, values) => root !== undefined && values.indexOf(root) === index);
  let sdkRoot;
  let ndkHome = env.NDK_HOME?.trim();
  if (ndkHome === undefined) {
    for (const candidate of sdkRoots) {
      const found = pinnedNdk(candidate, access);
      if (found !== undefined) {
        sdkRoot = candidate;
        ndkHome = found;
        break;
      }
    }
  } else {
    sdkRoot = sdkRoots.find((candidate) => ndkHome.startsWith(`${candidate}${path.sep}`));
  }
  return {
    ...env,
    JAVA_HOME: home,
    ...(sdkRoot === undefined ? {} : { ANDROID_HOME: sdkRoot, ANDROID_SDK_ROOT: sdkRoot }),
    // Tauri reads NDK_HOME, but a dependency that builds C through CMake is handed a
    // CMAKE_SYSTEM_NAME of Android and finds the toolchain only under the names the NDK
    // itself documents, so the same path is published under all three.
    ...(ndkHome === undefined
      ? {}
      : { NDK_HOME: ndkHome, ANDROID_NDK_HOME: ndkHome, ANDROID_NDK_ROOT: ndkHome }),
  };
}

function pinnedNdk(sdkRoot, access) {
  const candidate = path.join(sdkRoot, "ndk", NDK_VERSION);
  try {
    access(path.join(candidate, "ndk-build"), constants.X_OK);
    return candidate;
  } catch {
    return undefined;
  }
}
