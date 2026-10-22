import type { KernelEvent, KernelTransition } from "epub-navigator";
import type { NavigatorRuntime } from "epub-navigator/runtime";

export function requirePendingFrame(container: HTMLElement): HTMLIFrameElement {
  const frame = container.querySelector<HTMLIFrameElement>(
    'iframe[data-epub-navigator-participant="pending-primary"]',
  );
  if (frame === null) throw new Error("Navigator did not create the expected pending iframe");
  return frame;
}

export function requireCurrentFrame(container: HTMLElement): HTMLIFrameElement {
  const frame = container.querySelector<HTMLIFrameElement>(
    'iframe[data-epub-navigator-participant="primary"]',
  );
  if (frame === null) throw new Error("Navigator did not promote the expected iframe");
  return frame;
}

export function openedGeneration(transition: KernelTransition): number {
  const pending = transition.events.find((event) => event.type === "document-pending");
  if (pending === undefined) throw new Error("Navigator emitted no pending document");
  return pending.generation;
}

export async function waitForCommit(
  runtime: NavigatorRuntime,
  generation: number,
  errors: readonly string[],
  events: readonly KernelEvent[],
): Promise<void> {
  const deadline = performance.now() + 10_000;
  const seen = events.length;
  while (performance.now() < deadline) {
    if (runtime.snapshot().document.committed?.generation === generation) return;
    if (errors.length > 0) throw new Error(errors.join("; "));
    const failed = events
      .slice(seen)
      .find((event) => event.type === "document-load-failed" && event.generation === generation);
    if (failed !== undefined) {
      throw new Error(
        `Document generation ${generation} failed to load: ${JSON.stringify(failed)}`,
      );
    }
    await new Promise((resolve) => window.setTimeout(resolve, 16));
  }
  throw new Error(
    `Document generation ${generation} did not commit: ${JSON.stringify(events.slice(-16))}`,
  );
}

export function captureGeometry() {
  const frames = document.querySelectorAll<HTMLIFrameElement>(
    '.fixture-runner-frame iframe[data-epub-navigator-participant="primary"], .fixture-runner-frame iframe[data-epub-navigator-participant="fixed-companion"]',
  );
  if (frames.length === 0) return null;
  const rects = Array.from(frames, (frame) => frame.getBoundingClientRect());
  const rect = {
    x: Math.min(...rects.map((r) => r.x)),
    y: Math.min(...rects.map((r) => r.y)),
    right: Math.max(...rects.map((r) => r.right)),
    bottom: Math.max(...rects.map((r) => r.bottom)),
  };
  const x = Math.min(window.innerWidth - 1, Math.max(0, Math.floor(rect.x)));
  const y = Math.min(window.innerHeight - 1, Math.max(0, Math.floor(rect.y)));
  return {
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
    x,
    y,
    width: Math.max(1, Math.min(window.innerWidth, Math.ceil(rect.right)) - x),
    height: Math.max(1, Math.min(window.innerHeight, Math.ceil(rect.bottom)) - y),
  };
}

export async function waitForPaint(): Promise<void> {
  await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
  await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
}
