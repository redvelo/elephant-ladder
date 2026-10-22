import { publicationResourceUrl, type MountedPublication } from "epub-navigator";
import type {
  PaginationMeasurementCache,
  PublicationPaginationCapabilities,
} from "epub-navigator/runtime";

const sha256 = /^[0-9a-f]{64}$/;
const base64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const maximumBulkCacheRecords = 32;

type PaginationCacheDescriptor = {
  readonly baseUrl: string;
  readonly handle: string;
  readonly measurementArtifactDigest: string;
};

export type PaginationDescriptor =
  | {
      readonly state: "available";
      readonly responses?: {
        readonly mount: MountedPublication["mount"];
        readonly eligibleResourcePaths: readonly string[];
      };
      readonly foregroundPreparation?: { readonly eligibleResourcePaths: readonly string[] };
      readonly cache?: PaginationCacheDescriptor;
      readonly pacing?: "normal" | "conservative";
    }
  | { readonly state: "unavailable"; readonly reason: "capabilities-unavailable" };

export function isPaginationDescriptor(
  value: unknown,
  measurementOrigin: string,
): value is PaginationDescriptor {
  if (!isRecord(value)) return false;
  if (value.state === "unavailable") {
    return Object.keys(value).length === 2 && value.reason === "capabilities-unavailable";
  }
  if (value.state !== "available" || !isMeasurementOrigin(measurementOrigin)) return false;
  const keys = Object.keys(value);
  const hasResponses = "responses" in value;
  const hasForegroundPreparation = "foregroundPreparation" in value;
  if (
    (!hasResponses && !hasForegroundPreparation) ||
    keys.length !==
      1 +
        Number(hasResponses) +
        Number(hasForegroundPreparation) +
        Number("cache" in value) +
        Number("pacing" in value) ||
    !keys.every((key) =>
      ["state", "responses", "foregroundPreparation", "cache", "pacing"].includes(key),
    ) ||
    ("pacing" in value && value.pacing !== "normal" && value.pacing !== "conservative")
  ) {
    return false;
  }
  const validationMount = {
    baseUrl: `${measurementOrigin}/`,
    remoteResources: "blocked" as const,
    contentOrigin: "isolated-publication-origin" as const,
  };
  if (
    hasResponses &&
    (!isRecord(value.responses) ||
      Object.keys(value.responses).length !== 2 ||
      !isPaginationMount(value.responses.mount, measurementOrigin) ||
      !isEligiblePathList(value.responses.eligibleResourcePaths, validationMount))
  ) {
    return false;
  }
  if (
    "cache" in value &&
    (!isRecord(value.cache) ||
      Object.keys(value.cache).length !== 3 ||
      value.cache.baseUrl !== "/native/pagination-cache/" ||
      typeof value.cache.handle !== "string" ||
      value.cache.handle.length < 32 ||
      typeof value.cache.measurementArtifactDigest !== "string" ||
      !sha256.test(value.cache.measurementArtifactDigest))
  ) {
    return false;
  }
  if (
    hasForegroundPreparation &&
    (!isRecord(value.foregroundPreparation) ||
      Object.keys(value.foregroundPreparation).length !== 1 ||
      !isEligiblePathList(value.foregroundPreparation.eligibleResourcePaths, validationMount))
  ) {
    return false;
  }
  return true;
}

function isEligiblePathList(
  value: unknown,
  mount: MountedPublication["mount"],
): value is readonly string[] {
  if (!Array.isArray(value) || value.length === 0) return false;
  const paths = new Set<string>();
  for (const path of value) {
    if (typeof path !== "string" || paths.has(path)) return false;
    try {
      publicationResourceUrl(mount, path);
    } catch {
      return false;
    }
    paths.add(path);
  }
  return true;
}

function isPaginationMount(
  value: unknown,
  measurementOrigin: string,
): value is MountedPublication["mount"] {
  return (
    isRecord(value) &&
    Object.keys(value).length === 3 &&
    value.baseUrl === `${measurementOrigin}/` &&
    value.remoteResources === "blocked" &&
    value.contentOrigin === "isolated-publication-origin"
  );
}

