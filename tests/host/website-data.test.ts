import { expect, test } from "vitest";

import { isWebsiteDataResetResponse } from "../../src/host/website-data";

const purgeReport = {
  androidWebStorage: "attempted",
  cookies: "unsupported",
  cacheStorage: "unsupported",
  workerStorage: "unsupported",
  httpAuthentication: "unsupported",
  formData: "unsupported",
  opfs: "unsupported",
};

test("website-data reset response accepts only exact partial-purge results", () => {
  expect(
    isWebsiteDataResetResponse({ status: "reset", physicalPurge: "partial", purgeReport }),
  ).toBe(true);
  expect(
    isWebsiteDataResetResponse({
      status: "reset",
      physicalPurge: "unsupported",
      purgeReport: { ...purgeReport, androidWebStorage: "unsupported" },
    }),
  ).toBe(true);
  expect(isWebsiteDataResetResponse({ status: "reset" })).toBe(false);
  expect(
    isWebsiteDataResetResponse({ status: "reset", physicalPurge: "purged", purgeReport }),
  ).toBe(false);
  expect(
    isWebsiteDataResetResponse({
      status: "reset",
      physicalPurge: "partial",
      purgeReport,
      extra: true,
    }),
  ).toBe(false);
  expect(
    isWebsiteDataResetResponse({
      status: "reset",
      physicalPurge: "partial",
      purgeReport: { ...purgeReport, cookies: "cleared" },
    }),
  ).toBe(false);
});
