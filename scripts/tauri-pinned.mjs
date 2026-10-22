import { spawnSync } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { androidJavaEnv } from "./android-java.mjs";

// The CLI owns CEF package assembly, so it is pinned to the same release as the runtime
// crates rather than tracked separately; upgrade them together.
const TAURI_CLI_VERSION = "3.0.0-alpha.4";
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const installRoot = path.join(
  repositoryRoot,
  "src-tauri",
  "target",
  `tauri-cli-${TAURI_CLI_VERSION}`,
);
const executable = path.join(
  installRoot,
  "bin",
  process.platform === "win32" ? "cargo-tauri.exe" : "cargo-tauri",
);

let childEnv;
try {
  childEnv = androidJavaEnv(process.argv.slice(2));
  if (process.argv[0] === "android") {
    const configIndex = process.argv.indexOf("--config");
    const config = configIndex >= 0 ? process.argv[configIndex + 1] : undefined;
    if (config !== undefined) {
      const configValue = JSON.parse(readFileSync(path.resolve(repositoryRoot, config), "utf8"));
      const debugSuffix = configValue?.bundle?.android?.debugApplicationIdSuffix;
      childEnv = {
        ...childEnv,
        TAURI_ANDROID_CONFIG: path.relative(
          path.join(repositoryRoot, "src-tauri"),
          path.resolve(repositoryRoot, config),
        ),
        ...(typeof debugSuffix === "string"
          ? { TAURI_ANDROID_DEBUG_APPLICATION_ID_SUFFIX: debugSuffix }
          : {}),
      };
    }
  }
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

// A `dev` run serves the reader bundle from disk, so a frontend rebuild needs no Rust
// rebuild. Only debug binaries honour this, and nothing sets it for a packaged build.
if (process.argv.slice(2).includes("dev")) {
  const runner = process.argv.includes("--features") && process.argv.includes("fixture-runner");
  childEnv = {
    ...childEnv,
    ELEPHANT_READER_ASSETS: path.join(repositoryRoot, runner ? "dist-fixture-runner" : "dist"),
  };
}

try {
  accessSync(executable, constants.X_OK);
} catch {
  const install = spawnSync(
    "cargo",
    ["install", "tauri-cli", "--version", TAURI_CLI_VERSION, "--locked", "--root", installRoot],
    { cwd: repositoryRoot, stdio: "inherit" },
  );
  if (install.error) throw install.error;
  if (install.status !== 0) process.exit(install.status ?? 1);
}

// `VAR=value command` is not a thing outside POSIX shells, so the target directory an isolated
// build needs is passed as a flag and turned into the environment variable Cargo reads.
const forwarded = process.argv
  .slice(2)
  .filter((argument) => !argument.startsWith("--cargo-target-dir="));
const targetDirectory = process.argv
  .find((argument) => argument.startsWith("--cargo-target-dir="))
  ?.slice("--cargo-target-dir=".length);
if (targetDirectory !== undefined) {
  // Passed through unresolved: Cargo runs with `src-tauri` as its directory, so a relative value
  // lands beside the default target directory, which is where the collectors look for it.
  childEnv = { ...childEnv, CARGO_TARGET_DIR: targetDirectory };
}

const command = spawnSync(executable, forwarded, {
  cwd: repositoryRoot,
  stdio: "inherit",
  env: childEnv,
});
if (command.error) throw command.error;
process.exit(command.status ?? 1);