export function createLivePaginationCapabilities(
  descriptor: Extract<PaginationDescriptor, { state: "available" }>,
  sessionToken = "",
): PublicationPaginationCapabilities {
  const controller = new AbortController();
  let closed = false;
  return Object.freeze({
    ...(descriptor.responses === undefined
      ? {}
      : {
          responses: Object.freeze({
            mount: Object.freeze({ ...descriptor.responses.mount }),
            eligibleResourcePaths: Object.freeze([...descriptor.responses.eligibleResourcePaths]),
          }),
        }),
    ...(descriptor.foregroundPreparation === undefined
      ? {}
      : {
          foregroundPreparation: Object.freeze({
            eligibleResourcePaths: Object.freeze([
              ...descriptor.foregroundPreparation.eligibleResourcePaths,
            ]),
          }),
        }),
    ...(descriptor.cache === undefined
      ? {}
      : {
          cache: createPaginationCache(descriptor.cache, sessionToken),
          cacheIdentity: Object.freeze({
            measurementArtifactDigest: descriptor.cache.measurementArtifactDigest,
          }),
        }),
    ...(descriptor.pacing === undefined ? {} : { pacing: descriptor.pacing }),
    signal: controller.signal,
    close(): void {
      if (closed) return;
      closed = true;
      controller.abort();
    },
  });
}

function createPaginationCache(
  descriptor: PaginationCacheDescriptor,
  sessionToken: string,
): PaginationMeasurementCache {
  const request = async (
    method: "POST" | "PUT" | "DELETE",
    basis: string,
    signal: AbortSignal,
    body?: unknown,
  ): Promise<Response | null> => {
    if (!sha256.test(basis)) return null;
    try {
      return await fetch(`${descriptor.baseUrl}${basis}`, {
        method,
        headers: {
          Authorization: `Bearer ${sessionToken}`,
          "x-elephant-pagination-handle": descriptor.handle,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal,
      });
    } catch (error) {
      if (signal.aborted) throw error;
      return null;
    }
  };
  const cache: PaginationMeasurementCache = {
    async read(basis, record, options) {
      const [value] = await cache.readMany!(basis, [record], options);
      return value ?? null;
    },
    async readMany(basis, records, { signal, maxBytes }) {
      if (records.length === 0) return [];
      if (
        !sha256.test(basis) ||
        records.length > maximumBulkCacheRecords ||
        new Set(records).size !== records.length ||
        records.some((record) => !sha256.test(record))
      ) {
        return records.map(() => null);
      }
      const response = await request("POST", basis, signal, { records });
      if (response === null || response.status === 409) return records.map(() => null);
      if (!response.ok) throw new Error(`pagination cache bulk read failed: ${response.status}`);
      const value: unknown = await response.json();
      if (
        !isRecord(value) ||
        Object.keys(value).length !== 1 ||
        !Array.isArray(value.records) ||
        value.records.length !== records.length
      ) {
        throw new Error("pagination cache bulk read returned invalid data");
      }
      return value.records.map((record) => decodeCacheRecord(record, maxBytes));
    },
    async write(basis, record, value, options) {
      await cache.writeMany!(basis, [{ type: "write", record, value }], options);
    },
    async writeMany(basis, operations, { signal }) {
      if (
        !sha256.test(basis) ||
        operations.length === 0 ||
        operations.length > maximumBulkCacheRecords ||
        new Set(operations.map(({ record }) => record)).size !== operations.length ||
        operations.some(
          (operation) =>
            !sha256.test(operation.record) ||
            (operation.type === "write" && operation.value.byteLength > 65_536),
        )
      ) {
        throw new Error("pagination cache bulk mutation is invalid");
      }
      const response = await request("PUT", basis, signal, {
        operations: operations.map((operation) =>
          operation.type === "write"
            ? { type: "write", record: operation.record, value: encodeCacheRecord(operation.value) }
            : { type: "remove", record: operation.record },
        ),
      });
      requireSuccessfulMutation(response, "bulk mutation");
    },
    async remove(basis, record, options) {
      await cache.writeMany!(basis, [{ type: "remove", record }], options);
    },
    async removeBasis(basis, { signal }) {
      requireSuccessfulMutation(await request("DELETE", basis, signal), "remove basis");
    },
  };
  return Object.freeze(cache);
}

function encodeCacheRecord(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function decodeCacheRecord(value: unknown, maxBytes: number): Uint8Array | null {
  if (typeof value !== "string" || !base64.test(value)) {
    if (value === null) return null;
    throw new Error("pagination cache bulk read returned invalid data");
  }
  if (value.length > Math.ceil(maxBytes / 3) * 4) return null;
  const decoded = atob(value);
  if (decoded.length > maxBytes) return null;
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function requireSuccessfulMutation(response: Response | null, operation: string): void {
  if (response === null) throw new Error(`pagination cache ${operation} failed`);
  if (!response.ok) {
    throw new Error(`pagination cache ${operation} failed: ${response.status}`);
  }
}

function isMeasurementOrigin(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.username === "" &&
      url.password === "" &&
      url.pathname === "/" &&
      url.search === "" &&
      url.hash === "" &&
      value === url.origin
    );
  } catch {
    return false;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
