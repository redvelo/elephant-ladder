import type { NavigatorReadingSummary } from "epub-navigator/react";
import { isRecord } from "../host/guards";
import { reflectionPrompts, type Journal, type JournalMutation, type Reflection } from "./model";
import { nativeRequest } from "../host/requests";

function isSummary(value: unknown): value is NavigatorReadingSummary {
  if (!isRecord(value) || (value.chapter !== null && typeof value.chapter !== "string"))
    return false;
  const pages = value.pages;
  return (
    pages === null ||
    (isRecord(pages) &&
      Number.isSafeInteger(pages.current) &&
      Number.isSafeInteger(pages.total) &&
      typeof pages.current === "number" &&
      typeof pages.total === "number" &&
      pages.current > 0 &&
      pages.current <= pages.total &&
      (pages.accuracy === "exact" || pages.accuracy === "calibrated"))
  );
}

function isReflection(value: unknown): value is Reflection {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.text === "string" &&
    typeof value.prompt === "number" &&
    Number.isInteger(value.prompt) &&
    value.prompt >= 0 &&
    value.prompt < reflectionPrompts.length &&
    (value.context === null || isSummary(value.context)) &&
    typeof value.createdAt === "string" &&
    typeof value.updatedAt === "string"
  );
}

export async function requestJournal(
  publicationId: string,
  token: string,
  documentId: string,
  mutation?: JournalMutation,
  signal?: AbortSignal,
): Promise<Journal> {
  const response = await nativeRequest(
    `/native/publications/${encodeURIComponent(publicationId)}/journal`,
    { sessionToken: token, libraryDocumentId: documentId, trusted: true },
    {
      method: mutation === undefined ? "GET" : "POST",
      ...(mutation === undefined ? {} : { json: mutation, keepalive: true }),
      signal,
    },
  );
  if (!response.ok)
    throw new Error(
      `Could not ${mutation === undefined ? "load" : "save"} reflections (HTTP ${response.status}).`,
    );
  const value: unknown = await response.json();
  if (
    !isRecord(value) ||
    (value.summary !== null && !isSummary(value.summary)) ||
    (value.draft !== null && !isReflection(value.draft)) ||
    !Array.isArray(value.entries) ||
    !value.entries.every(isReflection)
  )
    throw new Error("Invalid reading journal response.");
  return value as unknown as Journal;
}
