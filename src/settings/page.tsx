import { Button, NavigatorPaginatedOverflow } from "epub-navigator/ui";
import { Button as BaseButton } from "@base-ui/react/button";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useState } from "react";
import { DeviceSettingsDialog } from "../device/controls";
import { useDeviceSettings } from "../device/settings";
import { useHoverPointer } from "../lib/pointer";
import { loadStorageUsageAtom, storageUsageAtom } from "./storage-usage";
import { loadAppPolicyAtom } from "../policy/app-policy";
import { BookPermissionsDialog } from "./permissions";
import { ReadingSettingsDialog } from "./reading-dialog";
import { StorageUsageRows } from "./storage-usage-rows";
import { bootstrapAtom } from "../app/session";
import { dictionariesViewAtom } from "../dictionaries/state";
import { loadSettingsAtom, settingsAtom, settingsErrorAtom } from "./state";
import "./settings.css";

/** The one settings page the index has open. Like the shell's surfaces, they never stack. */
type SettingsSection = "reading" | "device" | "permissions";

export function SettingsPage({
  disabled,
  portalContainer,
  onDictionaries,
  onNotices,
  onShortcuts,
}: {
  disabled: boolean;
  portalContainer: HTMLElement | null;
  onDictionaries: (trigger: HTMLElement) => void;
  onNotices: (trigger: HTMLElement) => void;
  onShortcuts: (trigger: HTMLElement) => void;
}) {
  const bootstrap = useAtomValue(bootstrapAtom);
  const settings = useAtomValue(settingsAtom);
  const error = useAtomValue(settingsErrorAtom);
  const reload = useSetAtom(loadSettingsAtom);
  const dictionaries = useAtomValue(dictionariesViewAtom);
  const device = useDeviceSettings();
  const usage = useAtomValue(storageUsageAtom);
  const loadStorageUsage = useSetAtom(loadStorageUsageAtom);
  const loadAppPolicy = useSetAtom(loadAppPolicyAtom);
  const keyboardReachable = useHoverPointer();
  const [section, setSection] = useState<{
    kind: SettingsSection;
    trigger: HTMLElement | null;
  } | null>(null);
  const application = bootstrap?.mode === "library" ? bootstrap.application : undefined;
  // Both answers come from ledgers the application already keeps, so they are read on open.
  useEffect(() => {
    void loadStorageUsage();
  }, [loadStorageUsage]);
  useEffect(() => loadAppPolicy(), [loadAppPolicy]);
  const count =
    dictionaries.status === "ready"
      ? dictionaries.installations.length
      : bootstrap?.mode === "library"
        ? bootstrap.dictionaries.installed
        : 0;
  const deviceParts = [
    device.frontLight ? "front light" : null,
    device.sideButtons ? "page buttons" : null,
    device.display !== null ? "display refresh" : null,
  ]
    .filter((part) => part !== null)
    .join(", ");
  const open = (kind: SettingsSection) => (trigger: HTMLElement) => setSection({ kind, trigger });
  return (
    <section className="settings-page" aria-labelledby="settings-title">
      <h1 id="settings-title">Settings</h1>
      {section?.kind === "reading" ? (
        <ReadingSettingsDialog
          disabled={disabled}
          container={portalContainer}
          returnFocus={section.trigger}
          onClose={() => setSection(null)}
        />
      ) : null}
      {section?.kind === "device" ? (
        <DeviceSettingsDialog
          controls={device}
          disabled={disabled}
          container={portalContainer}
          returnFocus={section.trigger}
          onClose={() => setSection(null)}
        />
      ) : null}
      {section?.kind === "permissions" ? (
        <BookPermissionsDialog
          disabled={disabled}
          container={portalContainer}
          returnFocus={section.trigger}
          onClose={() => setSection(null)}
        />
      ) : null}
      <NavigatorPaginatedOverflow pageLabel="Settings" className="settings-viewport" deviceTurns>
        {error !== null ? (
          <div className="settings-error" role="alert">
            {error}
            {settings === null ? (
              <Button variant="outline" onClick={() => reload()}>
                Retry
              </Button>
            ) : null}
          </div>
        ) : null}
        {settings === null && error === null ? <p role="status">Loading settings…</p> : null}
        {/* The index: each row names a page, so the list itself stays short enough to see. */}
        <div className="settings-section">
          {settings !== null ? (
            <SettingsSectionLink
              label="Reading"
              detail="Typefaces, text size, layout, and how pages turn."
              onOpen={open("reading")}
            />
          ) : null}
          {keyboardReachable ? (
            <SettingsSectionLink
              label="Keyboard shortcuts"
              detail="Navigate the app and your library with a keyboard."
              keyShortcut="?"
              onOpen={onShortcuts}
            />
          ) : null}
          <SettingsSectionLink
            label="Dictionaries"
            detail="Look words up without a network."
            value={`${count} installed`}
            disabled={disabled}
            onOpen={onDictionaries}
          />
          {deviceParts === "" ? null : (
            <SettingsSectionLink
              label="Device"
              detail={`${deviceParts.replace(/^./u, (first) => first.toUpperCase())}.`}
              onOpen={open("device")}
            />
          )}
          <SettingsSectionLink
            label="Book permissions"
            detail="What a book may run, and what it may load."
            disabled={disabled}
            onOpen={open("permissions")}
          />
        </div>
        <section className="settings-section" aria-labelledby="storage-title">
          <h2 id="storage-title">Storage</h2>
          <StorageUsageRows usage={usage} onRetry={() => void loadStorageUsage()} />
        </section>
        <section className="settings-section" aria-labelledby="about-title">
          <h2 id="about-title">About</h2>
          <div className="settings-row">
            <span>
              <strong>{application?.name ?? "Elephant Ladder"}</strong>
              <small>Reads EPUB publications on this device.</small>
            </span>
            <span>{application === undefined ? "" : `Version ${application.version}`}</span>
          </div>
          <BaseButton
            className="settings-row settings-link"
            disabled={disabled}
            onClick={(event) => onNotices(event.currentTarget)}
          >
            <span>
              <strong>Open-source notices</strong>
              <small>The software and books this application includes.</small>
            </span>
            <span aria-hidden="true">→</span>
          </BaseButton>
        </section>
      </NavigatorPaginatedOverflow>
    </section>
  );
}

function SettingsSectionLink({
  label,
  detail,
  value,
  keyShortcut,
  disabled = false,
  onOpen,
}: {
  readonly label: string;
  readonly detail: string;
  readonly value?: string | undefined;
  readonly keyShortcut?: string | undefined;
  readonly disabled?: boolean | undefined;
  readonly onOpen: (trigger: HTMLElement) => void;
}) {
  return (
    <BaseButton
      className="settings-row settings-link"
      disabled={disabled}
      aria-keyshortcuts={keyShortcut}
      onClick={(event) => onOpen(event.currentTarget)}
    >
      <span>
        <strong>{label}</strong>
        <small>{detail}</small>
      </span>
      <span>
        {value === undefined ? null : `${value} `}
        <span aria-hidden="true">→</span>
      </span>
    </BaseButton>
  );
}
