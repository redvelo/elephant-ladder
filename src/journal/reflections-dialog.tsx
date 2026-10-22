import { Button } from "epub-navigator/ui";
import { Button as BaseButton } from "@base-ui/react/button";
import { Field } from "@base-ui/react/field";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useRef, useState } from "react";
import { type PublicationSummary } from "../host/bootstrap/types";
import { AppDialog } from "../ui/dialog";
import {
  reflectionPrompts,
  summaryPages,
  type Reflection,
  type JournalMutation,
  type Journal,
} from "./model";
import { journalsAtom, mutateJournalAtom } from "./state";
import { formatLibraryDate } from "../lib/format";

export function ReflectionsDialog({
  publication,
  container,
  returnFocus,
  onClose,
}: {
  publication: PublicationSummary;
  container: HTMLElement | null;
  returnFocus: HTMLElement;
  onClose: () => void;
}) {
  const mutate = useSetAtom(mutateJournalAtom);
  const state = useAtomValue(journalsAtom)[publication.publicationId];
  const [editor, setEditor] = useState<{ entry: Reflection; draft: boolean } | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef<{ id: string; text: string } | null>(null);
  const writing = useRef<Promise<void> | null>(null);

  useEffect(() => {
    let active = true;
    void mutate(publication.publicationId, { action: "begin" })
      .then((journal) => {
        if (!active) return;
        if (journal.draft !== null) {
          setEditor({ entry: journal.draft, draft: true });
          setText(journal.draft.text);
        }
      })
      .catch((cause: unknown) => {
        if (active)
          setError(cause instanceof Error ? cause.message : "Could not open reflections.");
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, [mutate, publication.publicationId]);

  const flush = () =>
    flushDraft(
      pending,
      writing,
      (mutation) => mutate(publication.publicationId, mutation),
      setSaving,
      setError,
    );

  const run = (action: () => Promise<void>) => {
    setBusy(true);
    return flush()
      .then(action)
      .then(() => setError(null))
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : "Could not save reflections.");
      })
      .finally(() => setBusy(false));
  };
  // Dismissal never waits on a save. The draft finishes writing the way it does between
  // keystrokes, so Close, Escape and the system back gesture answer like every other surface.
  const close = () => {
    void flush().catch(() => {});
    onClose();
  };
  const begin = () =>
    void run(async () => {
      const journal = await mutate(publication.publicationId, { action: "begin" });
      if (journal.draft !== null) {
        setEditor({ entry: journal.draft, draft: true });
        setText(journal.draft.text);
      }
    });
  const entries = state?.status === "ready" ? state.journal.entries : [];

  return (
    <AppDialog
      title="Reflections"
      pageLabel="Reflections"
      description={publication.title}
      container={container}
      returnFocus={returnFocus}
      onClose={close}
      actions={
        <Button variant="outline" onClick={close}>
          Done
        </Button>
      }
    >
      {error !== null ? (
        <p className="reflection-error" role="alert">
          {error}
        </p>
      ) : null}
      {editor === null ? (
        <Button disabled={busy} onClick={begin}>
          {busy ? "Opening…" : "Add a reflection"}
        </Button>
      ) : (
        <form
          className="reflection-composer"
          onSubmit={(event) => {
            event.preventDefault();
            if (!text.trim() || busy) return;
            void run(async () => {
              if (editor.draft)
                await mutate(publication.publicationId, { action: "publish", id: editor.entry.id });
              else
                await mutate(publication.publicationId, {
                  action: "update",
                  id: editor.entry.id,
                  text,
                });
              setEditor(null);
              setText("");
            });
          }}
        >
          <Field.Root>
            <Field.Label>
              {editor.draft ? reflectionPrompts[editor.entry.prompt] : "Edit reflection"}
            </Field.Label>
            <ReflectionContext entry={editor.entry} />
            <Field.Control
              render={<textarea rows={5} />}
              id="reflection-text"
              value={text}
              maxLength={5000}
              disabled={busy}
              onValueChange={(value) => {
                setText(value);
                if (editor.draft) {
                  pending.current = { id: editor.entry.id, text: value };
                  void flush().catch(() => {});
                }
              }}
            />
          </Field.Root>
          <div className="reflection-composer-actions">
            <span role="status">
              {saving ? "Saving draft…" : editor.draft && error === null ? "Draft saved" : ""}
            </span>
            <Button type="submit" disabled={busy || !text.trim()}>
              {busy ? "Saving…" : "Save reflection"}
            </Button>
          </div>
        </form>
      )}
      {entries.length > 0 ? (
        <section className="reflection-history" aria-label="Previous reflections">
          <h3>Previous reflections</h3>
          {entries.map((entry) => (
            <article key={entry.id}>
              <ReflectionContext entry={entry} />
              <p className="reflection-text">{entry.text}</p>
              <div className="reflection-entry-actions">
                <BaseButton
                  className="text-control"
                  disabled={busy || editor?.draft === false}
                  onClick={() =>
                    void run(async () => {
                      setEditor({ entry, draft: false });
                      setText(entry.text);
                    })
                  }
                >
                  Edit
                </BaseButton>
                <BaseButton
                  className="text-control"
                  disabled={busy}
                  onClick={() =>
                    void run(async () => {
                      await mutate(publication.publicationId, { action: "delete", id: entry.id });
                      if (editor?.entry.id === entry.id) {
                        setEditor(null);
                        setText("");
                      }
                    })
                  }
                >
                  Delete
                </BaseButton>
              </div>
            </article>
          ))}
        </section>
      ) : null}
    </AppDialog>
  );
}

function ReflectionContext({ entry }: { entry: Reflection }) {
  return (
    <p className="reflection-context">
      {[formatLibraryDate(entry.createdAt), entry.context?.chapter, summaryPages(entry.context)]
        .filter(Boolean)
        .join(" · ")}
    </p>
  );
}

function flushDraft(
  pending: { current: { id: string; text: string } | null },
  writing: { current: Promise<void> | null },
  mutate: (mutation: JournalMutation) => Promise<Journal>,
  setSaving: (saving: boolean) => void,
  setError: (error: string | null) => void,
): Promise<void> {
  if (writing.current !== null) return writing.current;
  if (pending.current === null) return Promise.resolve();
  const write = async () => {
    setSaving(true);
    try {
      while (pending.current !== null) {
        const update = pending.current;
        pending.current = null;
        try {
          await mutate({ action: "update", ...update });
        } catch (cause) {
          pending.current ??= update;
          throw cause;
        }
      }
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not save your draft.");
      throw cause;
    } finally {
      writing.current = null;
      setSaving(false);
    }
  };
  writing.current = write();
  return writing.current;
}
