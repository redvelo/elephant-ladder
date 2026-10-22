import { expect, test } from "vitest";
import {
  countBackgroundMutes,
  mediaSessionMetadata,
  mediaSessionState,
  narrationServiceForeground,
  playerStarted,
} from "../../scripts/collect-android-media-overlays.mjs";

test("counts only background mutes applied to the product application", () => {
  const audio = [
    "09-15 18:03:21:187 AudioHardening background playback muted for com.redvelo.elephant_ladder (10010), level: partial, reason: 6, usage: USAGE_MEDIA",
    "09-15 18:21:30:922 AudioHardening focus request for req 1 ignored for com.redvelo.elephant_ladder (10010), level: partial usage: 1 exemption: 6",
    "09-15 18:22:00:000 AudioHardening background playback muted for com.redvelo.elephant_ladder_fixture (10011), level: partial, reason: 6, usage: USAGE_MEDIA",
  ].join("\n");
  expect(countBackgroundMutes(audio)).toBe(1);
});

test("requires the narration service to be a foreground media playback service", () => {
  const record = (foreground: string) =>
    [
      "  * ServiceRecord{366c2f8 u0 com.redvelo.elephant_ladder/org.chromium.content.app.SandboxedProcessService0:0 c:com.redvelo.elephant_ladder}",
      "    isForeground=false foregroundId=0",
      "  * ServiceRecord{7c95e1a u0 com.redvelo.elephant_ladder/.NarrationService c:com.redvelo.elephant_ladder}",
      `    ${foreground}`,
    ].join("\n");
  expect(
    narrationServiceForeground(record("isForeground=true foregroundId=7301 types=0x00000002")),
  ).toBe(true);
  expect(
    narrationServiceForeground(record("isForeground=false foregroundId=0 types=0x00000000")),
  ).toBe(false);
  expect(narrationServiceForeground("")).toBe(false);
});

test("reads the product media session and audio player state", () => {
  const sessions = [
    "    NarrationService com.redvelo.elephant_ladder/NarrationService (userId=0)",
    "      package=com.redvelo.elephant_ladder",
    "      active=true",
    "      state=PlaybackState {state=PAUSED(2), position=-1, buffered position=0, speed=1.0}",
  ].join("\n");
  expect(mediaSessionState(sessions)).toBe("PAUSED");
  expect(mediaSessionState("package=com.example.other")).toBeNull();
  const players =
    "  AudioPlaybackConfiguration piid:2151 deviceIds:[3] type:AAudio u/pid:10010/31126 state:started attr:AudioAttributes";
  expect(playerStarted(players, "10010")).toBe(true);
  expect(playerStarted(players, "1001")).toBe(false);
});

test("reads section and publication labels from the media session metadata", () => {
  const dump = [
    "      package=com.redvelo.elephant_ladder",
    "      state=PlaybackState {state=PAUSED(2), position=34421, speed=0.0}",
    "      metadata: size=4, description=Chapter 2. The Carpet-Bag., Moby-Dick, Moby-Dick",
  ].join("\n");
  expect(mediaSessionMetadata(dump)).toEqual({
    size: 4,
    fields: ["Chapter 2. The Carpet-Bag.", "Moby-Dick", "Moby-Dick"],
  });
  expect(mediaSessionMetadata("package=com.example.other")).toBeNull();
});
