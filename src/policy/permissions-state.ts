import { atom } from "jotai";
import {
  readPublicationPolicy,
  savePublicationPolicyWithRetry,
  type PublicationPolicyOverrides,
  type PublicationPolicyReview,
} from "./publication-policy";
import { bootstrapAtom } from "../app/session";

export type PermissionsReview = { notice: "saved" | "unconfirmed" | null } & (
  | { status: "loading" }
  | { status: "unavailable"; error: string }
  | {
      status: "ready" | "saving";
      stored: PublicationPolicyReview;
      changes: Partial<PublicationPolicyOverrides>;
    }
);

const PERMISSIONS_TIMEOUT_MILLISECONDS = 10_000;
const INITIAL_REVIEW: PermissionsReview = { status: "loading", notice: null };

export const permissionsReviewAtom = atom<PermissionsReview>(INITIAL_REVIEW);
const permissionsOperationAtom = atom<AbortController | null>(null);

function policyUrl(publicationId: string): string {
  return `/native/publications/${encodeURIComponent(publicationId)}/policy`;
}

/** Reads the saved permissions, returning the teardown that abandons the read. */
export const loadPermissionsAtom = atom(null, (get, set, publicationId: string): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap?.mode !== "library") return () => controller.abort();
  get(permissionsOperationAtom)?.abort();
  set(permissionsOperationAtom, controller);
  const current = (): boolean => get(permissionsOperationAtom) === controller;
  set(permissionsReviewAtom, { status: "loading", notice: get(permissionsReviewAtom).notice });
  const timeout = setTimeout(() => controller.abort(), PERMISSIONS_TIMEOUT_MILLISECONDS);
  void readPublicationPolicy({
    url: policyUrl(publicationId),
    sessionToken: bootstrap.sessionToken,
    signal: controller.signal,
  })
    .then((stored) => {
      if (!current()) return;
      const { notice } = get(permissionsReviewAtom);
      set(permissionsReviewAtom, { status: "ready", stored, changes: {}, notice });
    })
    .catch((cause: unknown) => {
      if (!current()) return;
      set(permissionsReviewAtom, {
        status: "unavailable",
        notice: get(permissionsReviewAtom).notice,
        error: controller.signal.aborted
          ? "Loading permissions timed out."
          : cause instanceof Error
            ? cause.message
            : "Permissions could not be loaded.",
      });
    })
    .finally(() => {
      clearTimeout(timeout);
      if (current()) set(permissionsOperationAtom, null);
    });
  return () => {
    clearTimeout(timeout);
    if (current()) {
      controller.abort();
      set(permissionsOperationAtom, null);
    }
  };
});

export const closePermissionsAtom = atom(null, (get, set): void => {
  get(permissionsOperationAtom)?.abort();
  set(permissionsOperationAtom, null);
  set(permissionsReviewAtom, INITIAL_REVIEW);
});

export const editPermissionsAtom = atom(
  null,
  (get, set, update: Partial<PublicationPolicyOverrides>): void => {
    const current = get(permissionsReviewAtom);
    if (current.status !== "ready") return;
    const changes = { ...current.changes, ...update };
    for (const key of ["activeContent", "remoteResources"] as const) {
      if (key in update && update[key] === current.stored.policy.publication[key]) {
        delete changes[key];
      }
    }
    set(permissionsReviewAtom, { ...current, changes, notice: null });
  },
);

export const resetPermissionsAtom = atom(null, (get, set): void => {
  const current = get(permissionsReviewAtom);
  if (current.status !== "ready") return;
  set(permissionsReviewAtom, {
    ...current,
    changes: { activeContent: null, remoteResources: null },
    notice: null,
  });
});

/** Resolves true when the save could not be confirmed and saved permissions must be reread. */
export const savePermissionsAtom = atom(
  null,
  async (get, set, publicationId: string): Promise<boolean> => {
    const bootstrap = get(bootstrapAtom);
    const view = get(permissionsReviewAtom);
    if (
      bootstrap?.mode !== "library" ||
      view.status !== "ready" ||
      get(permissionsOperationAtom) !== null ||
      Object.keys(view.changes).length === 0
    ) {
      return false;
    }
    const controller = new AbortController();
    set(permissionsOperationAtom, controller);
    const current = (): boolean => get(permissionsOperationAtom) === controller;
    set(permissionsReviewAtom, { ...view, status: "saving", notice: null });
    const timeout = setTimeout(() => controller.abort(), PERMISSIONS_TIMEOUT_MILLISECONDS);
    try {
      const stored = await savePublicationPolicyWithRetry({
        url: policyUrl(publicationId),
        sessionToken: bootstrap.sessionToken,
        libraryDocumentId: bootstrap.libraryDocumentId,
        initial: view.stored,
        changes: view.changes,
        signal: controller.signal,
      });
      if (!current()) return false;
      set(permissionsReviewAtom, {
        status: "ready",
        stored: { ...stored, relevance: view.stored.relevance },
        changes: {},
        notice: "saved",
      });
      return false;
    } catch {
      if (!current()) return false;
      // A lost response does not roll back a write. Read before enabling further edits.
      set(permissionsReviewAtom, { status: "loading", notice: "unconfirmed" });
      return true;
    } finally {
      clearTimeout(timeout);
      if (current()) set(permissionsOperationAtom, null);
    }
  },
);
