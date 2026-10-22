import { useEffect, useState } from "react";
import { AppDialog } from "../ui/dialog";
import { describeError } from "../host/requests";

type Notices = { readonly text: string } | { readonly failure: string };

async function readNotices(signal: AbortSignal): Promise<Notices> {
  try {
    const response = await fetch("/THIRD_PARTY_NOTICES.md", { cache: "no-store", signal });
    if (!response.ok) throw new Error(`The notices are unavailable (HTTP ${response.status}).`);
    return { text: await response.text() };
  } catch (cause) {
    return { failure: describeError(cause) };
  }
}

/** The notices this build ships with, read from the bundle rather than restated here. */
export function NoticesDialog({
  container,
  returnFocus,
  onClose,
}: {
  container: HTMLElement | null;
  returnFocus: HTMLElement | null;
  onClose: () => void;
}) {
  const [notices, setNotices] = useState<Notices | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    void readNotices(controller.signal).then((read) => {
      if (!controller.signal.aborted) setNotices(read);
    });
    return () => controller.abort();
  }, []);

  return (
    <AppDialog
      title="Open-source notices"
      pageLabel="Notices"
      description="The software and books this application is built from."
      container={container}
      returnFocus={returnFocus}
      onClose={onClose}
    >
      {notices === null ? <p role="status">Loading…</p> : null}
      {notices !== null && "failure" in notices ? <p role="alert">{notices.failure}</p> : null}
      {notices !== null && "text" in notices ? (
        <pre className="app-notices">{notices.text}</pre>
      ) : null}
    </AppDialog>
  );
}
