import { Button } from "epub-navigator/ui";
import { useCoarsePointer } from "../lib/pointer";

/** The first screen a new library shows: one book to open now, one way to bring your own. */
export function LibraryWelcome({
  loaded,
  disabled,
  onImport,
  onSample,
}: {
  loaded: boolean;
  disabled: boolean;
  onImport: () => void;
  onSample: () => void;
}) {
  const coarse = useCoarsePointer();
  // The library answers in a few frames, so a message about the wait is gone before it is read.
  if (!loaded) return null;
  return (
    <section className="empty-library">
      <span className="empty-book-mark" aria-hidden="true">
        EL
      </span>
      <h1>Your next chapter starts here.</h1>
      <p>Bring a book, or start with the one already here.</p>
      <div className="empty-library-actions">
        <Button disabled={disabled} onClick={onImport}>
          Import EPUB
        </Button>
        <Button variant="outline" disabled={disabled} onClick={onSample}>
          Open the sample book
        </Button>
      </div>
      <small>
        Alice’s Adventures in Wonderland, illustrated by John Tenniel.
        {coarse ? null : " You can also drop EPUBs onto this screen."}
      </small>
    </section>
  );
}

/** The shelf before it holds anything. Home carries the welcome, so this stays quiet. */
export function EmptyShelf({
  loaded,
  disabled,
  onImport,
}: {
  loaded: boolean;
  disabled: boolean;
  onImport: () => void;
}) {
  if (!loaded) return null;
  return (
    <section className="empty-shelf">
      <p>No books yet.</p>
      <Button disabled={disabled} onClick={onImport}>
        Import EPUB
      </Button>
    </section>
  );
}
