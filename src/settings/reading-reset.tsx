import { Button } from "epub-navigator/ui";
import { useAtomValue, useSetAtom } from "jotai";
import { useState } from "react";
import { AppDialog } from "../ui/dialog";
import { resetSettingsAtom, settingsResetPendingAtom } from "./state";

/** Reset is an explicit, confirmed action: it discards every reading choice at once. */
export function ResetReadingSettingsRow({
  disabled,
  portalContainer,
}: {
  disabled: boolean;
  portalContainer: HTMLElement | null;
}) {
  const reset = useSetAtom(resetSettingsAtom);
  const pending = useAtomValue(settingsResetPendingAtom);
  const [confirming, setConfirming] = useState<HTMLElement | null>(null);
  return (
    <>
      {confirming !== null ? (
        <AppDialog
          kind="decision"
          title="Reset reading settings?"
          description="Appearance, layout, and interaction return to the application defaults. Books keep their own layout, their reading positions, and their annotations."
          container={portalContainer}
          returnFocus={confirming}
          onClose={() => setConfirming(null)}
          actions={
            <>
              <Button variant="outline" disabled={pending} onClick={() => setConfirming(null)}>
                Cancel
              </Button>
              <Button
                disabled={pending}
                onClick={() => {
                  void reset().then(() => setConfirming(null));
                }}
              >
                {pending ? "Resetting…" : "Reset"}
              </Button>
            </>
          }
        />
      ) : null}
      <div className="settings-row">
        <span>
          <strong>Reset reading settings</strong>
          <small>Return appearance, layout, and interaction to the defaults.</small>
        </span>
        <Button
          variant="outline"
          disabled={disabled || pending}
          onClick={(event) => setConfirming(event.currentTarget)}
        >
          Reset…
        </Button>
      </div>
    </>
  );
}
