import { Button } from "@base-ui/react/button";
import { useRef, useState, type ReactNode } from "react";
import { type PublicationSummary } from "../host/bootstrap/types";
import { BookCover } from "./book-cover";
import { CreatorList } from "./metadata-text";
import { creatorsDirection } from "./metadata-direction";
import { TargetMenu, TargetMenuItem, TargetMenuSeparator } from "../ui/target-menu";
import { usePublicationDescription } from "./publication-description";
import { formatLibraryDate } from "../lib/format";

export type BookAction =
  | "reflections"
  | "default-layout"
  | "copy-annotations"
  | "export-annotations"
  | "import-annotations"
  | "site-data"
  | "delete";

export function BookMenu({
  publication,
  sessionToken,
  disabled,
  container,
  className = "book-open",
  tabIndex,
  onAction,
  onOpen,
  children,
}: {
  publication: PublicationSummary;
  sessionToken: string | null;
  disabled: boolean;
  container: HTMLElement | null;
  className?: string;
  /** A shelf is one tab stop: the grid moves focus between its books itself. */
  tabIndex?: number;
  onAction: (action: BookAction, publication: PublicationSummary, trigger: HTMLElement) => void;
  onOpen: () => void;
  children: ReactNode;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  const suppressClick = useRef(false);
  const [open, setOpen] = useState(false);
  const description = usePublicationDescription(
    publication.publicationId,
    open ? sessionToken : null,
  );
  const select = (action: BookAction) => {
    if (trigger.current === null) return;
    onAction(action, publication, trigger.current);
  };
  return (
    <TargetMenu
      title={publication.title}
      disabled={disabled}
      container={container}
      onOpenChange={setOpen}
      media={<BookCover publication={publication} sessionToken={sessionToken} />}
      header={
        <>
          {publication.creators.length > 0 ? (
            <p
              className="target-menu-byline"
              dir={creatorsDirection(publication.creatorDirections) ?? "auto"}
            >
              <CreatorList
                creators={publication.creators}
                directions={publication.creatorDirections}
              />
            </p>
          ) : null}
          {description === null ? null : <p className="target-menu-description">{description}</p>}
          <p className="target-menu-facts">
            Added {formatLibraryDate(publication.importedAt)}
            {publication.lastReadAt === null
              ? null
              : ` · Last opened ${formatLibraryDate(publication.lastReadAt)}`}
          </p>
        </>
      }
      trigger={
        <Button
          ref={trigger}
          disabled={disabled}
          className={className}
          tabIndex={tabIndex}
          aria-label={`Read ${publication.title}`}
          data-publication-id={publication.publicationId}
          data-publication-action="open"
          onPointerDown={() => {
            suppressClick.current = false;
          }}
          onClick={(event) => {
            if (suppressClick.current) {
              event.preventDefault();
              return;
            }
            if (!disabled) onOpen();
          }}
          onKeyDown={(event) => {
            if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
              event.preventDefault();
              const bounds = event.currentTarget.getBoundingClientRect();
              event.currentTarget.dispatchEvent(
                new MouseEvent("contextmenu", {
                  bubbles: true,
                  cancelable: true,
                  clientX: bounds.x + bounds.width / 2,
                  clientY: bounds.y + bounds.height / 2,
                }),
              );
            } else if (event.key === "Enter" || event.key === " ") {
              suppressClick.current = false;
            }
          }}
          onContextMenu={() => {
            suppressClick.current = true;
          }}
        >
          {children}
        </Button>
      }
    >
      <TargetMenuItem opensSurface onClick={() => select("reflections")}>
        Reflections
      </TargetMenuItem>
      <TargetMenuSeparator />
      <TargetMenuItem
        data-publication-id={publication.publicationId}
        data-publication-action="copy-annotations"
        onClick={() => select("copy-annotations")}
      >
        Copy annotations as Markdown
      </TargetMenuItem>
      <TargetMenuItem
        data-publication-id={publication.publicationId}
        data-publication-action="export-annotations"
        onClick={() => select("export-annotations")}
      >
        Export annotations
      </TargetMenuItem>
      <TargetMenuItem
        data-publication-id={publication.publicationId}
        data-publication-action="import-annotations"
        onClick={() => select("import-annotations")}
      >
        Import annotations…
      </TargetMenuItem>
      <TargetMenuSeparator />
      <TargetMenuItem
        data-publication-id={publication.publicationId}
        data-publication-action="default-layout"
        onClick={() => select("default-layout")}
      >
        Use the default layout
      </TargetMenuItem>
      <TargetMenuItem
        opensSurface
        data-publication-id={publication.publicationId}
        data-publication-action="site-data"
        onClick={() => select("site-data")}
      >
        Permissions…
      </TargetMenuItem>
      <TargetMenuItem opensSurface onClick={() => select("delete")}>
        Delete from device…
      </TargetMenuItem>
    </TargetMenu>
  );
}
