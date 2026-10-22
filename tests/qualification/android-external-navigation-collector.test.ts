import { describe, expect, test } from "vitest";

import {
  assessReturnContinuity,
  CATEGORY_BROWSABLE,
  createBrowsableIntentExpectation,
  parseActivityTaskEvidence,
  parseForegroundActivity,
} from "../../scripts/collect-android-external-navigation.mjs";

const app = "com.redvelo.elephant_ladder";
const target = createBrowsableIntentExpectation("https://example.com/qualification");

function observation(packageName: string, taskId: number, browsable = false) {
  return {
    foreground: {
      packageName,
      activityName: ".MainActivity",
      component: `${packageName}/.MainActivity`,
      taskId,
    },
    browsableIntents: browsable
      ? [
          {
            action: "android.intent.action.VIEW",
            categories: [CATEGORY_BROWSABLE],
            uri: target.uri,
            taskId: 44,
          },
        ]
      : [],
  };
}

describe("Android external navigation evidence", () => {
  test("constructs a sanitized CATEGORY_BROWSABLE expectation", () => {
    expect(
      createBrowsableIntentExpectation("https://Example.COM:8443/a/book?q=private#fragment"),
    ).toStrictEqual({
      action: "android.intent.action.VIEW",
      category: CATEGORY_BROWSABLE,
      uri: { scheme: "https", host: "example.com", port: "8443", path: "/a/book" },
    });
    expect(() => createBrowsableIntentExpectation("intent://example.com")).toThrow(
      /absolute HTTP\(S\)/,
    );
    expect(() => createBrowsableIntentExpectation("https://user@example.com/book")).toThrow(
      /without credentials/,
    );
  });

  test("parses current foreground activity across dumpsys forms", () => {
    expect(
      parseForegroundActivity(
        "topResumedActivity=ActivityRecord{abc u0 com.android.chrome/com.google.android.apps.chrome.Main t44}",
      ),
    ).toStrictEqual({
      packageName: "com.android.chrome",
      activityName: "com.google.android.apps.chrome.Main",
      component: "com.android.chrome/com.google.android.apps.chrome.Main",
      taskId: 44,
    });
    expect(
      parseForegroundActivity(
        "mResumedActivity: ActivityRecord{abc u0 com.redvelo.elephant_ladder/.MainActivity taskId=17}",
      )?.taskId,
    ).toBe(17);
    expect(parseForegroundActivity("mResumedActivity: null")).toBeNull();
  });

  test("extracts only normalized BROWSABLE VIEW task evidence", () => {
    const dump = `
      * Task{aaa #44 type=standard}
        intent={act=android.intent.action.VIEW cat=[android.intent.category.BROWSABLE] dat=https://Example.com/qualification?token=secret flg=0x10000000 cmp=com.android.chrome/.Main}
        Intent { act=android.intent.action.VIEW dat=https://ignored.example/ }
        topResumedActivity=ActivityRecord{bbb u0 com.android.chrome/.Main t44}
    `;
    expect(parseActivityTaskEvidence(dump)).toStrictEqual({
      foreground: {
        packageName: "com.android.chrome",
        activityName: ".Main",
        component: "com.android.chrome/.Main",
        taskId: 44,
      },
      browsableIntents: [
        {
          action: "android.intent.action.VIEW",
          categories: [CATEGORY_BROWSABLE],
          uri: { scheme: "https", host: "example.com", port: null, path: "/qualification" },
          taskId: 44,
          component: "com.android.chrome/.Main",
        },
      ],
    });
  });

  test("passes only an ordered app to external to same-task app return", () => {
    const complete = [
      observation(app, 17),
      observation("com.android.intentresolver", 44, true),
      observation(app, 17),
    ];
    expect(assessReturnContinuity(complete, target)).toMatchObject({
      status: "pass",
      checks: {
        appInitiallyForeground: true,
        matchingBrowsableIntent: true,
        externalForeground: true,
        appReturned: true,
        sameAppTask: true,
      },
    });
    expect(assessReturnContinuity(complete.slice(0, 2), target).status).toBe("incomplete");
    expect(
      assessReturnContinuity(
        [observation(app, 17), observation("com.android.chrome", 44, true), observation(app, 18)],
        target,
      ).status,
    ).toBe("incomplete");
    expect(
      assessReturnContinuity(
        [observation(app, 17), observation("com.android.chrome", 44), observation(app, 17)],
        target,
      ).status,
    ).toBe("incomplete");
  });
});
