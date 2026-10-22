import type { NavigatorReadingSummary } from "epub-navigator/react";

export const reflectionPrompts = [
  "What stayed with you?",
  "What are you thinking about this book?",
  "Anything you want to remember?",
  "What surprised you?",
  "What are you wondering about?",
] as const;

export interface Reflection {
  id: string;
  text: string;
  prompt: number;
  context: NavigatorReadingSummary | null;
  createdAt: string;
  updatedAt: string;
}

export interface Journal {
  summary: NavigatorReadingSummary | null;
  draft: Reflection | null;
  entries: Reflection[];
}

export type JournalMutation =
  | { action: "begin" }
  | { action: "update"; id: string; text: string }
  | { action: "publish" | "delete"; id: string };

export function summaryPages(summary: NavigatorReadingSummary | null): string | null {
  const pages = summary?.pages;
  return pages === undefined || pages === null
    ? null
    : `${pages.current} / ${pages.accuracy === "calibrated" ? "≈ " : ""}${pages.total} pages`;
}
