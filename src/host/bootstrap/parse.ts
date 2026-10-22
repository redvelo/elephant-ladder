import { isNavigatorMediaOverlayAvailability } from "epub-navigator";
import { isAnnotationProjection } from "../../annotations/guards";
import { isDictionaryCapability } from "../../reader/capabilities/dictionary";
import { isPaginationDescriptor } from "../../reader/capabilities/pagination";
import { isReadingPosition } from "../../reader/reading-position";
import { commitBootstrapRecovery, isNavigationPath } from "../navigation";
import { exactOriginOf, hasExactKeys, isExactOrigin, isRecord } from "../guards";
import { isPreferenceRevisions, isReaderPreferences, isReaderTypefaces } from "./preferences";
import { isMountedPublication, isResolvedPublicationPolicy } from "./publication";
import type { Bootstrap } from "./types";

/** The stored interface color, read before React mounts so the first paint is not relit. */
function readBootstrapValue(): Record<string, unknown> | null {
  try {
    const text = document.getElementById("native-bootstrap")?.textContent;
    if (text === null || text === undefined) return null;
    const value: unknown = JSON.parse(text);
    return isRecord(value) ? value : null;
  } catch {
    return null;
  }
}

export function readBootstrapColorScheme(): "system" | "light" | "dark" {
  const value = readBootstrapValue();
  if (value === null || !isRecord(value.preferences)) return "system";
  const { colorScheme } = value.preferences;
  return colorScheme === "light" || colorScheme === "dark" ? colorScheme : "system";
}

/** A display that ghosts is told apart before React mounts, so the first paint is already paced. */
export function readBootstrapInteractionFeedback(): "standard" | "minimal" {
  const value = readBootstrapValue();
  return isRecord(value?.display) && value.display.technology === "eink" ? "minimal" : "standard";
}

export async function loadBootstrap(): Promise<Bootstrap> {
  const element = document.getElementById("native-bootstrap");
  if (element === null || element.textContent === null) {
    throw new Error("Native bootstrap is missing");
  }
  const value: unknown = JSON.parse(element.textContent);
  if (!isBootstrap(value)) throw new Error("Native bootstrap is invalid");
  commitBootstrapRecovery(element, value.recoveryNavigationPath);
  return value;
}

function isApplicationIdentity(value: unknown): boolean {
  return (
    isRecord(value) &&
    typeof value.name === "string" &&
    value.name.length > 0 &&
    typeof value.version === "string" &&
    value.version.length > 0
  );
}

function isBootstrap(value: unknown): value is Bootstrap {
  if (
    !isRecord(value) ||
    (value.mode !== "library" && value.mode !== "reading") ||
    typeof value.readerSessionId !== "string" ||
    !/^[0-9a-f]{64}$/.test(value.readerSessionId) ||
    typeof value.launchId !== "string" ||
    !/^[0-9a-f]{32}$/.test(value.launchId) ||
    typeof value.readerOrigin !== "string" ||
    typeof value.assetOrigin !== "string" ||
    typeof value.sessionToken !== "string" ||
    !/^[0-9a-f]{64}$/.test(value.sessionToken) ||
    value.tauriIpcEnabled !== false ||
    !isRecord(value.nativeActions) ||
    !isRecord(value.display) ||
    (value.display.technology !== "eink" && value.display.technology !== "unknown") ||
    !isDisplayReading(value.display) ||
    typeof value.nativeActions.frontLightDialog !== "boolean" ||
    typeof value.nativeActions.sideButtons !== "boolean" ||
    (value.diagnosticsLevel !== "summary" && value.diagnosticsLevel !== "trace") ||
    !isNavigationPath(value.recoveryNavigationPath) ||
    !isReaderPreferences(value.preferences) ||
    !isPreferenceRevisions(value.preferenceRevisions) ||
    !isReaderTypefaces(value.readerTypefaces)
  ) {
    return false;
  }
  if (value.readerOrigin !== window.location.origin || value.assetOrigin !== value.readerOrigin) {
    return false;
  }
  if (value.mode === "library") {
    return (
      hasExactKeys(value, libraryBootstrapKeys) &&
      typeof value.libraryDocumentId === "string" &&
      /^[0-9a-f]{64}$/.test(value.libraryDocumentId) &&
      isDictionariesSummary(value.dictionaries) &&
      isApplicationIdentity(value.application)
    );
  }
  return (
    hasExactKeys(value, readingBootstrapKeys) &&
    value.libraryDocumentId === null &&
    typeof value.publicationOrigin === "string" &&
    typeof value.measurementOrigin === "string" &&
    isExactOrigin(value.publicationOrigin) &&
    isExactOrigin(value.measurementOrigin) &&
    typeof value.publicationId === "string" &&
    /^[0-9a-f]{32}$/.test(value.publicationId) &&
    isMountedPublication(value.publication) &&
    isResolvedPublicationPolicy(value.policy) &&
    isNavigatorMediaOverlayAvailability(value.mediaOverlay) &&
    (value.dictionary === null || isDictionaryCapability(value.dictionary)) &&
    value.publication.mount.remoteResources === value.policy.remoteResources &&
    exactOriginOf(value.publication.mount.baseUrl) === value.publicationOrigin &&
    value.publication.mount.baseUrl === `${value.publicationOrigin}/` &&
    isPaginationDescriptor(value.pagination, value.measurementOrigin) &&
    Number.isSafeInteger(value.positionRevision) &&
    (value.positionRevision as number) >= 0 &&
    (value.initialReadingPosition === null || isReadingPosition(value.initialReadingPosition)) &&
    isAnnotationProjection(value.annotations) &&
    Array.isArray(value.annotationIdentifiers) &&
    value.annotationIdentifiers.every((identifier) => typeof identifier === "string")
  );
}

const bootstrapBaseKeys = [
  "mode",
  "libraryDocumentId",
  "readerSessionId",
  "launchId",
  "readerOrigin",
  "assetOrigin",
  "sessionToken",
  "tauriIpcEnabled",
  "nativeActions",
  "display",
  "preferences",
  "preferenceRevisions",
  "readerTypefaces",
  "diagnosticsLevel",
  "recoveryNavigationPath",
] as const;

const libraryBootstrapKeys = new Set<string>([...bootstrapBaseKeys, "dictionaries", "application"]);

const readingBootstrapKeys = new Set<string>([
  ...bootstrapBaseKeys,
  "publicationOrigin",
  "measurementOrigin",
  "publicationId",
  "publication",
  "pagination",
  "initialReadingPosition",
  "positionRevision",
  "annotations",
  "annotationIdentifiers",
  "policy",
  "mediaOverlay",
  "dictionary",
]);

function isDisplayReading(display: Record<string, unknown>): boolean {
  return (
    (display.refreshMode === undefined ||
      (typeof display.refreshMode === "string" && display.refreshMode.length > 0)) &&
    (display.speedModeWhileDragging === undefined ||
      typeof display.speedModeWhileDragging === "boolean")
  );
}

function isDictionariesSummary(value: unknown): boolean {
  return (
    isRecord(value) &&
    Object.keys(value).length === 2 &&
    Number.isSafeInteger(value.installed) &&
    (value.installed as number) >= 0 &&
    typeof value.operationActive === "boolean"
  );
}
