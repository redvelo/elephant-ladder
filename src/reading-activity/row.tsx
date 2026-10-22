import { useEffect, useState } from "react";
import { activitySummary, type ActivityDay } from "./model";
import { nativeRequest } from "../host/requests";

export function ReadingActivityRow({ sessionToken }: { sessionToken: string | null }) {
  const [days, setDays] = useState<ActivityDay[] | null>(null);
  const [now, setNow] = useState(() => new Date());
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (sessionToken === null) return;
    const controller = new AbortController();
    const load = () => {
      void loadActivity(sessionToken, controller.signal)
        .then((data) => {
          if (controller.signal.aborted) return;
          setDays(data);
          setFailed(false);
          setNow(new Date());
        })
        .catch(() => {
          if (!controller.signal.aborted) setFailed(true);
        });
    };
    load();
    const timer = window.setInterval(load, 60_000);
    const visible = () => {
      if (document.visibilityState === "visible") load();
    };
    document.addEventListener("visibilitychange", visible);
    return () => {
      controller.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [sessionToken]);
  const summary = days === null ? null : activitySummary(days, now);
  return (
    <section className="home-reading-activity" aria-label="Reading activity">
      <dl>
        <div>
          <dt>Daily streak</dt>
          <dd>
            {summary === null ? "—" : `${summary.streak} ${summary.streak === 1 ? "day" : "days"}`}
          </dd>
        </div>
        <div>
          <dt>Pages read today</dt>
          <dd>{summary?.pages.toLocaleString() ?? "—"}</dd>
        </div>
        <div>
          <dt>Reading today</dt>
          <dd>{summary === null ? "—" : `${summary.minutes} min`}</dd>
        </div>
      </dl>
      {failed ? <span role="status">Reading activity unavailable</span> : null}
    </section>
  );
}

async function loadActivity(sessionToken: string, signal: AbortSignal): Promise<ActivityDay[]> {
  const response = await nativeRequest(
    "/native/reading-activity",
    { sessionToken, trusted: true },
    { signal },
  );
  if (!response.ok) throw new Error("Reading activity unavailable");
  const data: unknown = await response.json();
  if (
    !Array.isArray(data) ||
    !data.every(
      (day: ActivityDay) =>
        typeof day?.day === "string" &&
        Number.isFinite(day.milliseconds) &&
        day.milliseconds >= 0 &&
        Number.isFinite(day.pages) &&
        day.pages >= 0,
    )
  )
    throw new Error("Invalid reading activity");
  return data;
}
