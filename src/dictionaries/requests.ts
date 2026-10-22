import { isRecord } from "../host/guards";
import { nativeRequest } from "../host/requests";
import type {
  CatalogPackSummary,
  DictionaryAudio,
  DictionaryCatalog,
  DictionaryInstallation,
  DictionaryOperation,
  DictionaryOperationPhase,
  DictionaryRequestContext,
} from "./model";
import { isTerminalPhase } from "./model";

/** The catalog could not be read, which leaves installed dictionaries the only facts. */
export class DictionaryCatalogUnavailable extends Error {
  constructor() {
    super("The dictionary catalog could not be reached.");
    this.name = "DictionaryCatalogUnavailable";
  }
}

export async function fetchDictionaryCatalog(
  context: DictionaryRequestContext,
): Promise<DictionaryCatalog> {
  const value = await send(context, "GET", "/native/dictionary/catalog");
  if (!isCatalog(value)) throw new Error("The dictionary catalog is invalid.");
  return value;
}

export async function fetchDictionaryInstallations(
  context: DictionaryRequestContext,
): Promise<DictionaryInstallation[]> {
  const value = await send(context, "GET", "/native/dictionary/installations");
  if (!isInstallations(value)) throw new Error("Installed dictionaries are invalid.");
  return value.installations;
}

/**
 * Starts one language's install and returns the operation that reports it.
 *
 * The host installs the language's corpus and the pronunciations that come with it under
 * this one operation, so an install is one download even after a restart.
 */
export async function startDictionaryInstall(
  context: DictionaryRequestContext,
  corpusLanguage: string,
  audio: DictionaryAudio,
): Promise<string> {
  const value = await send(context, "POST", "/native/dictionary/installations", {
    corpusLanguage,
    audio,
  });
  if (!isRecord(value) || typeof value.operationId !== "string" || value.operationId.length === 0) {
    throw new Error("The dictionary install did not start.");
  }
  return value.operationId;
}

/** Operations the dialog or the library may still be watching, newest first. */
export async function fetchDictionaryOperations(
  context: DictionaryRequestContext,
): Promise<DictionaryOperation[]> {
  const value = await send(context, "GET", "/native/dictionary/operations");
  if (
    !isRecord(value) ||
    !Array.isArray(value.operations) ||
    !value.operations.every(isOperation)
  ) {
    throw new Error("The dictionary operations are invalid.");
  }
  return value.operations;
}

/** The operation still running, if any. */
export function activeOperation(
  operations: readonly DictionaryOperation[],
): DictionaryOperation | null {
  return operations.find((operation) => !isTerminalPhase(operation.phase)) ?? null;
}

async function readDictionaryOperation(
  context: DictionaryRequestContext,
  operationId: string,
): Promise<DictionaryOperation> {
  const value = await send(
    context,
    "GET",
    `/native/dictionary/operations/${encodeURIComponent(operationId)}`,
  );
  if (!isOperation(value)) throw new Error("The dictionary operation is invalid.");
  return value;
}

export async function cancelDictionaryOperation(
  context: DictionaryRequestContext,
  operationId: string,
): Promise<void> {
  await send(context, "DELETE", `/native/dictionary/operations/${encodeURIComponent(operationId)}`);
}

export async function deleteDictionaryInstallation(
  context: DictionaryRequestContext,
  packId: string,
  packRevision: string,
): Promise<void> {
  await send(
    context,
    "DELETE",
    `/native/dictionary/installations/${encodeURIComponent(packId)}/${encodeURIComponent(packRevision)}`,
  );
}

export interface WatchOptions {
  readonly intervalMs?: number;
  readonly wait?: (milliseconds: number) => Promise<void>;
  readonly onChange: (operation: DictionaryOperation | null) => void;
}

/**
 * Reports the running install until none is left.
 *
 * This follows the operation rather than any one surface, so progress keeps arriving after
 * the dictionaries dialog closes.
 */
export async function watchDictionaryOperations(
  context: DictionaryRequestContext,
  options: WatchOptions,
): Promise<void> {
  const interval = options.intervalMs ?? 1000;
  const wait =
    options.wait ??
    ((milliseconds: number) => new Promise<void>((resolve) => setTimeout(resolve, milliseconds)));
  const stopped = () => context.signal?.aborted === true;
  for (;;) {
    let running: DictionaryOperation | null;
    try {
      running = activeOperation(await fetchDictionaryOperations(context));
    } catch {
      options.onChange(null);
      return;
    }
    if (stopped()) return;
    options.onChange(running);
    if (running === null) return;
    await wait(interval);
    if (stopped()) return;
  }
}

