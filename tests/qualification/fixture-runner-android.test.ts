import { expect, test } from "vitest";

import {
  ANDROID_FIXTURE_ID_EXTRA,
  androidFixtureLaunchArguments,
  parseWebViewPackage,
  relativeAndroidFixtureArtifact,
} from "../../scripts/collect-fixture-runner-android.mjs";
import { FIXTURE_IDS, parseFixtureIds } from "../../scripts/fixture-runner-contract.mjs";

test("Android fixture defaults select the complete integrated catalog", () => {
  expect(parseFixtureIds([])).toStrictEqual([...FIXTURE_IDS]);
  expect(parseFixtureIds(["active-probe"])).toStrictEqual(["active-probe"]);
  expect(() => parseFixtureIds(["unknown"])).toThrow(/Unknown fixture id/);
});

test("Android fixture launch passes one exact allowlisted selection", () => {
  expect(androidFixtureLaunchArguments("active-probe")).toStrictEqual([
    "--es",
    ANDROID_FIXTURE_ID_EXTRA,
    "active-probe",
  ]);
  expect(() => androidFixtureLaunchArguments(" active-probe")).toThrow(/Unknown fixture id/);
  expect(() => androidFixtureLaunchArguments("../active-probe")).toThrow(/Unknown fixture id/);
});

test("Android child evidence resolves the suite-level fixture APK", () => {
  const artifact = { file: "fixture-runner.apk", sha256: "fixture-digest" };
  expect(
    relativeAndroidFixtureArtifact(artifact, "/evidence/android/active-probe", "/evidence/android"),
  ).toStrictEqual({ ...artifact, file: "../fixture-runner.apk" });
  expect(
    relativeAndroidFixtureArtifact(
      artifact,
      "/evidence/android/relations/storage-isolation/primary-first",
      "/evidence/android",
    ),
  ).toStrictEqual({ ...artifact, file: "../../../fixture-runner.apk" });
});

test("Android WebView evidence parser omits unrelated dumpsys content", () => {
  expect(
    parseWebViewPackage(
      "Current WebView package (name, version): (com.google.android.webview, 146.0.7680.177)",
    ),
  ).toStrictEqual({ package: "com.google.android.webview", version: "146.0.7680.177" });
  expect(parseWebViewPackage("Current WebView package is unavailable")).toStrictEqual({
    package: "unavailable",
    version: "unavailable",
  });
});

test("Android corpus launch names the staged publication and plan by package path", async () => {
  const { androidCorpusLaunchArguments } =
    await import("../../scripts/collect-w3c-corpus-android.mjs");
  expect(
    androidCorpusLaunchArguments(
      "/data/user/0/com.redvelo.elephant_ladder.fixture_runner/corpus/scr-support.epub",
      "/data/user/0/com.redvelo.elephant_ladder.fixture_runner/corpus/scr-support.observation.json",
    ),
  ).toStrictEqual([
    "--es",
    "com.redvelo.elephant_ladder.fixture_runner.FIXTURE_EPUB",
    "/data/user/0/com.redvelo.elephant_ladder.fixture_runner/corpus/scr-support.epub",
    "--es",
    "com.redvelo.elephant_ladder.fixture_runner.FIXTURE_OBSERVATION",
    "/data/user/0/com.redvelo.elephant_ladder.fixture_runner/corpus/scr-support.observation.json",
  ]);
});
