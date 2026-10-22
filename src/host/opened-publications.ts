import { nativeRequest } from "./requests";

/** A publication the platform asked this application to open, waiting to be imported. */
export interface OpenedPublicationArrival {
  readonly openId: string;
  readonly fileName: string;
  readonly sizeBytes: number;
  /** Whether the platform was asked to read this book now, or only to keep it. */
  readonly intent: "read" | "keep";
}

const retryDelayMs = 2_000;

export async function watchOpenedPublications(
  sessionToken: string,
  signal: AbortSignal,
  onArrival: (arrival: OpenedPublicationArrival) => void,
): Promise<void> {
  let after: string | null = null;
  while (!signal.aborted) {
    try {
      const query: string = after === null ? "" : `?after=${encodeURIComponent(after)}`;
      const response = await nativeRequest(
        `/native/opened-publications/pending${query}`,
        { sessionToken, trusted: true },
        { signal },
      );
      if (response.status === 204) continue;
      const value: unknown = response.ok ? await response.json() : null;
      if (!isOpenedPublicationArrival(value))
        throw new Error("invalid opened publication response");
      after = value.openId;
      onArrival(value);
    } catch {
      if (signal.aborted) return;
      await new Promise((resolve) => setTimeout(resolve, retryDelayMs));
    }
  }
}

function isOpenedPublicationArrival(value: unknown): value is OpenedPublicationArrival {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.openId === "string" &&
    typeof record.fileName === "string" &&
    record.fileName.length > 0 &&
    typeof record.sizeBytes === "number" &&
    Number.isSafeInteger(record.sizeBytes) &&
    record.sizeBytes > 0 &&
    (record.intent === "read" || record.intent === "keep")
  );
}
