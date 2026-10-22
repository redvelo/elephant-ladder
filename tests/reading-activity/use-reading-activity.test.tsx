// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { useReadingActivity } from "../../src/reading-activity/use-reading-activity";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function Harness({ token, session }: { token: string | undefined; session: string }) {
  useReadingActivity(token, session);
  return null;
}

function visibilityListeners(spy: { mock: { calls: readonly unknown[][] } }): number {
  return spy.mock.calls.filter((call) => call[0] === "visibilitychange").length;
}

/**
 * The activity effect writes on teardown, so re-running it per render would both reinstall six
 * listeners and send a reading-activity write. Its inputs must stay stable across a render that
 * changed nothing, which the React Compiler is responsible for.
 */
test("a render that changes nothing leaves the activity effect installed", () => {
  const listen = vi.spyOn(document, "addEventListener");
  const schedule = vi.spyOn(window, "setInterval");
  const { rerender } = render(<Harness token="token" session="session" />);
  const installed = visibilityListeners(listen);
  const scheduled = schedule.mock.calls.length;
  expect(installed).toBe(1);
  rerender(<Harness token="token" session="session" />);
  expect(visibilityListeners(listen)).toBe(installed);
  expect(schedule.mock.calls).toHaveLength(scheduled);
});

test("the session token arriving leaves the listeners in place", () => {
  const listen = vi.spyOn(document, "addEventListener");
  const { rerender } = render(<Harness token={undefined} session="session" />);
  const installed = visibilityListeners(listen);
  rerender(<Harness token="token" session="session" />);
  expect(visibilityListeners(listen)).toBe(installed);
});

test("a new session reinstalls the activity effect", () => {
  const listen = vi.spyOn(document, "addEventListener");
  const { rerender } = render(<Harness token="token" session="first" />);
  const installed = visibilityListeners(listen);
  rerender(<Harness token="token" session="second" />);
  expect(visibilityListeners(listen)).toBe(installed + 1);
});
