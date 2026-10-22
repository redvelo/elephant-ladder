import { atom } from "jotai";

export type Destination = "home" | "library" | "settings";

export const destinationAtom = atom<Destination>("home");
