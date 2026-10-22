import { readdirSync, readFileSync } from "node:fs";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const configurations = [
  "src-tauri/tauri.conf.json",
  "src-tauri/tauri.fixture-runner.conf.json",
  "src-tauri/tauri.fixture-runner.android.conf.json",
];

for (const path of configurations) {
  const configuration = JSON.parse(readFileSync(resolve(root, path), "utf8"));
  if (
    !Array.isArray(configuration.app?.security?.capabilities) ||
    configuration.app.security.capabilities.length !== 0
  ) {
    throw new Error(`${path} must keep browser Tauri capabilities empty`);
  }
}

for (const path of files(resolve(root, "src"))) {
  if (![".js", ".ts", ".tsx"].includes(extname(path))) continue;
  const contents = readFileSync(path, "utf8");
  if (contents.includes("@tauri-apps/") || contents.includes("__TAURI_INTERNALS__")) {
    throw new Error(`Browser source must not use Tauri IPC: ${path}`);
  }
}

process.stdout.write("Browser Tauri IPC is disabled and all capability sets are empty.\n");

function* files(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else if (entry.isFile()) yield path;
  }
}
