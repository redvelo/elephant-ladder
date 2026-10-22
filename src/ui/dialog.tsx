import {
  Button,
  CloseIcon,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  NavigatorPaginatedOverflow,
} from "epub-navigator/ui";
import { useRef, type ReactNode } from "react";
import "./dialog.css";

export interface AppDialogProps {
  /** A decision states its answers explicitly; a surface is somewhere you went. */
  kind?: "decision" | "surface";
  kicker?: string | undefined;
  title: string;
  description?: ReactNode;
  pageLabel?: string | undefined;
  container: HTMLElement | null;
  returnFocus?: HTMLElement | null;
  onClose: () => void;
  /** Kept outside the paginated body so an answer never needs a page turn. */
  actions?: ReactNode;
  children?: ReactNode;
}

export function AppDialog({
  kind = "surface",
  kicker,
  title,
  description,
  pageLabel,
  container,
  returnFocus,
  onClose,
  actions,
  children,
}: AppDialogProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="app-dialog"
        data-kind={kind}
        {...(kind === "decision" ? { role: "alertdialog" as const } : {})}
        portalContainer={container}
        backdropClassName="app-dialog-backdrop"
        viewportClassName="app-dialog-viewport"
        initialFocus={heading}
        finalFocus={() => returnFocus ?? false}
      >
        <header className="app-dialog-header">
          <div>
            {kicker === undefined ? null : <p className="app-dialog-kicker">{kicker}</p>}
            <DialogTitle ref={heading} tabIndex={-1} className="app-dialog-title">
              {title}
            </DialogTitle>
          </div>
          {kind === "surface" ? (
            <Button variant="ghost" size="icon-lg" aria-label="Close" onClick={onClose}>
              <CloseIcon />
            </Button>
          ) : null}
        </header>
        <NavigatorPaginatedOverflow pageLabel={pageLabel ?? title} className="app-dialog-pages">
          {description === undefined ? null : (
            <DialogDescription className="app-dialog-description">{description}</DialogDescription>
          )}
          {children === undefined ? null : <div className="app-dialog-body">{children}</div>}
        </NavigatorPaginatedOverflow>
        {actions === undefined ? null : <div className="app-dialog-actions">{actions}</div>}
      </DialogContent>
    </Dialog>
  );
}
