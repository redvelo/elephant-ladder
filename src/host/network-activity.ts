import { nativeRequest } from "./requests";

export interface NetworkActivity {
  readonly sessionId: string;
  readonly lastObservedAt: number;
}

interface ActivitySnapshot {
  readonly sessionId: string;
  readonly available: boolean;
  readonly lastObservedAt: number | null;
}

function isSnapshot(value: unknown, sessionId: string): value is ActivitySnapshot {
  if (value === null || typeof value !== "object") return false;
  const snapshot = value as Record<string, unknown>;
  return (
    Object.keys(snapshot).length === 3 &&
    snapshot.sessionId === sessionId &&
    typeof snapshot.available === "boolean" &&
    (snapshot.lastObservedAt === null ||
      (typeof snapshot.lastObservedAt === "number" &&
        Number.isSafeInteger(snapshot.lastObservedAt) &&
        snapshot.lastObservedAt >= 0 &&
        snapshot.lastObservedAt <= Date.now()))
  );
}

/** One visible-reader polling owner. Reads never manufacture a new observation time. */
export function observeNetworkActivity(
  sessionId: string,
  token: string,
  onActivity: (activity: NetworkActivity | null) => void,
  page: {
    document: EventTarget & { readonly visibilityState: string };
    window: EventTarget;
  } = { document, window },
): () => void {
  let disposed = false;
  let terminal = false;
  let pageHidden = false;
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let request: AbortController | null = null;
  let failures = 0;
  let lastObservedAt: number | null = null;
  onActivity(null);

  const publish = (at: number | null): void => {
    if (lastObservedAt === at) return;
    lastObservedAt = at;
    onActivity(at === null ? null : { sessionId, lastObservedAt: at });
  };
  const visible = (): boolean =>
    !disposed && !terminal && !pageHidden && page.document.visibilityState === "visible";

  const poll = async (): Promise<void> => {
    if (!visible() || request !== null) return;
    const owner = generation;
    const controller = new AbortController();
    request = controller;
    const timeout = setTimeout(() => controller.abort(), 5_000);
    let delay = 2_000;
    try {
      const response = await nativeRequest(
        "/native/reader/network-activity",
        { sessionToken: token, readerSessionId: sessionId, trusted: true },
        { signal: controller.signal },
      );
      if (owner !== generation || !visible()) return;
      if ([401, 403, 409].includes(response.status)) {
        terminal = true;
        publish(null);
        return;
      }
      if (!response.ok) throw new Error("Network activity read failed");
      const snapshot: unknown = await response.json();
      if (owner !== generation || !visible()) return;
      if (controller.signal.aborted) throw new Error("Network activity read timed out");
      if (!isSnapshot(snapshot, sessionId)) throw new Error("Invalid network activity snapshot");
      failures = 0;
      publish(snapshot.available ? snapshot.lastObservedAt : null);
      // Unsupported platforms stay quiet, but a temporarily unavailable observer can recover.
      if (!snapshot.available) delay = 30_000;
    } catch {
      if (owner !== generation || !visible()) return;
      delay = Math.min(10_000, 2_000 * 2 ** Math.min(++failures, 3));
    } finally {
      clearTimeout(timeout);
      if (owner === generation) {
        request = null;
        if (visible()) timer = setTimeout(() => void poll(), delay);
      }
    }
  };

  const suspend = (): void => {
    generation += 1;
    clearTimeout(timer);
    request?.abort();
    request = null;
    publish(null);
  };
  const visibilityChanged = (): void => {
    if (visible()) {
      clearTimeout(timer);
      void poll();
    } else {
      suspend();
    }
  };
  const pageHide = (): void => {
    pageHidden = true;
    suspend();
  };
  const pageShow = (): void => {
    pageHidden = false;
    visibilityChanged();
  };
  page.document.addEventListener("visibilitychange", visibilityChanged);
  page.window.addEventListener("pagehide", pageHide);
  page.window.addEventListener("pageshow", pageShow);
  void poll();
  return () => {
    disposed = true;
    suspend();
    page.document.removeEventListener("visibilitychange", visibilityChanged);
    page.window.removeEventListener("pagehide", pageHide);
    page.window.removeEventListener("pageshow", pageShow);
  };
}
