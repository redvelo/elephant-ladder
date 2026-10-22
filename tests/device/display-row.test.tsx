// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { DisplayRow } from "../../src/device/controls";

afterEach(() => cleanup());

test("a device in Regal+ is told it is the recommendation and what the repaint costs", () => {
  render(<DisplayRow reading={{ refreshMode: "regal_plus", speedModeWhileDragging: false }} />);
  expect(screen.getByText("Regal+")).toBeTruthy();
  expect(screen.getByText(/Regal is the mode we recommend/u)).toBeTruthy();
  expect(screen.getByText(/each time you lift your finger/u)).toBeTruthy();
  expect(
    screen.getByText(/HD is the alternative: with Elephant Ladder open, open EinkWise/u),
  ).toBeTruthy();
});

test("a device in HD is told what it gains and that Regal+ is recommended", () => {
  render(<DisplayRow reading={{ refreshMode: "hd", speedModeWhileDragging: false }} />);
  expect(screen.getByText("HD")).toBeTruthy();
  expect(screen.getByText(/refreshes only what changed/u)).toBeTruthy();
  expect(screen.getByText(/We recommend Regal\+ for the cleanest turns/u)).toBeTruthy();
});

test("a device in a mode faster than HD is asked to go back to HD", () => {
  render(<DisplayRow reading={{ refreshMode: "speed", speedModeWhileDragging: false }} />);
  expect(screen.getByText("Fast")).toBeTruthy();
  expect(screen.getByText(/Use HD at least, or Regal\+ for the cleanest turns/u)).toBeTruthy();
});

test("a device dragging in its fast waveform is told that is the recommended default", () => {
  render(<DisplayRow reading={{ refreshMode: "regal_plus", speedModeWhileDragging: true }} />);
  expect(screen.getByText("On")).toBeTruthy();
  expect(screen.getByText(/the setting we recommend/u)).toBeTruthy();
  expect(screen.getByText(/a selection shows as you extend it/u)).toBeTruthy();
});

test("a device that declined the fast waveform is told drags freeze and how to turn it back on", () => {
  render(<DisplayRow reading={{ refreshMode: "regal", speedModeWhileDragging: false }} />);
  expect(screen.getByText("Off")).toBeTruthy();
  expect(screen.getByText(/shows nothing until you let go/u)).toBeTruthy();
  expect(screen.getByText(/turn on Speed Mode's When Dragging Page/u)).toBeTruthy();
});

test("a device that reports nothing is not guessed at", () => {
  render(<DisplayRow reading={{ refreshMode: null, speedModeWhileDragging: null }} />);
  expect(screen.getByText("Not reported")).toBeTruthy();
  expect(screen.getByText(/does not report the refresh mode/u)).toBeTruthy();
  expect(screen.getByText("—")).toBeTruthy();
  expect(screen.getByText(/does not report the setting/u)).toBeTruthy();
});
