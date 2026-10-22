import { expect, test } from "vitest";
import {
  copyReadingPosition,
  isReadingPosition,
  readingPositionCaptureDiagnostic,
  readingPositionKey,
  replaceReadingPositionTimer,
} from "../../src/reader/reading-position";

const ordinaryPosition = {
  targetJson: '{"source":"EPUB/cover.xhtml"}',
  occurrenceHint: { occurrence: 0, resource: "EPUB/cover.xhtml" },
};

test("capture copies roll offsets into persistence identities without changing ordinary positions", () => {
  const captured = { ...ordinaryPosition, status: "generated", rollProgression: 0.25 };
  const position = copyReadingPosition(captured);
  expect(position).toStrictEqual({ ...ordinaryPosition, rollProgression: 0.25 });
  expect(readingPositionKey(position)).not.toBe(
    readingPositionKey(copyReadingPosition({ ...captured, rollProgression: 0.75 })),
  );
  expect(copyReadingPosition(ordinaryPosition)).toStrictEqual(ordinaryPosition);
  expect(readingPositionKey(copyReadingPosition(ordinaryPosition))).toBe(
    JSON.stringify(ordinaryPosition),
  );
});

test("bootstrap positions preserve old shapes and the full roll position", () => {
  for (const position of [
    ordinaryPosition,
    { ...ordinaryPosition, occurrenceHint: null },
    ...[0, 0.375, 1].map((rollProgression) => ({ ...ordinaryPosition, rollProgression })),
  ]) {
    const bootstrap = JSON.parse(JSON.stringify({ initialReadingPosition: position }));
    expect(isReadingPosition(bootstrap.initialReadingPosition)).toBe(true);
    expect(copyReadingPosition(bootstrap.initialReadingPosition)).toStrictEqual(position);
  }
});

test.each([-0.001, 1.001, NaN, Infinity, -Infinity, null, undefined, "0.5"])(
  "rejects invalid roll progression %s",
  (rollProgression) => {
    expect(isReadingPosition({ ...ordinaryPosition, rollProgression })).toBe(false);
  },
);

test("reading positions do not accept arbitrary new fields or invalid old fields", () => {
  for (const position of [
    null,
    [],
    {},
    { ...ordinaryPosition, extra: true },
    { ...ordinaryPosition, occurrenceHint: { ...ordinaryPosition.occurrenceHint, extra: true } },
    { ...ordinaryPosition, occurrenceHint: { occurrence: -1, resource: "cover.xhtml" } },
    { ...ordinaryPosition, targetJson: null },
  ]) {
    expect(isReadingPosition(position)).toBe(false);
  }
});

test("a non-point location cancels an obsolete position capture", () => {
  const cleared: number[] = [];
  let scheduled = 0;
  const timer = replaceReadingPositionTimer(
    {
      clearTimeout(value) {
        cleared.push(value);
      },
      setTimeout() {
        scheduled += 1;
        return 12;
      },
    },
    11,
    { revision: 2, location: null, basis: null, visibleOccurrences: null },
    () => undefined,
  );

  expect(timer).toBe(null);
  expect(cleared).toStrictEqual([11]);
  expect(scheduled).toBe(0);
});

test("only a usable point schedules a position capture", () => {
  let delay = 0;
  const timer = replaceReadingPositionTimer(
    {
      clearTimeout() {},
      setTimeout(_callback, value) {
        delay = value;
        return 13;
      },
    },
    null,
    {
      revision: 3,
      location: {
        kind: "point",
        occurrence: 0,
        resource: "EPUB/cover.xhtml",
        evidence: [{ kind: "document-boundary", boundary: "start" }],
        observation: { layoutProgression: null, renderedTextProgression: null },
      },
      basis: { documentGeneration: 1, frameGeneration: 1, layoutGeneration: 1 },
      visibleOccurrences: null,
    },
    () => undefined,
  );

  expect(timer).toBe(13);
  expect(delay).toBe(500);
});

test("capture outcomes map to diagnostics and generated positions have stable identities", () => {
  expect(
    readingPositionCaptureDiagnostic({
      status: "unavailable",
      reason: "current-location-unavailable",
    }),
  ).toEqual({ outcome: "unavailable", reason: "current-location-unavailable" });
  expect(readingPositionCaptureDiagnostic({ status: "stale" })).toEqual({
    outcome: "stale",
    reason: null,
  });
  expect(
    readingPositionCaptureDiagnostic({ status: "unavailable", reason: "no-exact-selector" }),
  ).toEqual({ outcome: "unavailable", reason: "no-exact-selector" });
  expect(readingPositionCaptureDiagnostic({ status: "failed", message: "x" })).toEqual({
    outcome: "failed",
    reason: null,
  });
  expect(
    readingPositionKey({
      targetJson: '{"source":"cover.xhtml"}',
      occurrenceHint: { occurrence: 0, resource: "EPUB/cover.xhtml" },
    }),
  ).toBe(
    readingPositionKey({
      targetJson: '{"source":"cover.xhtml"}',
      occurrenceHint: { occurrence: 0, resource: "EPUB/cover.xhtml" },
    }),
  );
});
