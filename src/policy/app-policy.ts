import { atom } from "jotai";
import { isRecord } from "../host/guards";
import { describeError, nativeRequest } from "../host/requests";
import { bootstrapAtom } from "../app/session";

export interface AppPolicy {
  readonly activeContent: "automatic" | "ask" | "disabled";
  readonly remoteResources: "allowed" | "ask" | "blocked";
}

export type AppPolicyState =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly policy: AppPolicy; readonly revision: number }
  | { readonly status: "unavailable"; readonly message: string };

export const appPolicyAtom = atom<AppPolicyState>({ status: "loading" });
export const appPolicyErrorAtom = atom<string | null>(null);

const activeContentDefaults = ["automatic", "ask", "disabled"] as const;
const remoteResourceDefaults = ["allowed", "ask", "blocked"] as const;

function parsePolicy(value: unknown): { policy: AppPolicy; revision: number } {
  if (
    !isRecord(value) ||
    !isRecord(value.policy) ||
    !isRecord(value.policy.global) ||
    !isRecord(value.revisions) ||
    typeof value.revisions.global !== "number" ||
    !includes(activeContentDefaults, value.policy.global.activeContent) ||
    !includes(remoteResourceDefaults, value.policy.global.remoteResources)
  ) {
    throw new Error("Invalid permission defaults.");
  }
  return {
    policy: {
      activeContent: value.policy.global.activeContent,
      remoteResources: value.policy.global.remoteResources,
    },
    revision: value.revisions.global,
  };
}

function includes<const T extends string>(values: readonly T[], value: unknown): value is T {
  return values.some((candidate) => candidate === value);
}

export const loadAppPolicyAtom = atom(null, (get, set): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap === null) return () => controller.abort();
  void nativeRequest(
    "/native/publication-policy",
    { sessionToken: bootstrap.sessionToken, trusted: true },
    { signal: controller.signal },
  )
    .then(async (response) => {
      if (!response.ok) throw new Error("Could not load permission defaults.");
      const parsed = parsePolicy(await response.json());
      if (controller.signal.aborted) return;
      set(appPolicyAtom, { status: "ready", ...parsed });
      set(appPolicyErrorAtom, null);
    })
    .catch((cause: unknown) => {
      if (!controller.signal.aborted)
        set(appPolicyAtom, { status: "unavailable", message: describeError(cause) });
    });
  return () => controller.abort();
});

/**
 * Saves the application's own defaults. The host refuses the write while a publication is open,
 * so a conflict is reported rather than retried against a moving target.
 */
export const saveAppPolicyAtom = atom(
  null,
  async (get, set, change: Partial<AppPolicy>): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    const current = get(appPolicyAtom);
    if (bootstrap?.mode !== "library" || current.status !== "ready") return;
    const policy: AppPolicy = { ...current.policy, ...change };
    set(appPolicyAtom, { status: "ready", policy, revision: current.revision });
    set(appPolicyErrorAtom, null);
    try {
      const response = await nativeRequest(
        "/native/publication-policy",
        { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
        { method: "PUT", json: { baseRevision: current.revision, policy } },
      );
      const value: unknown = await response.json();
      if (response.ok && isRecord(value) && value.status === "saved") {
        set(appPolicyAtom, { status: "ready", ...parsePolicy(value) });
        return;
      }
      if (response.status === 409 && isRecord(value) && value.status === "conflict") {
        // A reason without an authoritative projection means the write was refused outright,
        // such as a publication being open or a stale library document.
        if (!isRecord(value.authoritative)) {
          throw new Error(
            value.reason === "publication is open"
              ? "Close the open book before changing permission defaults."
              : "Could not save permission defaults.",
          );
        }
        set(appPolicyAtom, { status: "ready", ...parsePolicy(value.authoritative) });
        set(
          appPolicyErrorAtom,
          "Permission defaults were changed elsewhere. The current values are shown.",
        );
        return;
      }
      throw new Error("Could not save permission defaults.");
    } catch (cause) {
      set(appPolicyAtom, { status: "ready", policy: current.policy, revision: current.revision });
      set(appPolicyErrorAtom, describeError(cause));
    }
  },
);
