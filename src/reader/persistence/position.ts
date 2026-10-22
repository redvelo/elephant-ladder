import type {
  NavigatorReaderHandle,
  NavigatorReadingPosition,
  NavigatorReadingSummary,
} from "epub-navigator/react";
import { isRecord } from "../../host/guards";
import { nativeRequest } from "../../host/requests";
import type {
  ReadingPositionCaptureReason,
  ReadingPositionOutcome,
} from "../../host/diagnostics/records";
import {
  copyReadingPosition,
  readingPositionCaptureDiagnostic,
  readingPositionKey,
} from "../reading-position";

const READING_POSITION_SAVE_TIMEOUT_MILLISECONDS = 5_000;

interface SaveReadingPositionResponse {
  status: "saved";
  readerSessionId: string;
  sequence: number;
  positionRevision: number;
}

interface SaveReadingPositionConflict {
  status: "conflict";
  reason: "revision-mismatch" | "revision-exhausted";
  authoritative: { positionRevision: number };
}

interface PendingPositionSave {
  summary: NavigatorReadingSummary | null;
  sequence: number;
  position: NavigatorReadingPosition;
  key: string;
  revisionRetried: boolean;
}

export interface PositionPersistenceOwner {
  summary: NavigatorReadingSummary | null;
  sessionId: string;
  token: string;
  revision: number;
  sequence: number;
  debounceTimer: number | null;
  capturePromise: Promise<void> | null;
  recaptureRequested: boolean;
  checkpointPromise: Promise<void> | null;
  writePromise: Promise<void> | null;
  pending: PendingPositionSave | null;
  lastSavedKey: string | null;
  lastQueuedKey: string | null;
  stopping: boolean;
}

function isSaveReadingPositionResponse(value: unknown): value is SaveReadingPositionResponse {
  return (
    isRecord(value) &&
    value.status === "saved" &&
    typeof value.readerSessionId === "string" &&
    Number.isSafeInteger(value.sequence) &&
    Number.isSafeInteger(value.positionRevision)
  );
}

function isSaveReadingPositionConflict(value: unknown): value is SaveReadingPositionConflict {
  return (
    isRecord(value) &&
    value.status === "conflict" &&
    (value.reason === "revision-mismatch" || value.reason === "revision-exhausted") &&
    isRecord(value.authoritative) &&
    typeof value.authoritative.positionRevision === "number" &&
    Number.isSafeInteger(value.authoritative.positionRevision) &&
    value.authoritative.positionRevision >= 0 &&
    value.authoritative.positionRevision <= 0xffff_ffff
  );
}

export function createPositionPersistenceOwner(
  sessionId: string,
  token: string,
  revision: number,
  initialPosition: NavigatorReadingPosition | null,
): PositionPersistenceOwner {
  return {
    summary: null,
    sessionId,
    token,
    revision,
    sequence: 0,
    debounceTimer: null,
    capturePromise: null,
    recaptureRequested: false,
    checkpointPromise: null,
    writePromise: null,
    pending: null,
    lastSavedKey: initialPosition === null ? null : readingPositionKey(initialPosition),
    lastQueuedKey: initialPosition === null ? null : readingPositionKey(initialPosition),
    stopping: false,
  };
}

export interface PositionIssues {
  capture(outcome: ReadingPositionOutcome, reason: ReadingPositionCaptureReason | null): void;
  write(): void;
}

export function requestReadingPositionCapture(
  owner: PositionPersistenceOwner,
  reader: { readonly current: NavigatorReaderHandle | null },
  currentOwner: { readonly current: PositionPersistenceOwner | null },
  issues: PositionIssues,
): void {
  if (owner.capturePromise !== null) {
    owner.recaptureRequested = true;
    return;
  }
  owner.capturePromise = captureReadingPosition(owner, reader, currentOwner, issues).finally(() => {
    owner.capturePromise = null;
    if (owner.recaptureRequested && !owner.stopping) {
      owner.recaptureRequested = false;
      requestReadingPositionCapture(owner, reader, currentOwner, issues);
    }
  });
}

async function captureReadingPosition(
  owner: PositionPersistenceOwner,
  reader: { readonly current: NavigatorReaderHandle | null },
  currentOwner: { readonly current: PositionPersistenceOwner | null },
  issues: PositionIssues,
): Promise<void> {
  const handle = reader.current;
  if (handle === null || currentOwner.current !== owner) return;
  let captured: Awaited<ReturnType<NavigatorReaderHandle["captureReadingPosition"]>>;
  const summary = owner.summary;
  try {
    captured = await handle.captureReadingPosition();
  } catch {
    issues.capture("failed", null);
    return;
  }
  if (captured.status !== "generated") {
    // A location without a durable target (an image page, a superseded turn) is ordinary while
    // reading; the next capture replaces it. Keep it observable in diagnostics, not in the UI.
    const failure = readingPositionCaptureDiagnostic(captured);
    issues.capture(failure.outcome, failure.reason);
    return;
  }
  if (currentOwner.current !== owner) return;
  if (owner.summary !== summary) {
    owner.recaptureRequested = true;
    return;
  }
  const position = copyReadingPosition(captured);
  const key = readingPositionKey(position) + (summary === null ? "" : JSON.stringify(summary));
  if (key === owner.lastQueuedKey) return;
  owner.lastQueuedKey = key;
  owner.sequence += 1;
  owner.pending = {
    summary,
    sequence: owner.sequence,
    position,
    key,
    revisionRetried: false,
  };
  startReadingPositionWrites(owner, currentOwner, issues);
}

