import { atom } from "jotai";
import type { NavigatorReaderPreferences } from "epub-navigator/react";
import { isRecord } from "../host/guards";
import { isStoredPreferences } from "../host/bootstrap/preferences";
import { describeError, nativeRequest } from "../host/requests";
import { applyApplicationColorScheme } from "../app/color-scheme";
import { bootstrapAtom } from "../app/session";

interface AppSettings {
  preferences: NavigatorReaderPreferences;
  revision: number;
  /** What the application keeps about this device, which the reader knows nothing about. */
  device: { sideButtonsTurnPages: boolean };
}

type SettingsChange = {
  preferences?: NavigatorReaderPreferences;
  device?: { sideButtonsTurnPages: boolean };
};

/** Coalesces edits made while a write is in flight, so a held stepper never loses a press. */
interface SettingsWriter {
  token: string;
  libraryDocumentId: string;
  revision: number;
  pending: Omit<AppSettings, "revision"> | null;
  writePromise: Promise<void> | null;
}

export const settingsAtom = atom<AppSettings | null>(null);
export const settingsErrorAtom = atom<string | null>(null);
const settingsWriterAtom = atom<SettingsWriter | null>(null);

function parseSettings(value: unknown): AppSettings {
  if (
    !isRecord(value) ||
    typeof value.revision !== "number" ||
    !isRecord(value.device) ||
    typeof value.device.sideButtonsTurnPages !== "boolean" ||
    !isStoredPreferences({
      preferences: value.preferences,
      revisions: { global: value.revision, publication: 0 },
    })
  ) {
    throw new Error("Invalid settings response.");
  }
  return value as unknown as AppSettings;
}

export const loadSettingsAtom = atom(null, (get, set): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap === null) return () => controller.abort();
  set(settingsErrorAtom, null);
  void nativeRequest(
    "/native/app-settings",
    { sessionToken: bootstrap.sessionToken },
    { signal: controller.signal },
  )
    .then(async (response) => {
      if (!response.ok) throw new Error("Could not load settings.");
      const settings = parseSettings(await response.json());
      if (controller.signal.aborted) return;
      set(settingsAtom, settings);
      if (bootstrap.mode === "library") {
        set(settingsWriterAtom, {
          token: bootstrap.sessionToken,
          libraryDocumentId: bootstrap.libraryDocumentId,
          revision: settings.revision,
          pending: null,
          writePromise: null,
        });
      }
    })
    .catch((cause: unknown) => {
      if (!controller.signal.aborted)
        set(settingsErrorAtom, cause instanceof Error ? cause.message : "Could not load settings.");
    });
  return () => controller.abort();
});

/**
 * Applies the change locally and persists it behind the edit, the way a reading session does.
 * A write never disables the controls, and the authoritative answer is adopted only when no
 * newer edit is waiting for it.
 */
export const saveSettingsAtom = atom(null, (get, set, change: SettingsChange): void => {
  const current = get(settingsAtom);
  const writer = get(settingsWriterAtom);
  if (current === null || writer === null) return;
  const next: AppSettings = { ...current, ...change };
  set(settingsAtom, next);
  applyApplicationColorScheme(next.preferences.colorScheme);
  writer.pending = { preferences: next.preferences, device: next.device };
  void flushSettings(
    writer,
    (settings) => {
      set(settingsAtom, settings);
      applyApplicationColorScheme(settings.preferences.colorScheme);
    },
    (message) => set(settingsErrorAtom, message),
  );
});

async function flushSettings(
  writer: SettingsWriter,
  applyAuthoritative: (settings: AppSettings) => void,
  reportError: (message: string | null) => void,
): Promise<void> {
  while (writer.pending !== null || writer.writePromise !== null) {
    if (writer.writePromise === null) {
      writer.writePromise = drainSettingsWrites(writer, applyAuthoritative, reportError).finally(
        () => {
          writer.writePromise = null;
        },
      );
    }
    await writer.writePromise;
  }
}

async function drainSettingsWrites(
  writer: SettingsWriter,
  applyAuthoritative: (settings: AppSettings) => void,
  reportError: (message: string | null) => void,
): Promise<void> {
  try {
    while (writer.pending !== null) {
      const draft = writer.pending;
      writer.pending = null;
      const response = await nativeRequest(
        "/native/app-settings",
        { sessionToken: writer.token, libraryDocumentId: writer.libraryDocumentId },
        { method: "PUT", json: { revision: writer.revision, ...draft } },
      );
      if (!response.ok && response.status !== 409) throw new Error("Could not save settings.");
      const settings = parseSettings(await response.json());
      writer.revision = settings.revision;
      if (response.status === 409) {
        writer.pending = null;
        applyAuthoritative(settings);
        reportError("Settings were changed on another screen. The current values are shown.");
        return;
      }
      if (writer.pending === null) applyAuthoritative(settings);
      reportError(null);
    }
  } catch (cause) {
    writer.pending = null;
    reportError(describeError(cause));
  }
}

export const settingsResetPendingAtom = atom(false);

/** Returns the reading preferences to the application defaults, keeping the device answers. */
export const resetSettingsAtom = atom(null, async (get, set): Promise<void> => {
  const bootstrap = get(bootstrapAtom);
  const writer = get(settingsWriterAtom);
  if (bootstrap?.mode !== "library" || writer === null || get(settingsResetPendingAtom)) return;
  set(settingsResetPendingAtom, true);
  set(settingsErrorAtom, null);
  try {
    const response = await nativeRequest(
      "/native/app-settings/reset",
      { sessionToken: writer.token, libraryDocumentId: writer.libraryDocumentId },
      { method: "POST", json: { revision: writer.revision } },
    );
    if (!response.ok && response.status !== 409) throw new Error("Could not reset settings.");
    const settings = parseSettings(await response.json());
    writer.revision = settings.revision;
    writer.pending = null;
    set(settingsAtom, settings);
    applyApplicationColorScheme(settings.preferences.colorScheme);
    if (response.status === 409)
      set(
        settingsErrorAtom,
        "Settings were changed on another screen. The current values are shown.",
      );
  } catch (cause) {
    set(settingsErrorAtom, describeError(cause));
  } finally {
    set(settingsResetPendingAtom, false);
  }
});
