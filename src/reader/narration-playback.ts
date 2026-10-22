import type { NavigatorNarrationStatus } from "epub-navigator/react";
import { nativeRequest } from "../host/requests";

const COMMAND_RETRY_MILLISECONDS = 2_000;
const POSITION_DRIFT_MILLISECONDS = 2_000;
const MAX_LABEL_LENGTH = 200;
const MAX_COMMANDS = 16;
const TERMINAL_STATUSES = new Set([401, 403, 409, 501]);

type ReportedState = "playing" | "paused" | "stopped";
type NarrationCommand = "play" | "pause" | "next" | "previous";

interface NarrationReport {
  readonly state: ReportedState;
  readonly title: string;
  readonly section: string | null;
  readonly positionMs: number | null;
  readonly durationMs: number | null;
  readonly rate: number;
}

export interface NarrationPlayback {
  report(narration: NavigatorNarrationStatus, publicationTitle: string): void;
  close(): void;
}

export function createNarrationPlayback(
  sessionId: string,
  sessionToken: string,
  commands: {
    readonly play: () => void;
    readonly pause: () => void;
    readonly next: () => void;
    readonly previous: () => void;
  },
): NarrationPlayback {
  const controller = new AbortController();
  const authority = { sessionToken, readerSessionId: sessionId, trusted: true };
  let last: { readonly report: NarrationReport; readonly at: number } | null = null;
  let listening = false;

  const send = (report: NarrationReport): void => {
    void nativeRequest("/native/reader/narration", authority, {
      method: "POST",
      json: report,
      keepalive: report.state === "stopped",
    })
      .then((response) => {
        // A host that refuses is done with this session: aborting stops the reports and the
        // command poll together, rather than leaving a second flag to agree with.
        if (TERMINAL_STATUSES.has(response.status)) controller.abort();
      })
      .catch(() => undefined);
  };

  const listen = async (): Promise<void> => {
    listening = true;
    const following = (): boolean =>
      last !== null && last.report.state !== "stopped" && !controller.signal.aborted;
    while (following()) {
      try {
        const response = await nativeRequest("/native/reader/narration/commands", authority, {
          signal: controller.signal,
        });
        if (TERMINAL_STATUSES.has(response.status)) {
          controller.abort();
          break;
        }
        if (!response.ok) {
          await delay(COMMAND_RETRY_MILLISECONDS, controller.signal);
          continue;
        }
        const value: unknown = await response.json();
        for (const command of isCommandBatch(value) ? value.commands : []) {
          if (command === "play") commands.play();
          else if (command === "pause") commands.pause();
          else if (command === "next") commands.next();
          else commands.previous();
        }
      } catch {
        if (controller.signal.aborted) break;
        await delay(COMMAND_RETRY_MILLISECONDS, controller.signal);
      }
    }
    listening = false;
  };

  return {
    report(narration, publicationTitle) {
      if (controller.signal.aborted) return;
      const state: ReportedState =
        narration.state === "playing" || narration.state === "loading"
          ? "playing"
          : narration.state === "paused"
            ? "paused"
            : "stopped";
      if (state === "stopped") {
        if (last === null || last.report.state === "stopped") return;
        const report = stoppedReport();
        last = { report, at: Date.now() };
        send(report);
        return;
      }
      const report: NarrationReport = {
        state,
        title: publicationTitle.slice(0, MAX_LABEL_LENGTH),
        section: narration.section?.slice(0, MAX_LABEL_LENGTH) ?? null,
        positionMs: narration.timing === null ? null : Math.round(narration.timing.elapsedMs),
        durationMs: narration.timing === null ? null : Math.round(narration.timing.totalMs),
        rate: Number(narration.rate),
      };
      const now = Date.now();
      if (last !== null && sameDisplay(last.report, report) && !positionJumped(last, report, now)) {
        return;
      }
      last = { report, at: now };
      send(report);
      if (!listening) void listen();
    },
    close() {
      const refused = controller.signal.aborted;
      controller.abort();
      if (!refused && last !== null && last.report.state !== "stopped") send(stoppedReport());
      last = null;
    },
  };
}

function stoppedReport(): NarrationReport {
  return {
    state: "stopped",
    title: "",
    section: null,
    positionMs: null,
    durationMs: null,
    rate: 1,
  };
}

function sameDisplay(left: NarrationReport, right: NarrationReport): boolean {
  return (
    left.state === right.state &&
    left.title === right.title &&
    left.section === right.section &&
    left.durationMs === right.durationMs &&
    left.rate === right.rate
  );
}

// Native controls extrapolate position from the last report, so only a jump needs a refresh.
function positionJumped(
  last: { readonly report: NarrationReport; readonly at: number },
  report: NarrationReport,
  now: number,
): boolean {
  if (last.report.positionMs === null || report.positionMs === null) {
    return last.report.positionMs !== report.positionMs;
  }
  const elapsed = last.report.state === "playing" ? (now - last.at) * last.report.rate : 0;
  return (
    Math.abs(report.positionMs - (last.report.positionMs + elapsed)) > POSITION_DRIFT_MILLISECONDS
  );
}

function isCommandBatch(value: unknown): value is { commands: NarrationCommand[] } {
  if (typeof value !== "object" || value === null) return false;
  const keys = Object.keys(value);
  const commands = (value as { commands?: unknown }).commands;
  return (
    keys.length === 1 &&
    keys[0] === "commands" &&
    Array.isArray(commands) &&
    commands.length <= MAX_COMMANDS &&
    commands.every(
      (command) =>
        command === "play" || command === "pause" || command === "next" || command === "previous",
    )
  );
}

function delay(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, milliseconds);
    signal.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });
}
