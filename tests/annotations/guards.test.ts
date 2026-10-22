import { expect, test } from "vitest";
import {
  isAnnotationMutationAccepted,
  isAnnotationMutationConflict,
  isAnnotationProjection,
} from "../../src/annotations/guards";

const highlight = {
  type: "highlight",
  id: "record-1",
  targetJson: '{"selector":true}',
  occurrence: 0,
  excerpt: "selected",
  sectionLabel: null,
};

test("annotation projection guard accepts the complete public record shapes", () => {
  expect(
    isAnnotationProjection({
      revision: 3,
      records: [
        highlight,
        { ...highlight, type: "note", id: "record-2", text: "note" },
        {
          type: "bookmark",
          id: "record-3",
          targetJson: "{}",
          occurrence: 0,
          sectionLabel: "Chapter",
          label: null,
        },
      ],
    }),
  ).toBe(true);
});

test("annotation projection guard rejects host fields, duplicates, and malformed targets", () => {
  expect(
    isAnnotationProjection({
      revision: 1,
      records: [{ ...highlight, resourceHint: "EPUB/chapter.xhtml" }],
    }),
  ).toBe(false);
  expect(isAnnotationProjection({ revision: 2, records: [highlight, highlight] })).toBe(false);
  expect(
    isAnnotationProjection({ revision: 1, records: [{ ...highlight, targetJson: "[]" }] }),
  ).toBe(false);
  expect(isAnnotationProjection({ revision: 0, records: [highlight] })).toBe(false);
});

test("annotation mutation response guards bind request identity and projection", () => {
  const projection = { revision: 1, records: [highlight] };
  expect(
    isAnnotationMutationAccepted(
      { status: "accepted", requestId: "request-1", recordId: "record-1", projection },
      "request-1",
    ),
  ).toBe(true);
  expect(
    isAnnotationMutationAccepted(
      { status: "accepted", requestId: "other", recordId: "record-1", projection },
      "request-1",
    ),
  ).toBe(false);
  expect(
    isAnnotationMutationConflict(
      {
        status: "conflict",
        reason: "revision-mismatch",
        requestId: "request-1",
        authoritative: { projection },
      },
      "request-1",
    ),
  ).toBe(true);
});
