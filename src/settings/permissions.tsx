import { Button, NavigatorSettingsChoice } from "epub-navigator/ui";
import { useAtomValue, useSetAtom } from "jotai";
import { AppDialog } from "../ui/dialog";
import {
  appPolicyAtom,
  appPolicyErrorAtom,
  loadAppPolicyAtom,
  saveAppPolicyAtom,
  type AppPolicyState,
} from "../policy/app-policy";

/** The defaults for what any book may do, before a book is answered for individually. */
export function BookPermissionsDialog({
  disabled,
  container,
  returnFocus,
  onClose,
}: {
  readonly disabled: boolean;
  readonly container: HTMLElement | null;
  readonly returnFocus: HTMLElement | null;
  readonly onClose: () => void;
}) {
  const view = useAtomValue(appPolicyAtom);
  const error = useAtomValue(appPolicyErrorAtom);
  const save = useSetAtom(saveAppPolicyAtom);
  const loadAppPolicy = useSetAtom(loadAppPolicyAtom);
  return (
    <AppDialog
      title="Book permissions"
      description="An EPUB can carry scripts and can try to load files from the internet. These are the defaults for every book; a book you have answered for keeps its own answer, and changes apply the next time you open a book."
      pageLabel="Book permissions"
      container={container}
      returnFocus={returnFocus}
      onClose={onClose}
    >
      {error !== null ? (
        <p className="settings-defaults-description" role="alert">
          {error}
        </p>
      ) : null}
      <PermissionChoices
        view={view}
        disabled={disabled}
        onChange={(change) => void save(change)}
        onRetry={loadAppPolicy}
      />
    </AppDialog>
  );
}

function PermissionChoices({
  view,
  disabled,
  onChange,
  onRetry,
}: {
  view: AppPolicyState;
  disabled: boolean;
  onChange: (change: {
    activeContent?: "automatic" | "ask" | "disabled";
    remoteResources?: "allowed" | "ask" | "blocked";
  }) => void;
  onRetry: () => void;
}) {
  if (view.status === "unavailable") {
    return (
      <div className="settings-row" role="alert">
        <span>{view.message}</span>
        <Button variant="outline" onClick={onRetry}>
          Retry
        </Button>
      </div>
    );
  }
  if (view.status !== "ready") {
    return (
      <p role="status" className="settings-measuring">
        Loading…
      </p>
    );
  }
  return (
    <div className="settings-permission-fields">
      <NavigatorSettingsChoice
        id="settings-active-content"
        label="Active content"
        value={view.policy.activeContent}
        options={[
          { value: "automatic", label: "Automatic" },
          { value: "ask", label: "Ask" },
          { value: "disabled", label: "Disabled" },
        ]}
        description={activeContentDescription(view.policy.activeContent)}
        disabled={disabled}
        onChange={(activeContent) => onChange({ activeContent })}
      />
      <NavigatorSettingsChoice
        id="settings-network-access"
        label="Network access"
        value={view.policy.remoteResources}
        options={[
          { value: "allowed", label: "Allow" },
          { value: "ask", label: "Ask" },
          { value: "blocked", label: "Block" },
        ]}
        description={networkDescription(view.policy.remoteResources)}
        disabled={disabled}
        onChange={(remoteResources) => onChange({ remoteResources })}
      />
    </div>
  );
}

function activeContentDescription(value: "automatic" | "ask" | "disabled"): string {
  if (value === "automatic")
    return "Let a book's scripts run, unless the book could not be fully checked.";
  if (value === "ask") return "Ask the first time a book that may contain scripts is opened.";
  return "Never run a book's scripts.";
}

function networkDescription(value: "allowed" | "ask" | "blocked"): string {
  if (value === "allowed")
    return "Let books load images, fonts, and other files over HTTPS. Plain HTTP stays blocked.";
  if (value === "ask") return "Ask the first time a book that may reach the network is opened.";
  return "Never let a book reach the network.";
}
