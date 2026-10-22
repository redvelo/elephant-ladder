import { useAtomValue, useSetAtom } from "jotai";
import { Button, CloseIcon, NavigatorPaginatedOverflow } from "epub-navigator/ui";
import { Dialog } from "@base-ui/react/dialog";
import { settingsAtom } from "../settings/state";
import { bootstrapAtom } from "../app/session";
import { closeLibrarySurfaceAtom, librarySurfaceAtom } from "./shell-state";

type Shortcut = readonly [string, readonly string[]];

const navigation: readonly Shortcut[] = [
  ["Home", ["Ctrl/Cmd 1"]],
  ["Library", ["Ctrl/Cmd 2"]],
  ["Settings", ["Ctrl/Cmd ,"]],
  ["Import books", ["Ctrl/Cmd O"]],
  ["Search library", ["/"]],
  ["Shortcut help", ["?"]],
];
const shelf: readonly Shortcut[] = [
  ["Move between books", ["Arrows"]],
  ["Move by a shelf page", ["PgUp", "PgDn"]],
  ["First / last book", ["Home", "End"]],
];
const controls: readonly Shortcut[] = [
  ["Focus controls", ["Tab", "Shift Tab"]],
  ["Activate focused control", ["Enter", "Space"]],
  ["Dismiss active surface", ["Esc"]],
];
const vim: readonly Shortcut[] = [
  ["Move between books", ["h", "j", "k", "l"]],
  ["First / last book", ["gg", "G"]],
];

function ShortcutGroup({
  title,
  shortcuts,
}: {
  readonly title: string;
  readonly shortcuts: readonly Shortcut[];
}) {
  return (
    <section className="shortcut-group" aria-label={title}>
      <h3>{title}</h3>
      <dl>
        {shortcuts.map(([action, keys]) => (
          <div key={action}>
            <dt>{action}</dt>
            <dd className="shortcut-keys">
              {keys.map((chord, index) => (
                <span key={chord}>
                  {index > 0 ? <span>/</span> : null}
                  {chord.split(" ").map((key, keyIndex) => (
                    <span key={key}>
                      {keyIndex > 0 ? <span>+</span> : null}
                      <kbd>{key}</kbd>
                    </span>
                  ))}
                </span>
              ))}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

export function ShortcutsDialog({
  portalContainer,
}: {
  readonly portalContainer: HTMLElement | null;
}) {
  const surface = useAtomValue(librarySurfaceAtom);
  const settings = useAtomValue(settingsAtom);
  const bootstrap = useAtomValue(bootstrapAtom);
  const close = useSetAtom(closeLibrarySurfaceAtom);
  if (surface?.kind !== "shortcuts") return null;
  return (
    <Dialog.Root
      modal="trap-focus"
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <Dialog.Portal container={portalContainer}>
        <Dialog.Backdrop className="shell-shortcuts-backdrop" />
        <Dialog.Viewport className="shell-shortcuts-viewport">
          <Dialog.Popup
            finalFocus={() => surface.returnFocus ?? false}
            className="shortcut-help shell-shortcuts"
          >
            <header className="shell-shortcuts-header">
              <Dialog.Title>Keyboard shortcuts</Dialog.Title>
              <Dialog.Close
                render={
                  <Button variant="ghost" size="icon" aria-label="Close keyboard shortcuts" />
                }
              >
                <CloseIcon />
              </Dialog.Close>
            </header>
            <NavigatorPaginatedOverflow pageLabel="Keyboard shortcuts">
              <div className="shortcut-grid shell-shortcuts-groups">
                <ShortcutGroup title="App navigation" shortcuts={navigation} />
                <ShortcutGroup title="Library shelves" shortcuts={shelf} />
                <ShortcutGroup title="Controls" shortcuts={controls} />
                {(settings?.preferences ?? bootstrap?.preferences)?.keyboardMode === "vim" ? (
                  <ShortcutGroup title="Vim navigation" shortcuts={vim} />
                ) : null}
              </div>
            </NavigatorPaginatedOverflow>
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
