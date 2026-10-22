import { nativeRequest } from "../host/requests";

export type AnnotationLinkArrival =
  | { readonly status: "open"; readonly linkId: string; readonly publicationId: string }
  | { readonly status: "missing"; readonly linkId: string };

const retryDelayMs = 2_000;

export async function watchAnnotationLinks(
  sessionToken: string,
  signal: AbortSignal,
  onLink: (link: AnnotationLinkArrival) => void,
): Promise<void> {
  let after: string | null = null;
  while (!signal.aborted) {
    try {
      const query: string = after === null ? "" : `?after=${encodeURIComponent(after)}`;
      const response = await nativeRequest(
        `/native/annotation-links/pending${query}`,
        { sessionToken, trusted: true },
        { signal },
      );
      if (response.status === 204) continue;
      const value: unknown = response.ok ? await response.json() : null;
      if (!isAnnotationLinkArrival(value)) throw new Error("invalid annotation link response");
      after = value.linkId;
      onLink(value);
    } catch {
      if (signal.aborted) return;
      await new Promise((resolve) => setTimeout(resolve, retryDelayMs));
    }
  }
}

function isAnnotationLinkArrival(value: unknown): value is AnnotationLinkArrival {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  if (typeof record.linkId !== "string") return false;
  return (
    (record.status === "open" && typeof record.publicationId === "string") ||
    record.status === "missing"
  );
}
