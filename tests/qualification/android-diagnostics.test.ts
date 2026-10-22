import { expect, test } from "vitest";
import { parseApkApplicationId } from "../../scripts/android-device.mjs";

test("the Android smoke check accepts only the production APK identity", () => {
  expect(
    parseApkApplicationId("package: name='com.redvelo.elephant_ladder' versionCode='1000'"),
  ).toBe("com.redvelo.elephant_ladder");
  expect(parseApkApplicationId("package: name='com.redvelo.elephant_ladder.fixture_runner'")).toBe(
    "com.redvelo.elephant_ladder.fixture_runner",
  );
  expect(parseApkApplicationId("not APK badging")).toBe(null);
});
