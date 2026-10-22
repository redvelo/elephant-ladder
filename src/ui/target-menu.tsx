import { ContextMenu as Menu } from "@base-ui/react/context-menu";
import { Button as BaseButton } from "@base-ui/react/button";
import { Drawer, DrawerClose, DrawerContent, DrawerTitle } from "epub-navigator/ui";
import { createContext, use, useRef, useState, type ReactElement, type ReactNode } from "react";
import { useCoarsePointer } from "../lib/pointer";
import "./menus.css";

/**
 * The pointer menu is drawn the way Navigator draws the reader's menus. The drawer's rows keep
 * their own touch geometry: a drawer has no reader counterpart to conform to.
 */
const surface =
  "rounded-2xl border border-border bg-popover p-1 text-sm text-popover-foreground outline-none";
const item =
  "flex min-h-11 w-full cursor-default items-center gap-2 rounded-xl px-2.5 text-start text-sm outline-none select-none data-disabled:pointer-events-none data-disabled:opacity-50 data-highlighted:bg-[var(--reader-selected)] data-highlighted:text-[var(--reader-selected-foreground)]";
const separator = "mx-1 my-1 h-px border-0 bg-border";

/** A menu for one thing: a header naming the target, then the actions that apply to it. */
export function TargetMenu({
  title,
  media,
  header,
  trigger,
  disabled,
  container,
  onOpenChange,
  children,
}: {
  readonly title: string;
  readonly media?: ReactNode;
  readonly header?: ReactNode;
  readonly trigger: ReactElement;
  readonly disabled: boolean;
  readonly container: HTMLElement | null;
  readonly onOpenChange?: ((open: boolean) => void) | undefined;
  readonly children: ReactNode;
}) {
  const coarse = useCoarsePointer();
  const [open, setOpen] = useState(false);
  // An action that opens another surface owns the focus that follows it.
  const openingSurface = useRef(false);
  const context = {
    drawer: coarse,
    onSelect: () => {
      openingSurface.current = true;
    },
  };
  const change = (next: boolean) => {
    if (next) openingSurface.current = false;
    setOpen(next);
    onOpenChange?.(next);
  };
  return (
    <TargetMenuContext value={context}>
      {/* The context menu contributes only the gesture on a coarse pointer; leaving its own
          root open there would lay a dismissal layer over the drawer. */}
      <Menu.Root disabled={disabled} open={coarse ? false : open} onOpenChange={change}>
        <Menu.Trigger render={trigger} />
        {coarse ? null : (
          <Menu.Portal container={container}>
            <Menu.Positioner
              className="target-menu-positioner"
              side="bottom"
              align="end"
              sideOffset={4}
              collisionPadding={12}
            >
              <Menu.Popup
                className={`target-menu ${surface}`}
                aria-label={title}
                finalFocus={() => !openingSurface.current}
              >
                <div className="target-menu-header">
                  {media}
                  <div className="target-menu-identity">
                    <p className="target-menu-title">{title}</p>
                    {header}
                  </div>
                </div>
                {children}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        )}
      </Menu.Root>
      {coarse ? (
        <Drawer open={open} onOpenChange={change}>
          <DrawerContent
            className="target-menu-drawer"
            backdropClassName="target-menu-backdrop"
            portalContainer={container}
            finalFocus={() => !openingSurface.current}
          >
            <div className="target-menu-header">
              {media}
              <div className="target-menu-identity">
                <DrawerTitle className="target-menu-title">{title}</DrawerTitle>
                {header}
              </div>
            </div>
            <div className="target-menu-items">{children}</div>
          </DrawerContent>
        </Drawer>
      ) : null}
    </TargetMenuContext>
  );
}

export function TargetMenuItem({
  onClick,
  opensSurface = false,
  children,
  ...rest
}: {
  readonly onClick: () => void;
  /** Whether the action replaces the menu with another surface, which then owns focus. */
  readonly opensSurface?: boolean;
  readonly disabled?: boolean;
  readonly children: ReactNode;
  readonly "data-publication-id"?: string;
  readonly "data-publication-action"?: string;
  readonly "data-qualification-action"?: string;
}) {
  const { drawer, onSelect } = use(TargetMenuContext);
  const select = () => {
    if (opensSurface) onSelect();
    onClick();
  };
  if (!drawer) {
    return (
      <Menu.Item {...rest} className={item} onClick={select}>
        {children}
      </Menu.Item>
    );
  }
  return (
    <DrawerClose render={<BaseButton {...rest} className="target-menu-item" onClick={select} />}>
      {children}
    </DrawerClose>
  );
}

export function TargetMenuSeparator() {
  const { drawer } = use(TargetMenuContext);
  if (drawer) return <hr className="target-menu-separator" />;
  return <Menu.Separator className={separator} />;
}

const TargetMenuContext = createContext<{
  readonly drawer: boolean;
  readonly onSelect: () => void;
}>({ drawer: false, onSelect: () => {} });
