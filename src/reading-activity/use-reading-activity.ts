import { useEffect, useRef, type RefObject } from "react";
import { ReadingActivity, type ActivityView } from "./model";
import { nativeRequest } from "../host/requests";

async function saveActivity(
  current: { tracker: ReadingActivity; id: string } | null,
  sessionToken: string | undefined,
) {
  if (current === null || sessionToken === undefined) return;
  current.tracker.tick(Date.now());
  await Promise.all(
    [...current.tracker.days.values()].map(async (day) => {
      const response = await nativeRequest(
        "/native/reading-activity",
        { sessionToken },
        { method: "PUT", keepalive: true, json: { recordingId: current.id, ...day } },
      );
      if (!response.ok) throw new Error("Reading activity could not be saved.");
    }),
  );
}

export function useReadingActivity(sessionToken: string | undefined, sessionId: string) {
  const owner = useRef<{ tracker: ReadingActivity; id: string } | null>(null);
  const token = useRef(sessionToken);
  useEffect(() => {
    token.current = sessionToken;
  }, [sessionToken]);
  const observe = (view: ActivityView) => {
    observeActivity(owner, view);
  };
  /** Writes what is recorded so far, so leaving a book can wait for its activity to land. */
  const flush = () => saveActivity(owner.current, token.current);
  // The listeners read the token from a ref rather than depending on it: this effect writes on
  // teardown, so it may only be torn down when the reading session it records actually ends.
  useEffect(() => {
    const save = () => {
      void saveActivity(owner.current, token.current).catch(() => {});
    };
    const visibility = () => {
      owner.current?.tracker.visibility(
        document.visibilityState === "visible" && document.hasFocus(),
        Date.now(),
      );
      save();
    };
    const focus = () => queueMicrotask(visibility);
    const interact = () => owner.current?.tracker.interact(Date.now());
    const timer = window.setInterval(save, 5000);
    document.addEventListener("visibilitychange", visibility);
    window.addEventListener("focus", focus);
    window.addEventListener("blur", focus);
    window.addEventListener("pagehide", save);
    document.addEventListener("pointerdown", interact, true);
    document.addEventListener("keydown", interact, true);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", visibility);
      window.removeEventListener("focus", focus);
      window.removeEventListener("blur", focus);
      window.removeEventListener("pagehide", save);
      document.removeEventListener("pointerdown", interact, true);
      document.removeEventListener("keydown", interact, true);
      save();
      owner.current = null;
    };
  }, [sessionId]);
  return { observe, flush };
}

function observeActivity(
  owner: RefObject<{ tracker: ReadingActivity; id: string } | null>,
  view: ActivityView,
) {
  owner.current ??= { tracker: new ReadingActivity(Date.now()), id: crypto.randomUUID() };
  owner.current.tracker.observe(view, Date.now());
  owner.current.tracker.visibility(
    document.visibilityState === "visible" && document.hasFocus(),
    Date.now(),
  );
}
