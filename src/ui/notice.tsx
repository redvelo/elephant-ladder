import { Button as BaseButton } from "@base-ui/react/button";
import { useEffect, useRef, type ReactNode } from "react";

export type NoticeTone = "progress" | "done" | "failed";

/** How long a finished message stays before it clears itself. A low-refresh display holds it
    longer, because clearing it costs a refresh there. */
const linger = { standard: 5000, minimal: 9000 };

/** One message in the layer above the navigation. Work in progress and failures stay until
    they resolve or are dismissed; a completed message clears itself. */
export function AppNotice({
  tone,
  pace,
  onDismiss,
  action,
  children,
  ...rest
}: {
  readonly tone: NoticeTone;
  readonly pace: "standard" | "minimal";
  readonly onDismiss?: (() => void) | undefined;
  readonly action?: ReactNode;
  readonly children: ReactNode;
  readonly "data-website-data-reset-status"?: string;
  readonly "data-physical-purge"?: string;
}) {
  const dismiss = useRef(onDismiss);
  useEffect(() => {
    dismiss.current = onDismiss;
  }, [onDismiss]);
  useEffect(() => {
    if (tone !== "done") return undefined;
    const timer = window.setTimeout(() => dismiss.current?.(), linger[pace]);
    return () => window.clearTimeout(timer);
  }, [tone, pace]);
  return (
    <p
      className="app-notice"
      data-tone={tone}
      role={tone === "failed" ? "alert" : "status"}
      {...rest}
    >
      <span>{children}</span>
      {action ??
        (tone === "failed" && onDismiss !== undefined ? (
          <BaseButton className="text-control" onClick={onDismiss}>
            Dismiss
          </BaseButton>
        ) : null)}
    </p>
  );
}
