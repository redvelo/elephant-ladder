import { requireCurrentFrame } from "./frames";
import type { RunnerCheck } from "./reports";

export function evaluateHostBoundary(
  container: HTMLElement,
  publicationOrigin: string,
  agentMessageOrigins: ReadonlySet<string>,
): RunnerCheck[] {
  const frame = requireCurrentFrame(container);
  const expectedUrl = `${publicationOrigin}/EPUB/text/peer.xhtml`;
  const agentOrigins = [...agentMessageOrigins];
  return [
    { label: "canonical-frame-url", passed: frame.src === expectedUrl, detail: frame.src },
    {
      label: "exact-frame-sandbox",
      passed: frame.getAttribute("sandbox") === "allow-scripts allow-same-origin",
      detail: String(frame.getAttribute("sandbox")),
    },
    {
      label: "parent-dom-isolation",
      passed: frame.contentDocument === null,
      detail: String(frame.contentDocument !== null),
    },
    {
      label: "exact-agent-message-origin",
      passed:
        agentOrigins.length > 0 && agentOrigins.every((origin) => origin === publicationOrigin),
      detail: JSON.stringify(agentOrigins),
    },
  ];
}
