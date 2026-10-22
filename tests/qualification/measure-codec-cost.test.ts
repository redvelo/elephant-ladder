import { expect, test } from "vitest";

import {
  logEvents,
  parseArguments,
  runSamples,
  summarize,
} from "../../scripts/measure-codec-cost.mjs";

function first<T>(values: T[]): T {
  const [value] = values;
  if (value === undefined) throw new Error("expected at least one value");
  return value;
}

test("arguments select the platform, the repeat count and the publications", () => {
  expect(parseArguments([])).toStrictEqual({
    android: false,
    runs: 3,
    tests: ["pub-cmt-avif", "pub-cmt-jxl", "pub-cmt-opus", "pub-cmt-mp4"],
  });
  expect(parseArguments(["--android", "--runs", "5", "pub-cmt-jxl"])).toStrictEqual({
    android: true,
    runs: 5,
    tests: ["pub-cmt-jxl"],
  });
  expect(() => parseArguments(["--runs", "0"])).toThrow(/between 1 and 20/);
  expect(() => parseArguments(["--runs", "2.5"])).toThrow(/between 1 and 20/);
  expect(() => parseArguments(["--unknown"])).toThrow(/Unknown argument/);
});

test("only the host's own log lines are read, and malformed ones are skipped", () => {
  const text = [
    'I/elephant( 1234): {"event":"image_restated","elapsedMs":12}',
    "not json at all",
    "{ broken",
    '{"event":"something_else"}',
  ].join("\n");
  expect(logEvents(text)).toStrictEqual([
    { event: "image_restated", elapsedMs: 12 },
    { event: "something_else" },
  ]);
});

test("a run yields one sample per conversion, keyed by what was converted", () => {
  const samples = runSamples([
    {
      event: "image_restated",
      format: "image/jxl",
      path: "EPUB/img/001.jxl",
      elapsedMs: 30.2,
      sourceBytes: 83357,
      pngBytes: 148000,
      width: 600,
      height: 400,
    },
    { event: "publication_resource", outcome: "ready" },
    {
      event: "publication_import_derivation",
      sidecarPreparationMs: 16,
      sidecarTranscodedCount: 1,
    },
  ]);
  expect(samples.images).toHaveLength(1);
  const restated = first(samples.images);
  expect(restated.subject).toBe("image/jxl");
  expect(restated.megapixels).toBeCloseTo(0.24, 5);
  expect(samples.audio).toStrictEqual([{ subject: "audio", elapsedMs: 16, converted: 1 }]);
});

test("a derivation that converted nothing is not a sample", () => {
  const samples = runSamples([
    {
      event: "publication_import_derivation",
      sidecarPreparationMs: 4,
      sidecarTranscodedCount: 0,
    },
  ]);
  expect(samples.audio).toStrictEqual([]);
});

test("repeats are summarized per subject with a rate per megapixel", () => {
  const image = (elapsedMs: number) => ({
    event: "image_restated",
    format: "image/jxl",
    path: "EPUB/img/001.jxl",
    elapsedMs,
    sourceBytes: 1000,
    pngBytes: 2000,
    width: 1000,
    height: 1000,
  });
  const rows = summarize([
    runSamples([image(10)]),
    runSamples([image(30)]),
    runSamples([image(20)]),
  ]);
  expect(rows).toHaveLength(1);
  const summary = first(rows);
  expect(summary.subject).toBe("image/jxl");
  expect(summary.samples).toBe(3);
  expect(summary.min).toBe(10);
  expect(summary.median).toBe(20);
  expect(summary.max).toBe(30);
  // One megapixel, so the rate is the median itself.
  expect(summary.perMegapixel).toBe(20);
});

test("audio has no pixels, so it reports no rate", () => {
  const rows = summarize([
    runSamples([
      {
        event: "publication_import_derivation",
        sidecarPreparationMs: 16,
        sidecarTranscodedCount: 2,
      },
    ]),
  ]);
  const summary = first(rows);
  expect(summary.subject).toBe("audio");
  expect(summary.perMegapixel).toBeUndefined();
});
