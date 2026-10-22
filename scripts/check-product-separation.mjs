import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const output = resolve(root, "dist");
const forbidden = [
  "Fixture runner",
  "active-probe-v1",
  "elephant-active-probe-result-v1",
  "elephant-container-constrained-result-v1",
  "elephant-remote-policy-result-v1",
  "elephant-publication-snapshot-request-v1",
  "elephant-publication-snapshot-v1",
  "elephant-w3c-scr-support-result-v1",
  "elephant-w3c-scr-readingsystem-support-result-v1",
  "elephant-w3c-scr-support-svg-result-v1",
  "elephant-w3c-scr-support-iframe-result-v1",
  "fixture-runner",
  "container-constrained-v1",
  "remote-policy-denied-v1",
  "w3c-scr-support-v1",
  "w3c-scr-readingsystem-support-v1",
  "w3c-scr-support-svg-v1",
  "w3c-scr-support-iframe-v1",
];
const inspectedExtensions = new Set([".css", ".html", ".js", ".json"]);

for (const path of files(output)) {
  if (/fixture-runner/u.test(path))
    throw new Error(`Production output contains a fixture-runner asset: ${path}`);
  if (!inspectedExtensions.has(extname(path))) continue;
  const contents = readFileSync(path, "utf8");
  const marker = forbidden.find((value) => contents.includes(value));
  if (marker !== undefined)
    throw new Error(
      `Production output contains fixture-runner marker ${JSON.stringify(marker)} in ${path}`,
    );
}

// The bundled output is only half of it: the conformance crate must not be in the
// product binary's dependency graph either. The release profile strips symbols, so
// the graph is the only thing that proves this.
const dependsOnConformance = (features) => {
  const result = spawnSync(
    "cargo",
    ["tree", "-p", "elephant-ladder", "-e", "normal", ...features],
    { cwd: resolve(root, "src-tauri"), encoding: "utf8" },
  );
  if (result.status !== 0) throw new Error(`cargo tree failed: ${result.stderr}`);
  return result.stdout.includes("elephant-ladder-conformance");
};
if (dependsOnConformance([])) throw new Error("The product binary links the conformance crate.");
if (!dependsOnConformance(["--features", "fixture-runner"]))
  throw new Error("The fixture-runner build does not link the conformance crate.");

process.stdout.write(
  "Production output contains no fixture-runner assets or contract markers, " +
    "and the product binary does not link the conformance crate.\n",
);

function* files(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) yield* files(path);
    else if (entry.isFile()) yield path;
  }
}
