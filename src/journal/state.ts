import { atom } from "jotai";
import { requestJournal } from "./requests";
import { type Journal, type JournalMutation } from "./model";
import { bootstrapAtom } from "../app/session";

export type JournalState =
  | { status: "loading" }
  | { status: "ready"; journal: Journal }
  | { status: "failed"; message: string };
export const journalsAtom = atom<Record<string, JournalState>>({});

export const loadJournalAtom = atom(null, (get, set, publicationId: string): (() => void) => {
  const bootstrap = get(bootstrapAtom);
  const controller = new AbortController();
  if (bootstrap?.mode !== "library") return () => controller.abort();
  set(journalsAtom, { ...get(journalsAtom), [publicationId]: { status: "loading" } });
  void requestJournal(
    publicationId,
    bootstrap.sessionToken,
    bootstrap.libraryDocumentId,
    undefined,
    controller.signal,
  )
    .then((journal) => {
      if (!controller.signal.aborted)
        set(journalsAtom, { ...get(journalsAtom), [publicationId]: { status: "ready", journal } });
    })
    .catch((cause: unknown) => {
      if (!controller.signal.aborted)
        set(journalsAtom, {
          ...get(journalsAtom),
          [publicationId]: {
            status: "failed",
            message: cause instanceof Error ? cause.message : "Could not load reflections.",
          },
        });
    });
  return () => controller.abort();
});

export const mutateJournalAtom = atom(
  null,
  async (get, set, publicationId: string, mutation: JournalMutation): Promise<Journal> => {
    const bootstrap = get(bootstrapAtom);
    if (bootstrap?.mode !== "library") throw new Error("Library is unavailable.");
    const journal = await requestJournal(
      publicationId,
      bootstrap.sessionToken,
      bootstrap.libraryDocumentId,
      mutation,
    );
    set(journalsAtom, { ...get(journalsAtom), [publicationId]: { status: "ready", journal } });
    return journal;
  },
);
