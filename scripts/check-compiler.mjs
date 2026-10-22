// The React Compiler bails silently: a component or hook it skips gets no memoization and no
// diagnostic. Compile every source module and fail on any component or hook without a memo cache.
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { transformSync } from "oxc-transform-react";

const root = fileURLToPath(new URL("..", import.meta.url));
const declaration =
  /^(?:export\s+(?:default\s+)?)?(?:(?:async\s+)?function\s+([\w$]+)|const\s+([\w$]+)\s*=\s*(?:async\s*)?\()/gmu;
const topLevel =
  /^(?:export\s+(?:default\s+)?)?(?:async\s+)?(?:function|const|let|class)\s+([\w$]+)/gmu;
const compiled = (name) => /^use[A-Z0-9]/u.test(name) || /^[A-Z][a-z]/u.test(name);

// Holding no memoizable value is not a bailout. Each entry states why, and an entry that stops
// applying fails the check so the list cannot outlive its reason. Nothing needs one yet.
const allowedWithoutCache = new Map([]);

function* sources(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) yield* sources(path);
    else if (/\.tsx?$/u.test(entry.name) && !entry.name.endsWith(".d.ts")) yield path;
  }
}

const failures = [];
const applied = new Set();
let checked = 0;
let optedOut = 0;
for (const path of sources(join(root, "src"))) {
  const file = relative(root, path);
  const source = readFileSync(path, "utf8");
  const candidates = [...source.matchAll(declaration)]
    .map((match) => match[1] ?? match[2])
    .filter(compiled);
  if (candidates.length === 0) continue;
  const result = transformSync(file, source);
  if (result.fatal) {
    const messages = result.errors.map((error) => error.message).join("; ");
    failures.push(`${file}: transform failed: ${messages}`);
    continue;
  }
  const output = [...result.code.matchAll(topLevel)];
  for (const name of candidates) {
    const position = output.findIndex((match) => match[1] === name);
    if (position === -1) {
      failures.push(`${file}: ${name} is missing from compiler output`);
      continue;
    }
    const body = result.code.slice(output[position].index, output[position + 1]?.index);
    if (body.includes('"use no memo"')) {
      optedOut += 1;
      continue;
    }
    const allowed = `${file}:${name}`;
    if (body.includes("_c(")) {
      checked += 1;
      if (allowedWithoutCache.has(allowed)) {
        applied.add(allowed);
        failures.push(`${allowed} is memoized now; remove it from allowedWithoutCache`);
      }
      continue;
    }
    if (allowedWithoutCache.has(allowed)) applied.add(allowed);
    else failures.push(`${file}: ${name} has no compiler memo cache`);
  }
}

for (const allowed of allowedWithoutCache.keys()) {
  if (!applied.has(allowed)) failures.push(`${allowed} is no longer present; remove it`);
}

if (failures.length > 0) {
  process.stderr.write(`${failures.join("\n")}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(
    `react compiler memoized ${String(checked)} components and hooks ` +
      `(${String(applied.size)} hold nothing to cache, ${String(optedOut)} opted out)\n`,
  );
}
