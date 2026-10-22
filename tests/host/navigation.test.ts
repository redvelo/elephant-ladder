import { expect, test } from "vitest";
import { commitBootstrapRecovery, isNavigationPath } from "../../src/host/navigation";

const path = `/window/${"a".repeat(64)}/start`;

test("trusted recovery accepts only exact one-time navigation paths", () => {
  expect(isNavigationPath(path)).toBe(true);
  expect(isNavigationPath(`${path}?retry=1`)).toBe(false);
  expect(isNavigationPath(`http://reader.test${path}`)).toBe(false);
  expect(isNavigationPath(`/window/${"A".repeat(64)}/start`)).toBe(false);
  expect(isNavigationPath("/")).toBe(false);
});

test("bootstrap recovery validates before changing the document", () => {
  const events: string[] = [];
  const element = { remove: () => events.push("remove") };
  const navigationHistory = {
    replaceState: (_data: unknown, _unused: string, url?: string | URL | null) =>
      events.push(`replace:${String(url)}`),
  };

  expect(() => commitBootstrapRecovery(element, "/", navigationHistory)).toThrow(
    /recovery path is invalid/,
  );
  expect(events).toStrictEqual([]);

  commitBootstrapRecovery(element, path, navigationHistory);
  expect(events).toStrictEqual(["remove", `replace:${path}`]);
});