export interface PollOptions {
  readonly intervalMs?: number;
  readonly onProgress?: (operation: DictionaryOperation) => void;
  readonly wait?: (milliseconds: number) => Promise<void>;
}

/** Polls one operation until it reaches a terminal phase, the signal aborts, or it fails. */
export async function pollDictionaryOperation(
  context: DictionaryRequestContext,
  operationId: string,
  options: PollOptions = {},
): Promise<DictionaryOperation> {
  const interval = options.intervalMs ?? 1000;
  const wait =
    options.wait ??
    ((milliseconds: number) => new Promise<void>((resolve) => setTimeout(resolve, milliseconds)));
  for (;;) {
    const operation = await readDictionaryOperation(context, operationId);
    options.onProgress?.(operation);
    if (isTerminalPhase(operation.phase)) return operation;
    if (context.signal?.aborted === true) return operation;
    await wait(interval);
  }
}

async function send(
  context: DictionaryRequestContext,
  method: "GET" | "POST" | "DELETE",
  path: string,
  body?: unknown,
): Promise<unknown> {
  const response = await nativeRequest(
    path,
    { sessionToken: context.sessionToken, libraryDocumentId: context.libraryDocumentId },
    {
      method,
      ...(body === undefined ? {} : { json: body }),
      signal: context.signal,
      fetcher: context.request,
    },
  );
  const value: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    if (isRecord(value) && value.reason === "catalog-unavailable") {
      throw new DictionaryCatalogUnavailable();
    }
    throw new Error(dictionaryFailure(value) ?? `${path} failed with HTTP ${response.status}`);
  }
  if (!isRecord(value) || value.status !== "completed") {
    throw new Error(dictionaryFailure(value) ?? `${path} returned an invalid response.`);
  }
  return value;
}

function dictionaryFailure(value: unknown): string | null {
  if (!isRecord(value)) return null;
  if (typeof value.message === "string" && value.message.length > 0) return value.message;
  if (typeof value.reason === "string" && value.reason.length > 0) return value.reason;
  return null;
}

function isCatalog(value: unknown): value is DictionaryCatalog {
  return (
    isRecord(value) &&
    typeof value.catalogRevision === "string" &&
    Array.isArray(value.packs) &&
    value.packs.every(isCatalogPack)
  );
}

function isCatalogPack(value: unknown): value is CatalogPackSummary {
  if (
    !isRecord(value) ||
    typeof value.packId !== "string" ||
    !isRevision(value.packRevision) ||
    typeof value.corpusLanguage !== "string" ||
    !isCount(value.installedBytes) ||
    typeof value.installed !== "boolean"
  ) {
    return false;
  }
  if (value.packType === "audio") {
    return (
      (value.delivery === "bundled" || value.delivery === "remote") && isCount(value.recordings)
    );
  }
  return (
    value.packType === "corpus" &&
    typeof value.edition === "string" &&
    typeof value.dumpDate === "string" &&
    typeof value.monolingual === "boolean" &&
    Array.isArray(value.bilingualTargets) &&
    value.bilingualTargets.every((target) => typeof target === "string") &&
    Array.isArray(value.licenses) &&
    value.licenses.every((license) => typeof license === "string")
  );
}

function isInstallations(value: unknown): value is { installations: DictionaryInstallation[] } {
  return (
    isRecord(value) &&
    Array.isArray(value.installations) &&
    value.installations.every(
      (installation) =>
        isRecord(installation) &&
        (installation.packType === "corpus" || installation.packType === "audio") &&
        typeof installation.packId === "string" &&
        isRevision(installation.packRevision) &&
        typeof installation.corpusLanguage === "string" &&
        isCount(installation.installedBytes) &&
        isDelivery(installation.delivery),
    )
  );
}

/** Only an opened pronunciation collection reports a delivery; everything else has none. */
function isDelivery(value: unknown): boolean {
  return value === null || value === "bundled" || value === "remote";
}

function isOperation(value: unknown): value is DictionaryOperation {
  return (
    isRecord(value) &&
    typeof value.operationId === "string" &&
    typeof value.corpusLanguage === "string" &&
    value.corpusLanguage.length > 0 &&
    (value.audio === "streamed" || value.audio === "stored") &&
    typeof value.phase === "string" &&
    isPhase(value.phase) &&
    isCount(value.bytesCompleted) &&
    isCount(value.bytesTotal) &&
    (value.failure === null || typeof value.failure === "string")
  );
}

function isPhase(value: string): value is DictionaryOperationPhase {
  return (
    isTerminalPhase(value as DictionaryOperationPhase) ||
    ["catalog", "preflight", "downloading", "verifying", "installing"].includes(value)
  );
}

function isRevision(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function isCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}
