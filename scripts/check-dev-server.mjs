// A successful build does not mean the development server boots: dependency optimization only
// runs on first request there. Boot the server, request the entry, and fail on anything it reports.
import { spawn } from "node:child_process";
import { rm } from "node:fs/promises";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const READY_TIMEOUT_MS = 90_000;

const port = await freePort();
// A warm cache skips optimization, which is the step being checked.
await rm(new URL("../node_modules/.vite", import.meta.url), { recursive: true, force: true });

const log = [];
// Spawned directly: a package-manager shim does not forward the termination signal.
const viteBin = fileURLToPath(new URL("../node_modules/vite/bin/vite.js", import.meta.url));
const server = spawn(
  process.execPath,
  [viteBin, "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
  {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
  },
);
for (const stream of [server.stdout, server.stderr]) {
  stream.setEncoding("utf8");
  stream.on("data", (chunk) => log.push(chunk));
}

let failure = null;
try {
  await waitForReady();
  await expectOk("/src/main.tsx");
  await expectOk("/src/app/app.tsx");
  await expectOptimized("jotai");
  const optimizerError = log.join("").match(/MISSING_EXPORT|Error during dependency optimization/u);
  if (optimizerError !== null) fail(`dependency optimization failed: ${optimizerError[0]}`);
} catch (cause) {
  failure = cause;
} finally {
  server.kill("SIGTERM");
}

if (failure === null) {
  process.stdout.write("dev server served the application entry\n");
} else {
  process.stderr.write(`${failure instanceof Error ? failure.message : "check failed"}\n`);
  process.stderr.write(`\n--- dev server output ---\n${log.join("")}\n`);
  process.exitCode = 1;
}

function fail(message) {
  throw new Error(message);
}

/** Vite holds module requests open while it optimizes, so no request may wait indefinitely. */
async function get(path, timeoutMs = 30_000) {
  try {
    return await fetch(`http://127.0.0.1:${String(port)}${path}`, {
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch {
    return null;
  }
}

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.on("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (address === null || typeof address === "string") {
        probe.close(() => reject(new Error("could not reserve a port")));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
}

async function waitForReady() {
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      fail(`dev server exited early with code ${String(server.exitCode)}`);
    }
    if ((await get("/", 5_000))?.ok === true) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  fail("dev server did not become ready");
}

async function expectOk(path) {
  const response = await get(path);
  if (response === null) fail(`${path} did not respond`);
  if (!response.ok) fail(`${path} responded ${String(response.status)}`);
}

/** The optimizer writes a dependency's bundle only once it has processed it without error. */
async function expectOptimized(dependency) {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if ((await get(`/node_modules/.vite/deps/${dependency}.js`, 10_000))?.ok === true) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  fail(`${dependency} was never optimized`);
}
