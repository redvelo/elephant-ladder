import { Button, NavigatorSettingsChoice } from "epub-navigator/ui";
import { AppDialog } from "../ui/dialog";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useId } from "react";
import {
  closePermissionsAtom,
  editPermissionsAtom,
  loadPermissionsAtom,
  permissionsReviewAtom,
  resetPermissionsAtom,
  savePermissionsAtom,
} from "./permissions-state";

const assessmentLabels = {
  activeContent: {
    detected: "Active content detected.",
    "not-detected": "No active content detected.",
    unknown: "Could not fully assess active content.",
  },
  remoteResources: {
    detected: "Network use detected.",
    "not-detected": "No network use detected.",
    unknown: "Could not fully assess network use.",
  },
};

const activeContentItems = [
  { value: "default", label: "App default" },
  { value: "enabled", label: "Allow" },
  { value: "disabled", label: "Disable" },
] as const;
const remoteResourceItems = [
  { value: "default", label: "App default" },
  { value: "allowed", label: "Allow" },
  { value: "blocked", label: "Block" },
] as const;

export function PublicationSiteDataDialog({
  publication,
  portalContainer,
  returnFocus,
  onClose,
  onResetWebsiteData,
}: {
  readonly publication: { readonly publicationId: string; readonly title: string };
  readonly portalContainer: HTMLElement | null;
  readonly returnFocus: HTMLElement | null;
  readonly onClose: () => void;
  readonly onResetWebsiteData: () => void;
}) {
  const view = useAtomValue(permissionsReviewAtom);
  const loadPermissions = useSetAtom(loadPermissionsAtom);
  const closePermissions = useSetAtom(closePermissionsAtom);
  const edit = useSetAtom(editPermissionsAtom);
  const resetPermissions = useSetAtom(resetPermissionsAtom);
  const savePermissions = useSetAtom(savePermissionsAtom);
  const id = useId();
  const { publicationId } = publication;

  useEffect(() => {
    const stop = loadPermissions(publicationId);
    return () => {
      stop();
      closePermissions();
    };
  }, [publicationId, loadPermissions, closePermissions]);

  const save = async (): Promise<void> => {
    if (await savePermissions(publicationId)) loadPermissions(publicationId);
  };

  const loaded = view.status === "ready" || view.status === "saving" ? view : null;
  const draft = loaded === null ? null : { ...loaded.stored.policy.publication, ...loaded.changes };
  const activeDefault =
    loaded === null
      ? ""
      : {
          automatic: "Automatic",
          ask: "Ask",
          disabled: "Disabled",
        }[loaded.stored.policy.global.activeContent];
  const networkDefault =
    loaded === null
      ? ""
      : {
          allowed: "Allowed",
          ask: "Ask",
          blocked: "Blocked",
        }[loaded.stored.policy.global.remoteResources];
  const saving = view.status === "saving";

  return (
    <AppDialog
      kicker="This book only"
      title="Book permissions"
      description={`What "${publication.title}" may do, and the browser data it has stored. Saved changes apply the next time you open this book.`}
      pageLabel="Book permissions"
      container={portalContainer}
      returnFocus={returnFocus}
      onClose={() => {
        if (!saving) onClose();
      }}
      actions={
        <>
          <Button
            type="button"
            variant="outline"
            disabled={saving}
            onClick={() => {
              if (!saving) onClose();
            }}
          >
            {view.notice === null ? "Cancel" : "Close"}
          </Button>
          <Button
            type="submit"
            form="publication-permissions-form"
            disabled={view.status !== "ready" || Object.keys(view.changes).length === 0}
          >
            {saving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {view.notice === "unconfirmed" ? (
        <p className="publication-permissions-message" role="alert">
          The save could not be confirmed. It may have reached the device.
          {view.status === "ready"
            ? " Current saved permissions are shown; review them before trying again."
            : " Reload saved permissions before editing again. Closing does not undo a save."}
        </p>
      ) : null}
      {view.status === "loading" ? (
        <p className="publication-permissions-message" role="status">
          Loading saved permissions…
        </p>
      ) : view.status === "unavailable" ? (
        <div className="publication-permissions-message">
          <p role="alert">{view.error}</p>
          <Button variant="outline" onClick={() => loadPermissions(publicationId)}>
            Retry loading
          </Button>
        </div>
      ) : null}
      <form
        id="publication-permissions-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
        aria-busy={saving}
      >
        {draft === null || loaded === null ? null : (
          <div className="publication-permissions-fields">
            <div className="publication-permissions-setting">
              <NavigatorSettingsChoice
                id={`${id}-active`}
                label="Active content"
                value={draft.activeContent ?? "default"}
                options={activeContentItems}
                disabled={saving}
                onChange={(value) => {
                  edit({ activeContent: value === "default" ? null : value });
                }}
              />
              <p id={`${id}-active-hint`}>
                <span data-permission-assessment="activeContent">
                  Assessment:{" "}
                  {assessmentLabels.activeContent[loaded.stored.relevance.activeContent]}
                </span>
                <br />
                App default: {activeDefault}.<br />
                On next open:{" "}
                {draft.activeContent === null
                  ? activeDefault
                  : draft.activeContent === "enabled"
                    ? "Allowed"
                    : "Disabled"}
                .
              </p>
            </div>
            <div className="publication-permissions-setting">
              <NavigatorSettingsChoice
                id={`${id}-network`}
                label="Network access"
                value={draft.remoteResources ?? "default"}
                options={remoteResourceItems}
                disabled={saving}
                onChange={(value) => {
                  edit({ remoteResources: value === "default" ? null : value });
                }}
              />
              <p id={`${id}-network-hint`}>
                <span data-permission-assessment="remoteResources">
                  Assessment:{" "}
                  {assessmentLabels.remoteResources[loaded.stored.relevance.remoteResources]}
                </span>
                <br />
                App default: {networkDefault}.<br />
                On next open:{" "}
                {draft.remoteResources === null
                  ? networkDefault
                  : draft.remoteResources === "allowed"
                    ? "Allowed"
                    : "Blocked"}
                .
              </p>
            </div>
            <Button
              type="button"
              variant="outline"
              disabled={saving}
              onClick={() => resetPermissions()}
            >
              Reset to app defaults
            </Button>
            <p className="publication-permissions-hint">
              Reset is applied only when you save. App defaults may ask for consent again; website
              data, reading position, and annotations are not deleted.
            </p>
            <div className="publication-permissions-setting">
              <h3>Browser data</h3>
              <Button
                type="button"
                variant="outline"
                disabled={saving}
                data-publication-action="reset-website-data"
                onClick={onResetWebsiteData}
              >
                Reset browser data…
              </Button>
              <p className="publication-permissions-hint">
                The book starts with a new storage identity, so data it stored before becomes
                unreachable and is queued for removal. Reading position, annotations, and
                preferences are kept.
              </p>
            </div>
          </div>
        )}
        {view.notice === "saved" ? (
          <p className="publication-permissions-message" role="status">
            Permissions saved.
          </p>
        ) : null}
      </form>
    </AppDialog>
  );
}
