import type { NavigatorReaderPreferences } from "epub-navigator/react";
import { isRecord } from "../../host/guards";
import { isStoredPreferences } from "../../host/bootstrap/preferences";
import { type PreferenceRevisions } from "../../host/bootstrap/types";
import { nativeRequest } from "../../host/requests";

export interface PreferencePersistenceOwner {
  sessionId: string;
  token: string;
  revisions: PreferenceRevisions;
  pending: NavigatorReaderPreferences | null;
  writePromise: Promise<void> | null;
  stopping: boolean;
  terminalError: Error | null;
}

export async function flushPreferences(
  owner: PreferencePersistenceOwner,
  applyAuthoritative: (preferences: NavigatorReaderPreferences) => void,
  reportError: (message: string | null) => void,
): Promise<void> {
  while (owner.pending !== null || owner.writePromise !== null) {
    if (owner.writePromise === null) {
      owner.writePromise = drainPreferenceWrites(owner, applyAuthoritative, reportError).finally(
        () => {
          owner.writePromise = null;
        },
      );
    }
    await owner.writePromise;
  }
}

async function drainPreferenceWrites(
  owner: PreferencePersistenceOwner,
  applyAuthoritative: (preferences: NavigatorReaderPreferences) => void,
  reportError: (message: string | null) => void,
): Promise<void> {
  try {
    while (owner.pending !== null && owner.terminalError === null) {
      const preferences = owner.pending;
      owner.pending = null;
      const response = await nativeRequest(
        "/native/preferences",
        { sessionToken: owner.token },
        {
          method: "PUT",
          keepalive: true,
          json: {
            readerSessionId: owner.sessionId,
            baseRevisions: owner.revisions,
            preferences,
          },
        },
      );
      // The host rejects an invalid save with a text body, so parsing before checking the
      // status reported "Unexpected token" instead of the reason.
      const body = await response.text();
      let value: unknown;
      try {
        value = JSON.parse(body) as unknown;
      } catch {
        throw new Error(
          `Preference save failed with HTTP ${response.status}${body.length > 0 ? `: ${body}` : ""}`,
        );
      }
      if (response.ok && isRecord(value) && value.status === "saved") {
        const saved = { preferences: value.preferences, revisions: value.revisions };
        if (!isStoredPreferences(saved)) throw new Error("Preference save returned invalid data.");
        owner.revisions = saved.revisions;
        reportError(null);
        continue;
      }
      if (
        response.status === 409 &&
        isRecord(value) &&
        (value.reason === "revision-mismatch" || value.reason === "revision-exhausted")
      ) {
        const authoritative = value.authoritative;
        if (!isStoredPreferences(authoritative)) {
          throw new Error("Preference conflict returned invalid data.");
        }
        owner.revisions = authoritative.revisions;
        owner.pending = null;
        applyAuthoritative(authoritative.preferences);
        reportError(null);
        if (value.reason === "revision-exhausted") {
          throw new Error("Preference revision is exhausted.");
        }
        continue;
      }
      throw new Error(`Preference save failed with HTTP ${response.status}`);
    }
  } catch (cause) {
    owner.pending = null;
    owner.terminalError = cause instanceof Error ? cause : new Error(String(cause));
    reportError(owner.terminalError.message);
  }
}
