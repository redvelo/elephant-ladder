import { NavigatorReadingSettings } from "epub-navigator/ui";
import { useAtomValue, useSetAtom } from "jotai";
import { AppDialog } from "../ui/dialog";
import { bootstrapAtom } from "../app/session";
import { ResetReadingSettingsRow } from "./reading-reset";
import { saveSettingsAtom, settingsAtom } from "./state";

/** Every reading preference on one page, which is where the bulk of the settings live. */
export function ReadingSettingsDialog({
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
  const bootstrap = useAtomValue(bootstrapAtom);
  const settings = useAtomValue(settingsAtom);
  const save = useSetAtom(saveSettingsAtom);
  if (settings === null) return null;
  return (
    <AppDialog
      title="Reading"
      description="Appearance and interaction apply to every book. Layout applies only to books you have not already given their own."
      pageLabel="Reading settings"
      container={container}
      returnFocus={returnFocus}
      onClose={onClose}
    >
      <div className="settings-reading-panel">
        <fieldset className="reading-defaults" disabled={disabled}>
          <NavigatorReadingSettings
            paginated
            preferences={settings.preferences}
            readerTypefaces={bootstrap?.readerTypefaces ?? []}
            setPreferences={(update) => {
              const preferences =
                typeof update === "function" ? update(settings.preferences) : update;
              save({ preferences });
            }}
          />
        </fieldset>
        <ResetReadingSettingsRow disabled={disabled} portalContainer={container} />
      </div>
    </AppDialog>
  );
}
