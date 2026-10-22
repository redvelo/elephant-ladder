import { atom } from "jotai";
import { isRecord } from "../host/guards";
import { describeError, nativeRequest } from "../host/requests";
import { bootstrapAtom } from "../app/session";

export interface StorageUsage {
  readonly booksBytes: number;
  readonly dictionariesBytes: number;
}

export type StorageUsageState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; usage: StorageUsage }
  | { status: "failed"; message: string };

export const storageUsageAtom = atom<StorageUsageState>({ status: "idle" });

/** What storage holds, as the archive and the dictionaries recorded it. A snapshot, not a
    subscription: it is read when the settings page opens. */
export const loadStorageUsageAtom = atom(null, async (get, set): Promise<void> => {
  const bootstrap = get(bootstrapAtom);
  if (bootstrap?.mode !== "library" || get(storageUsageAtom).status === "loading") return;
  set(storageUsageAtom, { status: "loading" });
  try {
    const response = await nativeRequest("/native/storage/usage", {
      sessionToken: bootstrap.sessionToken,
      trusted: true,
    });
    if (!response.ok) throw new Error(`Storage usage failed with HTTP ${response.status}.`);
    const usage: unknown = await response.json();
    if (!isStorageUsage(usage)) throw new Error("Storage usage returned invalid data.");
    set(storageUsageAtom, { status: "ready", usage });
  } catch (cause) {
    set(storageUsageAtom, { status: "failed", message: describeError(cause) });
  }
});

function isStorageUsage(value: unknown): value is StorageUsage {
  return (
    isRecord(value) &&
    ["booksBytes", "dictionariesBytes"].every(
      (key) => Number.isSafeInteger(value[key]) && (value[key] as number) >= 0,
    )
  );
}
