import {
  isNavigatorMediaOverlayDescription,
  isNavigatorMediaOverlayTimelineResult,
  type NavigatorMediaOverlayAvailability,
  type NavigatorMediaOverlayProviderIssue,
} from "epub-navigator";
import type { PublicationMediaOverlayCapabilities } from "epub-navigator/runtime";
import { nativeRequest } from "../../host/requests";

export function createNativeMediaOverlayCapabilities(
  readerSessionId: string,
  sessionToken: string,
  availability: NavigatorMediaOverlayAvailability,
): PublicationMediaOverlayCapabilities {
  const controller = new AbortController();
  const base = `/native/reader-sessions/${encodeURIComponent(readerSessionId)}/media-overlays`;

  const request = async (
    path: string,
    signal: AbortSignal,
  ): Promise<
    | { readonly status: "ok"; readonly value: unknown }
    | { readonly status: "unavailable"; readonly reason: NavigatorMediaOverlayProviderIssue }
  > => {
    const combined = AbortSignal.any([controller.signal, signal]);
    let response: Response;
    try {
      response = await nativeRequest(path, { sessionToken, trusted: true }, { signal: combined });
    } catch (error) {
      if (combined.aborted) throw error;
      return { status: "unavailable", reason: "host-unavailable" };
    }
    if (response.status === 409) return { status: "unavailable", reason: "session-replaced" };
    if (response.status === 404) return { status: "unavailable", reason: "not-found" };
    if (!response.ok) return { status: "unavailable", reason: "host-unavailable" };
    try {
      return { status: "ok", value: await response.json() };
    } catch (error) {
      if (combined.aborted) throw error;
      return { status: "unavailable", reason: "host-unavailable" };
    }
  };

  return {
    availability,
    signal: controller.signal,
    async description(context) {
      const outcome = await request(base, context.signal);
      if (outcome.status !== "ok") return outcome;
      return isNavigatorMediaOverlayDescription(outcome.value)
        ? { status: "ok", description: outcome.value }
        : { status: "unavailable", reason: "host-unavailable" };
    },
    async timeline(overlayIndex, context) {
      if (!Number.isSafeInteger(overlayIndex) || overlayIndex < 0) {
        return { status: "unavailable", reason: "not-found" };
      }
      const outcome = await request(`${base}/${overlayIndex}`, context.signal);
      if (outcome.status !== "ok") return outcome;
      return isNavigatorMediaOverlayTimelineResult(outcome.value)
        ? { status: "ok", result: outcome.value }
        : { status: "unavailable", reason: "host-unavailable" };
    },
    close() {
      controller.abort();
    },
  };
}
