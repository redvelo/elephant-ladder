import { atom } from "jotai";

export const libraryQueryAtom = atom("");
export const librarySortAtom = atom<"recent" | "added" | "title" | "author">("recent");
export const libraryLanguageAtom = atom("");
export const libraryPageAtom = atom(0);

/** Titles and authors compared the way a reader types them: no accents, no punctuation. */
export function normalize(text: string): string {
  return text
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim();
}
