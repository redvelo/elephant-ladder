import { atom, useAtomValue, useSetAtom } from "jotai";
import { useEffect, useRef, type RefObject } from "react";
import type { NavigatorFullscreenController } from "epub-navigator/react";
import { isRecord } from "../host/guards";
import { authenticatedPostEmpty, nativeRequest } from "../host/requests";
import { bootstrapAtom } from "../app/session";
import { readerNoticeAtom } from "../reader/state";

const ROUTE = "/native/window/fullscreen";
// A fullscreen transition always resizes the window, and only the size it settles on matters.
const RESIZE_SETTLE_MS = 150;
// A request the compositor refuses resizes nothing, so the state is read back regardless.
const CONFIRM_MS = 300;

export const windowFullscreenAvailableAtom = atom(
  (get) => get(bootstrapAtom)?.nativeActions.windowFullscreen === true,
);
export const windowFullscreenAtom = atom(false);

export const syncWindowFullscreenAtom = atom(null, async (get, set): Promise<void> => {
  const bootstrap = get(bootstrapAtom);
  if (bootstrap === null || !get(windowFullscreenAvailableAtom)) return;
  try {
    const response = await nativeRequest(ROUTE, { sessionToken: bootstrap.sessionToken });
    if (!response.ok) return;
    const body: unknown = await response.json();
    if (!isRecord(body) || typeof body.fullscreen !== "boolean") return;
    set(windowFullscreenAtom, body.fullscreen);
  } catch {
    // The window's state is read again on the next resize.
  }
});

export const setWindowFullscreenAtom = atom(
  null,
  async (get, set, fullscreen: boolean): Promise<void> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap === null || !get(windowFullscreenAvailableAtom)) return;
    try {
      await authenticatedPostEmpty(ROUTE, bootstrap.sessionToken, { fullscreen });
      set(windowFullscreenAtom, fullscreen);
    } catch {
      set(readerNoticeAtom, "Fullscreen could not be changed.");
    }
  },
);

export function useWindowFullscreen(): NavigatorFullscreenController | null {
  const available = useAtomValue(windowFullscreenAvailableAtom);
  const active = useAtomValue(windowFullscreenAtom);
  const setFullscreen = useSetAtom(setWindowFullscreenAtom);
  const sync = useSetAtom(syncWindowFullscreenAtom);
  const timer = useRef<Timer | null>(null);

  useEffect(() => {
    if (!available) return;
    void sync();
    const onResize = () => syncAfter(timer, sync, RESIZE_SETTLE_MS);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
    };
  }, [available, sync]);

  if (!available) return null;
  return {
    active,
    toggle: () => {
      void setFullscreen(!active).then(() => syncAfter(timer, sync, CONFIRM_MS));
    },
  };
}

type Timer = ReturnType<typeof setTimeout>;

function syncAfter(timer: RefObject<Timer | null>, sync: () => Promise<void>, delay: number) {
  if (timer.current !== null) clearTimeout(timer.current);
  timer.current = setTimeout(() => {
    timer.current = null;
    void sync();
  }, delay);
}
