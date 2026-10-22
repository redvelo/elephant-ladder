import { Button } from "epub-navigator/ui";
import { formatBytes } from "../lib/format";
import type { StorageUsageState } from "./storage-usage";

/** What storage is spending, from the ledgers that already record it. */
export function StorageUsageRows({
  usage,
  onRetry,
}: {
  usage: StorageUsageState;
  onRetry: () => void;
}) {
  if (usage.status === "failed") {
    return (
      <div className="settings-row" role="alert">
        <span>{usage.message}</span>
        <Button variant="outline" onClick={onRetry}>
          Retry
        </Button>
      </div>
    );
  }
  if (usage.status !== "ready") {
    return (
      <p role="status" className="settings-measuring">
        Loading…
      </p>
    );
  }
  const rows = [
    {
      label: "Books",
      detail: "The EPUB files in your library.",
      bytes: usage.usage.booksBytes,
    },
    {
      label: "Dictionaries",
      detail: "Installed packs and their audio.",
      bytes: usage.usage.dictionariesBytes,
    },
  ];
  return rows.map((row) => (
    <div key={row.label} className="settings-row">
      <span>
        <strong>{row.label}</strong>
        <small>{row.detail}</small>
      </span>
      <span>{formatBytes(row.bytes)}</span>
    </div>
  ));
}
