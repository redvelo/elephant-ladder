import { expect, test } from "vitest";

import { assessWebsiteDataPurgeLogs } from "../../scripts/collect-android-website-data.mjs";

test("Android website-data evidence qualifies only an explicitly partial platform attempt", () => {
  const qualified = assessWebsiteDataPurgeLogs(
    [
      "I NativeActionsPlugin: publication-origin WebStorage purge attempted",
      '{"event":"publication_website_data_reset","physical_purge":"partial"}',
    ].join("\n"),
  );
  expect(qualified).toStrictEqual({
    status: "attempt-qualified",
    checks: {
      platformAttempt: true,
      hostPartial: true,
      neverPromotedToPurged: true,
    },
  });
  expect(assessWebsiteDataPurgeLogs("publication-origin WebStorage purge attempted").status).toBe(
    "incomplete",
  );
  expect(
    assessWebsiteDataPurgeLogs(
      [
        "publication-origin WebStorage purge attempted",
        '{"event":"publication_website_data_reset","physical_purge":"purged"}',
      ].join("\n"),
    ).status,
  ).toBe("incomplete");
});
