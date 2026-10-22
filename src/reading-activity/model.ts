export interface ActivityView {
  available: boolean;
  location: string;
  layout: string;
  pages: readonly string[];
}

export interface ActivityDay {
  day: string;
  milliseconds: number;
  pages: number;
}

export function localDay(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function activitySummary(days: readonly ActivityDay[], now = new Date()) {
  const today = localDay(now);
  const current = days.find((day) => day.day === today);
  const active = new Set(days.filter((day) => day.milliseconds >= 10_000).map((day) => day.day));
  const cursor = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 12);
  if (!active.has(today)) cursor.setDate(cursor.getDate() - 1);
  let streak = 0;
  while (active.has(localDay(cursor))) {
    streak++;
    cursor.setDate(cursor.getDate() - 1);
  }
  return {
    streak,
    pages: current?.pages ?? 0,
    minutes: Math.floor((current?.milliseconds ?? 0) / 60_000),
  };
}

export class ReadingActivity {
  readonly days = new Map<string, ActivityDay>();
  private view: ActivityView | null = null;
  private foreground = true;
  private lastTick: number;
  private lastInteraction: number;
  private dwell = 0;
  private credited = new Set<string>();
  private skipPages = false;

  constructor(now: number) {
    this.lastTick = now;
    this.lastInteraction = now;
  }

  tick(now: number) {
    const start = this.lastTick;
    this.lastTick = now;
    if (!this.foreground || !this.view?.available || now - start > 15_000 || now <= start) return;
    const end = Math.min(now, this.lastInteraction + 120_000);
    if (end <= start) return;
    let cursor = start;
    while (cursor < end) {
      const date = new Date(cursor);
      const midnight = new Date(date.getFullYear(), date.getMonth(), date.getDate() + 1).getTime();
      const duration = Math.min(end, midnight) - cursor;
      const key = localDay(date);
      const day = this.days.get(key) ?? { day: key, milliseconds: 0, pages: 0 };
      day.milliseconds += duration;
      this.dwell += duration;
      if (this.dwell >= 10_000 && !this.skipPages) {
        for (const page of this.view.pages) {
          if (!this.credited.has(page)) {
            day.pages++;
            this.credited.add(page);
          }
        }
      }
      this.days.set(key, day);
      cursor += duration;
    }
  }

  observe(view: ActivityView, now: number) {
    this.tick(now);
    const layoutChanged = this.view !== null && view.layout !== this.view.layout;
    if (layoutChanged) {
      this.credited.clear();
      this.skipPages = true;
      this.dwell = 0;
    } else if (view.location !== this.view?.location) {
      this.skipPages = false;
      this.dwell = 0;
      this.lastInteraction = now;
    }
    if (view.available && !this.view?.available) this.lastInteraction = now;
    this.view = view;
  }

  interact(now: number) {
    this.tick(now);
    this.lastInteraction = now;
  }

  visibility(visible: boolean, now: number) {
    if (this.foreground === visible) return;
    this.tick(now);
    this.foreground = visible;
    if (visible) this.lastInteraction = now;
  }
}
