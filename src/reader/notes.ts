import type { ReadNote } from "epub-navigator/react";
import { nativeRequest } from "../host/requests";

/**
 * Reads one note out of the publication the host is serving.
 *
 * The note is the publication's own markup, so it crosses as data and the reader sanitizes it
 * before rendering, the same way a publication description already does.
 */
export function createNoteReader({
  sessionToken,
  readerSessionId,
  request = fetch,
}: {
  readonly sessionToken: string;
  readonly readerSessionId: string;
  readonly request?: typeof fetch;
}): ReadNote {
  return async ({ resource, fragment }, signal) => {
    const query = new URLSearchParams({ resource, fragment });
    const response = await nativeRequest(
      `/native/reader-sessions/${readerSessionId}/note?${query.toString()}`,
      { sessionToken, readerSessionId },
      { signal, fetcher: request },
    );
    if (!response.ok) return null;
    const value: unknown = await response.json();
    if (value === null || typeof value !== "object") return null;
    const note = value as Record<string, unknown>;
    if (typeof note.markup !== "string") return null;
    const direction =
      note.direction === "ltr" || note.direction === "rtl" || note.direction === "auto"
        ? note.direction
        : null;
    return {
      markup: note.markup,
      direction,
      language: typeof note.language === "string" ? note.language : null,
    };
  };
}
