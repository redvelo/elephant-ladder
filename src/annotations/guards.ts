import type {
  NavigatorAnnotationProjection,
  NavigatorAnnotationRecord,
} from "epub-navigator/react";

export interface AnnotationMutationAcceptedResponse {
  status: "accepted";
  requestId: string;
  recordId: string;
  projection: NavigatorAnnotationProjection;
}

export interface AnnotationMutationConflictResponse {
  status: "conflict";
  reason: string;
  requestId: string;
  authoritative: { projection: NavigatorAnnotationProjection } | null;
}

export function isAnnotationMutationAccepted(
  value: unknown,
  requestId: string,
): value is AnnotationMutationAcceptedResponse {
  return (
    isExactRecord(value, ["status", "requestId", "recordId", "projection"]) &&
    value.status === "accepted" &&
    value.requestId === requestId &&
    isAnnotationId(value.recordId) &&
    isAnnotationProjection(value.projection)
  );
}

export function isAnnotationMutationConflict(
  value: unknown,
  requestId: string,
): value is AnnotationMutationConflictResponse {
  return (
    isExactRecord(value, ["status", "reason", "authoritative", "requestId"]) &&
    value.status === "conflict" &&
    typeof value.reason === "string" &&
    value.requestId === requestId &&
    (value.authoritative === null ||
      (isExactRecord(value.authoritative, ["projection"]) &&
        isAnnotationProjection(value.authoritative.projection)))
  );
}

export function isAnnotationProjection(value: unknown): value is NavigatorAnnotationProjection {
  if (
    !isExactRecord(value, ["revision", "records"]) ||
    !isU32(value.revision) ||
    !Array.isArray(value.records) ||
    (value.revision === 0 && value.records.length !== 0) ||
    value.revision < value.records.length
  ) {
    return false;
  }
  const ids = new Set<string>();
  for (const candidate of value.records) {
    if (!isAnnotationRecord(candidate) || ids.has(candidate.id)) return false;
    ids.add(candidate.id);
  }
  return true;
}

function isAnnotationRecord(value: unknown): value is NavigatorAnnotationRecord {
  if (!isRecord(value)) return false;
  const commonValid =
    isAnnotationId(value.id) &&
    isAnnotationTarget(value.targetJson) &&
    isU32(value.occurrence) &&
    isBoundedNullableText(value.sectionLabel, 256, 1_024);
  if (!commonValid) return false;
  if (value.type === "highlight") {
    return (
      isExactRecord(value, ["type", "id", "targetJson", "occurrence", "excerpt", "sectionLabel"]) &&
      isBoundedNullableText(value.excerpt, 512, 2_048)
    );
  }
  if (value.type === "note") {
    return (
      isExactRecord(value, [
        "type",
        "id",
        "targetJson",
        "occurrence",
        "excerpt",
        "sectionLabel",
        "text",
      ]) &&
      isBoundedNullableText(value.excerpt, 512, 2_048) &&
      typeof value.text === "string" &&
      /\S/u.test(value.text) &&
      Array.from(value.text).length <= 4_096 &&
      new TextEncoder().encode(value.text).length <= 16_384
    );
  }
  return (
    value.type === "bookmark" &&
    isExactRecord(value, ["type", "id", "targetJson", "occurrence", "sectionLabel", "label"]) &&
    isBoundedNullableText(value.label, 256, 1_024)
  );
}

function isAnnotationTarget(value: unknown): value is string {
  if (typeof value !== "string" || value.length === 0) return false;
  if (new TextEncoder().encode(value).length > 1_048_576) return false;
  try {
    const parsed: unknown = JSON.parse(value);
    return isRecord(parsed) && !Array.isArray(parsed);
  } catch {
    return false;
  }
}

function isBoundedNullableText(
  value: unknown,
  codePoints: number,
  bytes: number,
): value is string | null {
  return (
    value === null ||
    (typeof value === "string" &&
      Array.from(value).length <= codePoints &&
      new TextEncoder().encode(value).length <= bytes)
  );
}

function isAnnotationId(value: unknown): value is string {
  return typeof value === "string" && /^[A-Za-z0-9._~-]{1,128}$/.test(value);
}

function isU32(value: unknown): value is number {
  return Number.isInteger(value) && (value as number) >= 0 && (value as number) <= 0xffff_ffff;
}

function isExactRecord(value: unknown, keys: readonly string[]): value is Record<string, unknown> {
  return (
    isRecord(value) &&
    !Array.isArray(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.prototype.hasOwnProperty.call(value, key))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
