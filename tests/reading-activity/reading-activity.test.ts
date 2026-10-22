import { expect, test } from "vitest";
import {
  ReadingActivity,
  activitySummary,
  localDay,
  type ActivityView,
} from "../../src/reading-activity/model";

const start = new Date(2026, 8, 23, 12).getTime();
const view: ActivityView = { available: true, location: "one", layout: "default", pages: ["one"] };

test("quick jumps do not count skipped pages and repeated observations do not duplicate credit", () => {
  const tracker = new ReadingActivity(start);
  tracker.observe(view, start);
  tracker.observe({ ...view, location: "ten", pages: ["ten", "eleven"] }, start + 1000);
  tracker.tick(start + 6000);
  tracker.tick(start + 11000);
  tracker.observe({ ...view, location: "eleven", pages: ["eleven", "twelve"] }, start + 11000);
  tracker.tick(start + 21000);
  expect([...tracker.days.values()]).toEqual([
    { day: localDay(new Date(start)), milliseconds: 21000, pages: 3 },
  ]);
});

test("panels, backgrounding, idle time and suspended timers are excluded", () => {
  const tracker = new ReadingActivity(start);
  tracker.observe(view, start);
  tracker.tick(start + 5000);
  tracker.visibility(false, start + 5000);
  tracker.tick(start + 10000);
  tracker.visibility(true, start + 10000);
  tracker.observe({ ...view, available: false }, start + 15000);
  tracker.tick(start + 20000);
  tracker.observe(view, start + 20000);
  for (let t = 25000; t <= 150000; t += 5000) tracker.tick(start + t);
  expect([...tracker.days.values()][0]?.milliseconds).toBe(130000);
  tracker.interact(start + 150000);
  tracker.tick(start + 300000);
  expect([...tracker.days.values()][0]?.milliseconds).toBe(130000);
});

test("repagination does not credit the same view again", () => {
  const tracker = new ReadingActivity(start);
  tracker.observe(view, start);
  tracker.tick(start + 10000);
  tracker.observe({ ...view, layout: "large", location: "two", pages: ["two"] }, start + 10000);
  tracker.tick(start + 20000);
  expect([...tracker.days.values()][0]?.pages).toBe(1);
  tracker.observe({ ...view, layout: "large", location: "three", pages: ["three"] }, start + 20000);
  tracker.tick(start + 30000);
  expect([...tracker.days.values()][0]?.pages).toBe(2);
});

test("reading time splits across local midnight", () => {
  const before = new Date(2026, 8, 23, 23, 59, 55).getTime();
  const tracker = new ReadingActivity(before);
  tracker.observe(view, before);
  tracker.tick(before + 10000);
  expect([...tracker.days.values()]).toEqual([
    { day: "2026-09-23", milliseconds: 5000, pages: 0 },
    { day: "2026-09-24", milliseconds: 5000, pages: 1 },
  ]);
});

test("streak includes yesterday until reading today and breaks at missing days", () => {
  const days = [22, 21, 19].map((day) => ({
    day: `2026-09-${day}`,
    milliseconds: 60000,
    pages: 2,
  }));
  expect(activitySummary(days, new Date(start))).toEqual({ streak: 2, minutes: 0, pages: 0 });
  days.push({ day: "2026-09-23", milliseconds: 125000, pages: 3 });
  expect(activitySummary(days, new Date(start))).toEqual({ streak: 3, minutes: 2, pages: 3 });
});