export async function checkpointReadingPosition(
  owner: PositionPersistenceOwner,
  reader: { readonly current: NavigatorReaderHandle | null },
  currentOwner: { readonly current: PositionPersistenceOwner | null },
  issues: PositionIssues,
): Promise<void> {
  if (owner.checkpointPromise !== null) return owner.checkpointPromise;
  owner.checkpointPromise = (async () => {
    if (owner.debounceTimer !== null) {
      window.clearTimeout(owner.debounceTimer);
      owner.debounceTimer = null;
    }
    requestReadingPositionCapture(owner, reader, currentOwner, issues);
    while (owner.capturePromise !== null) await owner.capturePromise;
    while (owner.writePromise !== null) await owner.writePromise;
  })().finally(() => {
    owner.checkpointPromise = null;
  });
  return owner.checkpointPromise;
}

function startReadingPositionWrites(
  owner: PositionPersistenceOwner,
  currentOwner: { readonly current: PositionPersistenceOwner | null },
  issues: PositionIssues,
): void {
  if (owner.writePromise !== null || owner.pending === null) return;
  let tracked!: Promise<void>;
  tracked = drainReadingPositionWrites(owner, currentOwner, issues).finally(() => {
    if (owner.writePromise !== tracked) return;
    owner.writePromise = null;
    if (owner.pending !== null) startReadingPositionWrites(owner, currentOwner, issues);
  });
  owner.writePromise = tracked;
}

async function drainReadingPositionWrites(
  owner: PositionPersistenceOwner,
  currentOwner: { readonly current: PositionPersistenceOwner | null },
  issues: PositionIssues,
): Promise<void> {
  try {
    while (owner.pending !== null) {
      const pending = owner.pending;
      owner.pending = null;
      const baseRevision = owner.revision;
      const controller = new AbortController();
      const timeout = window.setTimeout(
        () => controller.abort(),
        READING_POSITION_SAVE_TIMEOUT_MILLISECONDS,
      );
      let response: Response;
      try {
        response = await nativeRequest(
          "/native/reading-position",
          { sessionToken: owner.token },
          {
            method: "PUT",
            json: {
              readerSessionId: owner.sessionId,
              baseRevision,
              sequence: pending.sequence,
              position: pending.position,
              summary: pending.summary,
            },
            signal: controller.signal,
          },
        );
      } finally {
        window.clearTimeout(timeout);
      }
      if (!response.ok && response.status !== 409) {
        throw new Error(`Reading position save failed with HTTP ${response.status}`);
      }
      const saved: unknown = await response.json();
      if (
        response.status === 409 &&
        isSaveReadingPositionConflict(saved) &&
        saved.reason === "revision-mismatch" &&
        !pending.revisionRetried
      ) {
        owner.revision = saved.authoritative.positionRevision;
        if (owner.pending === null) {
          owner.sequence += 1;
          owner.pending = {
            ...pending,
            sequence: owner.sequence,
            revisionRetried: true,
          };
        }
        continue;
      }
      if (!response.ok) {
        throw new Error(`Reading position save failed with HTTP ${response.status}`);
      }
      if (
        !isSaveReadingPositionResponse(saved) ||
        saved.readerSessionId !== owner.sessionId ||
        saved.sequence !== pending.sequence ||
        saved.positionRevision !== baseRevision + 1
      ) {
        throw new Error("Reading position save returned invalid data.");
      }
      if (currentOwner.current !== owner) return;
      owner.revision = saved.positionRevision;
      owner.lastSavedKey = pending.key;
    }
  } catch {
    owner.pending = null;
    owner.lastQueuedKey = owner.lastSavedKey;
    issues.write();
  }
}

export async function flushReadingPosition(
  owner: PositionPersistenceOwner,
  reader: { readonly current: NavigatorReaderHandle | null },
  issues: PositionIssues,
): Promise<void> {
  owner.stopping = true;
  owner.recaptureRequested = false;
  if (owner.debounceTimer !== null) {
    window.clearTimeout(owner.debounceTimer);
    owner.debounceTimer = null;
  }
  await owner.capturePromise;
  const currentOwner = { current: owner };
  await captureReadingPosition(owner, reader, currentOwner, issues);
  while (owner.writePromise !== null) await owner.writePromise;
}
