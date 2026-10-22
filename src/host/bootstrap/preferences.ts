import type { NavigatorReaderPreferences } from "epub-navigator/react";
import { isPreferenceValue, navigatorReaderPreferenceValues } from "epub-navigator/runtime";
import { isRecord } from "../guards";
import type { PreferenceRevisions, StoredPreferences } from "./types";

/** Which reader faces the host says it serves; an unknown name is not a face we can offer. */
export function isReaderTypefaces(
  value: unknown,
): value is readonly ("reader-serif" | "reader-sans")[] {
  return (
    Array.isArray(value) &&
    value.length <= 2 &&
    value.every((face) => face === "reader-serif" || face === "reader-sans") &&
    new Set(value).size === value.length
  );
}

/** Every preference whose value Navigator bounds to a list. */
const stringValuedPreferences = Object.keys(
  navigatorReaderPreferenceValues,
) as (keyof typeof navigatorReaderPreferenceValues)[];

export function isReaderPreferences(value: unknown): value is NavigatorReaderPreferences {
  return (
    isRecord(value) &&
    // Permitted values come from Navigator, so the application cannot refuse a value the
    // reader offers, nor accept one it has dropped.
    stringValuedPreferences.every((preference) =>
      isPreferenceValue(preference, value[preference]),
    ) &&
    typeof value.touchZones === "boolean" &&
    (value.clickZones === undefined || typeof value.clickZones === "boolean") &&
    Array.isArray(value.narrationSkippedTypes) &&
    value.narrationSkippedTypes.length <= 32 &&
    value.narrationSkippedTypes.every(
      (type, index, types) =>
        typeof type === "string" &&
        type.length > 0 &&
        type.length <= 64 &&
        !/\s/.test(type) &&
        types.indexOf(type) === index,
    )
  );
}

export function isPreferenceRevisions(value: unknown): value is PreferenceRevisions {
  return (
    isRecord(value) &&
    Number.isSafeInteger(value.global) &&
    (value.global as number) >= 0 &&
    Number.isSafeInteger(value.publication) &&
    (value.publication as number) >= 0
  );
}

export function isStoredPreferences(value: unknown): value is StoredPreferences {
  return (
    isRecord(value) &&
    isReaderPreferences(value.preferences) &&
    isPreferenceRevisions(value.revisions)
  );
}
