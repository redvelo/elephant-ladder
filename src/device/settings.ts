import { useAtomValue } from "jotai";
import { frontLightDialogEnabledAtom } from "./front-light";
import { bootstrapAtom } from "../app/session";
import { settingsAtom } from "../settings/state";

/**
 * Which device controls this device and session can actually offer. Side buttons are writable
 * only from the library document, which owns the application settings.
 */
export function useDeviceSettings(): DeviceControls {
  const bootstrap = useAtomValue(bootstrapAtom);
  const settings = useAtomValue(settingsAtom);
  return {
    frontLight: useAtomValue(frontLightDialogEnabledAtom),
    sideButtons:
      bootstrap?.mode === "library" && bootstrap.nativeActions.sideButtons && settings !== null,
    display:
      bootstrap?.display.technology === "eink"
        ? {
            refreshMode: bootstrap.display.refreshMode ?? null,
            speedModeWhileDragging: bootstrap.display.speedModeWhileDragging ?? null,
          }
        : null,
  };
}

/** Which device controls exist here, and so whether there is a Device page to open at all. */
export interface DeviceControls {
  readonly frontLight: boolean;
  readonly sideButtons: boolean;
  readonly display: DisplayReading | null;
}

/** What the e-ink device reports about itself: facts to show, never settings to change here. */
export interface DisplayReading {
  readonly refreshMode: string | null;
  readonly speedModeWhileDragging: boolean | null;
}
