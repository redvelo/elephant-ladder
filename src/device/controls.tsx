import { Switch } from "@base-ui/react/switch";
import { useAtomValue, useSetAtom } from "jotai";
import { AppDialog } from "../ui/dialog";
import { FrontLightButton } from "./front-light-button";
import {
  frontLightDialogErrorAtom,
  frontLightDialogPendingAtom,
  showFrontLightDialogAtom,
} from "./front-light";
import { saveSettingsAtom, settingsAtom } from "../settings/state";
import type { DeviceControls, DisplayReading } from "./settings";

export function FrontLightControl() {
  const pending = useAtomValue(frontLightDialogPendingAtom);
  const error = useAtomValue(frontLightDialogErrorAtom);
  const show = useSetAtom(showFrontLightDialogAtom);
  return <FrontLightButton pending={pending} error={error} onShow={() => void show()} />;
}

/** The same control the reader offers, given the label and reason the settings page expects. */
export function FrontLightRow() {
  return (
    <div className="settings-row settings-row-action">
      <span>
        <strong>Front light</strong>
        <small>Open the device's own brightness and warmth controls.</small>
      </span>
      <FrontLightControl />
    </div>
  );
}

/** The refresh mode as the device names it, in the words its own settings use. */
function refreshModeName(alias: string | null): string {
  switch (alias) {
    case null:
      return "Not reported";
    case "regal_plus":
      return "Regal+";
    case "regal":
      return "Regal";
    case "hd":
    case "normal":
      return "HD";
    case "balanced":
      return "Balanced";
    case "speed":
      return "Fast";
    case "x":
      return "Ultrafast";
    default:
      return alias.replace(/_/g, " ").replace(/^./, (first) => first.toUpperCase());
  }
}

const einkwise = (mode: string) =>
  `with Elephant Ladder open, open EinkWise (E Ink Center on older firmware) and set Refresh Mode to ${mode}.`;

/**
 * What the mode costs here. BOOX applies Regal as a repaint of the whole window every time a finger
 * or key is released, which is invisible on a page turn and a redraw on anything else; modes faster
 * than HD trade text sharpness for a speed this app never asks for. Regal+ is the recommendation,
 * HD the floor.
 */
function refreshModeAdvice(alias: string | null): string {
  switch (alias) {
    case null:
      return "This BOOX firmware does not report the refresh mode.";
    case "regal_plus":
    case "regal":
      return `Regal is the mode we recommend: it keeps text clean on every page turn. In exchange, BOOX repaints the whole page each time you lift your finger, so selecting text or opening a menu also redraws the page. If you would rather have quiet selections and accept faint ghosting between full refreshes, HD is the alternative: ${einkwise("HD")}`;
    case "hd":
    case "normal":
      return `HD refreshes only what changed, so selecting text and opening menus are quiet and text stays sharp. Faint ghosting can build up across page turns until BOOX's periodic full refresh clears it. We recommend Regal+ for the cleanest turns, at the cost of a page redraw whenever you lift your finger: ${einkwise("Regal+")}`;
    default:
      return `Modes faster than HD trade text sharpness for speed this app does not need, since a page is drawn once per turn. Use HD at least, or Regal+ for the cleanest turns: ${einkwise("Regal+ or HD")}`;
  }
}

/**
 * What the e-ink device decided for this application, and what it costs. Both values are the
 * device's: the application reads them and cannot change them, so the row states the fact and
 * says where the switch is.
 */
export function DisplayRow({ reading }: { readonly reading: DisplayReading }) {
  const dragging = reading.speedModeWhileDragging;
  return (
    <>
      <div className="settings-row" data-refresh-mode={reading.refreshMode ?? "unknown"}>
        <span>
          <strong>Refresh mode</strong>
          <small>{refreshModeAdvice(reading.refreshMode)}</small>
        </span>
        <span>{refreshModeName(reading.refreshMode)}</span>
      </div>
      <div className="settings-row" data-speed-mode-while-dragging={dragging ?? "unknown"}>
        <span>
          <strong>Speed Mode when dragging page</strong>
          <small>
            {dragging === true
              ? "This is the BOOX default and the setting we recommend. While your finger drags, the screen keeps up in a fast waveform, so a selection shows as you extend it, and BOOX clears up with a flash about a second after you let go. A tap or the page buttons turn the page without that flash."
              : dragging === false
                ? "With this off, BOOX stops updating the screen for the length of any drag that moves up or down the page. A selection across lines shows nothing until you let go, and swiping to turn still costs a refresh. We recommend turning it back on: with Elephant Ladder open, open EinkWise (E Ink Center on older firmware) and turn on Speed Mode's When Dragging Page."
                : "This BOOX firmware does not report the setting."}
          </small>
        </span>
        <span>{dragging === true ? "On" : dragging === false ? "Off" : "—"}</span>
      </div>
    </>
  );
}

/** Everything this device itself offers, on one page of its own. */
export function DeviceSettingsDialog({
  controls,
  disabled,
  container,
  returnFocus,
  onClose,
}: {
  readonly controls: DeviceControls;
  readonly disabled: boolean;
  readonly container: HTMLElement | null;
  readonly returnFocus: HTMLElement | null;
  readonly onClose: () => void;
}) {
  return (
    <AppDialog
      title="Device"
      pageLabel="Device settings"
      container={container}
      returnFocus={returnFocus}
      onClose={onClose}
    >
      {controls.frontLight ? <FrontLightRow /> : null}
      {controls.sideButtons ? <SideButtonsRow disabled={disabled} /> : null}
      {controls.display !== null ? <DisplayRow reading={controls.display} /> : null}
    </AppDialog>
  );
}

export function SideButtonsRow({ disabled = false }: { readonly disabled?: boolean }) {
  const settings = useAtomValue(settingsAtom);
  const save = useSetAtom(saveSettingsAtom);
  return (
    <div className="settings-row">
      <span id="side-buttons-label">
        <strong>Side buttons turn pages</strong>
        <small>
          This device&rsquo;s page buttons report volume keys. Use them to turn pages instead
          &mdash; volume stays on the system controls.
        </small>
      </span>
      <Switch.Root
        className="settings-switch"
        aria-labelledby="side-buttons-label"
        checked={settings?.device.sideButtonsTurnPages ?? false}
        disabled={disabled || settings === null}
        onCheckedChange={(checked) => {
          save({ device: { sideButtonsTurnPages: checked } });
        }}
      >
        <Switch.Thumb className="settings-switch-thumb" />
      </Switch.Root>
    </div>
  );
}
