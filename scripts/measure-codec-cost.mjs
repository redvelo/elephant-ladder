// What the codec conversions cost on the machine that will run them.
//
// AVIF, JPEG XL and Opus are EPUB core media types that no single engine decodes, so
// the host converts them. On a reading device that work is paid by the reader, and the
// only honest way to size it is to run the real conversion in the real application on
// the real hardware. This drives the committed corpus publications through the fixture
// runner and reads the timings the host logged while serving them.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
// Each publication carries one resource of one format, so a run samples each once.
const DEFAULT_TESTS = ["pub-cmt-avif", "pub-cmt-jxl", "pub-cmt-opus", "pub-cmt-mp4"];
const DEFAULT_RUNS = 3;

export function parseArguments(argv) {
  const android = argv.includes("--android");
  let runs = DEFAULT_RUNS;
  const tests = [];
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--android") continue;
    if (argument === "--runs") {
      const value = Number(argv[index + 1]);
      if (!Number.isInteger(value) || value < 1 || value > 20) {
        throw new Error("--runs takes a whole number of runs between 1 and 20");
      }
      runs = value;
      index += 1;
      continue;
    }
    if (argument.startsWith("-")) throw new Error(`Unknown argument: ${argument}`);
    tests.push(argument);
  }
  return { android, runs, tests: tests.length > 0 ? tests : DEFAULT_TESTS };
}

/** Every JSON object the host logged, in order, ignoring the lines that are not ours.
 *
 * A line may carry a prefix, because on Android the host's output reaches the device
 * log with a level, a tag and a process id in front of it.
 */
export function logEvents(text) {
  const events = [];
  for (const line of text.split("\n")) {
    const start = line.indexOf("{");
    if (start === -1) continue;
    try {
      events.push(JSON.parse(line.slice(start)));
    } catch {
      continue;
    }
  }
  return events;
}

/** The conversion costs one run recorded, keyed by what was converted. */
export function runSamples(events) {
  const images = [];
  const audio = [];
  for (const event of events) {
    if (event.event === "image_restated") {
      images.push({
        subject: String(event.format),
        path: String(event.path),
        elapsedMs: Number(event.elapsedMs),
        sourceBytes: Number(event.sourceBytes),
        outputBytes: Number(event.pngBytes),
        megapixels: (Number(event.width) * Number(event.height)) / 1_000_000,
      });
    }
    // Narration and content audio are converted once, while the publication is derived.
    const derivation = event.event === "publication_import_derivation" ? event : undefined;
    if (derivation !== undefined && Number(derivation.sidecarTranscodedCount) > 0) {
      audio.push({
        subject: "audio",
        elapsedMs: Number(derivation.sidecarPreparationMs),
        converted: Number(derivation.sidecarTranscodedCount),
      });
    }
  }
  return { images, audio };
}

function statistics(values) {
  const sorted = [...values].sort((first, second) => first - second);
  const middle = Math.floor(sorted.length / 2);
  return {
    samples: sorted.length,
    min: sorted[0],
    median: sorted.length % 2 === 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle],
    max: sorted[sorted.length - 1],
  };
}

export function summarize(runs) {
  const grouped = new Map();
  for (const run of runs) {
    for (const sample of [...run.images, ...run.audio]) {
      const existing = grouped.get(sample.subject) ?? [];
      existing.push(sample);
      grouped.set(sample.subject, existing);
    }
  }
  const rows = [];
  for (const [subject, samples] of grouped) {
    const timings = statistics(samples.map((sample) => sample.elapsedMs));
    const megapixels = samples[0].megapixels;
    rows.push({
      subject,
      ...timings,
      megapixels,
      perMegapixel:
        megapixels === undefined || megapixels === 0 ? undefined : timings.median / megapixels,
      sourceBytes: samples[0].sourceBytes,
      outputBytes: samples[0].outputBytes,
    });
  }
  rows.sort((first, second) => first.subject.localeCompare(second.subject));
  return rows;
}

function newestRunDirectory(platform) {
  const base = resolve(root, "test-results/w3c-corpus", platform);
  if (!existsSync(base)) throw new Error(`No corpus runs recorded under ${base}`);
  const directories = readdirSync(base)
    .map((name) => resolve(base, name))
    .filter((path) => statSync(path).isDirectory())
    .sort((first, second) => statSync(second).mtimeMs - statSync(first).mtimeMs);
  const newest = directories[0];
  if (newest === undefined) throw new Error(`No corpus runs recorded under ${base}`);
  return newest;
}

function collectRun(platform, tests) {
  const script =
    platform === "android"
      ? "scripts/collect-w3c-corpus-android.mjs"
      : "scripts/collect-w3c-corpus-linux.mjs";
  const result = spawnSync("node", [resolve(root, script), ...tests], {
    cwd: root,
    stdio: ["ignore", "inherit", "inherit"],
    env: process.env,
  });
  if (result.status !== 0) throw new Error(`${script} failed`);
  const directory = newestRunDirectory(platform);
  const events = [];
  for (const test of tests) {
    const log = resolve(directory, test, "runner.log");
    if (!existsSync(log)) continue;
    events.push(...logEvents(readFileSync(log, "utf8")));
  }
  return runSamples(events);
}

function print(platform, rows) {
  process.stdout.write(`\n## Conversion cost on ${platform}\n\n`);
  if (rows.length === 0) {
    process.stdout.write("Nothing was converted; this engine decodes every tested type.\n");
    return;
  }
  const header = ["subject", "n", "min ms", "median ms", "max ms", "ms/MP", "in KiB", "out KiB"];
  const body = rows.map((row) => [
    row.subject,
    String(row.samples),
    row.min.toFixed(1),
    row.median.toFixed(1),
    row.max.toFixed(1),
    row.perMegapixel === undefined ? "-" : row.perMegapixel.toFixed(0),
    row.sourceBytes === undefined ? "-" : (row.sourceBytes / 1024).toFixed(0),
    row.outputBytes === undefined ? "-" : (row.outputBytes / 1024).toFixed(0),
  ]);
  const widths = header.map((name, column) =>
    Math.max(name.length, ...body.map((cells) => cells[column].length)),
  );
  const line = (cells) => cells.map((cell, index) => cell.padEnd(widths[index])).join("  ");
  process.stdout.write(`${line(header)}\n`);
  for (const cells of body) process.stdout.write(`${line(cells)}\n`);
}

async function main() {
  const { android, runs, tests } = parseArguments(process.argv.slice(2));
  const platform = android ? "android" : "linux";
  const collected = [];
  for (let run = 0; run < runs; run += 1) {
    process.stdout.write(`\nRun ${run + 1} of ${runs} on ${platform}\n`);
    collected.push(collectRun(platform, tests));
  }
  print(platform, summarize(collected));
  process.stdout.write(
    "\nEach run launches the application fresh, so every sample is a cold conversion.\n",
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((cause) => {
    process.stderr.write(`${cause instanceof Error ? cause.message : String(cause)}\n`);
    process.exitCode = 1;
  });
}
