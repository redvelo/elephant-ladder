import type { NavigatorAnnotationRecord } from "epub-navigator/react";

const annotationLinkScheme = "elephant-ladder";

export interface ExportedAnnotation {
  readonly id: string;
  readonly motivation: string | null;
  readonly quote: string | null;
  readonly section: string | null;
  readonly note: string | null;
  readonly target: Record<string, unknown>;
}

export interface ExportedAnnotationSet {
  readonly title: string | null;
  readonly identifiers: readonly string[];
  readonly items: readonly ExportedAnnotation[];
}

export function readExportedAnnotationSet(value: unknown): ExportedAnnotationSet | null {
  if (!isRecord(value) || !Array.isArray(value.items)) return null;
  const about = isRecord(value.about) ? value.about : {};
  const identifiers = stringList(about["dc:identifier"]);
  const items: ExportedAnnotation[] = [];
  for (const item of value.items) {
    if (!isRecord(item) || typeof item.id !== "string" || !isRecord(item.target)) return null;
    const { meta, ...target } = item.target;
    const headings = isRecord(meta) && Array.isArray(meta.headings) ? meta.headings : [];
    const heading: unknown = headings[headings.length - 1];
    const text = isRecord(meta) && isRecord(meta.text) ? meta.text.highlight : null;
    const body = isRecord(item.body) ? item.body.value : null;
    items.push({
      id: item.id,
      motivation: typeof item.motivation === "string" ? item.motivation : null,
      quote: typeof text === "string" ? text : null,
      section: isRecord(heading) && typeof heading.txt === "string" ? heading.txt : null,
      note: localizedText(body),
      target,
    });
  }
  return { title: localizedText(about["dc:title"]), identifiers, items };
}

export function annotationSetMarkdown(set: ExportedAnnotationSet): string {
  const blocks: string[] = [];
  if (set.title !== null) blocks.push(`## ${singleLine(set.title)}`);
  let section: string | null = null;
  for (const item of set.items) {
    if (item.motivation === "bookmarking") continue;
    if (item.section !== null && item.section !== section) {
      blocks.push(`### ${singleLine(item.section)}`);
    }
    section = item.section ?? section;
    blocks.push(itemMarkdown(set.identifiers, item, false));
  }
  return `${blocks.join("\n\n")}\n`;
}

export function annotationMarkdown(
  identifiers: readonly string[],
  annotation: ExportedAnnotation,
): string {
  return `${itemMarkdown(identifiers, annotation, true)}\n`;
}

export function annotationFromRecord(record: NavigatorAnnotationRecord): ExportedAnnotation {
  const parsed: unknown = JSON.parse(record.targetJson);
  const { meta: _meta, ...target } = isRecord(parsed) ? parsed : {};
  return {
    id: `urn:uuid:${record.id}`,
    motivation:
      record.type === "bookmark"
        ? "bookmarking"
        : record.type === "note"
          ? "commenting"
          : "highlighting",
    quote: record.type === "bookmark" ? null : record.excerpt,
    section: record.sectionLabel,
    note: record.type === "note" ? record.text : null,
    target,
  };
}

export function annotationLink(
  identifiers: readonly string[],
  target: Record<string, unknown>,
): string {
  const parameters = new URLSearchParams();
  const hash = identifiers.find((identifier) => identifier.startsWith("urn:blake3:"));
  if (hash !== undefined) parameters.set("h", hash.slice("urn:blake3:".length));
  const identifier = identifiers.find((candidate) => !candidate.startsWith("urn:blake3:"));
  if (identifier !== undefined) parameters.set("id", identifier);
  parameters.set("target", base64Url(JSON.stringify(target)));
  return `${annotationLinkScheme}://annotation?${parameters.toString()}`;
}

function itemMarkdown(
  identifiers: readonly string[],
  item: ExportedAnnotation,
  withSection: boolean,
) {
  const parts: string[] = [];
  if (item.quote !== null) {
    parts.push(
      item.quote
        .split(/\r?\n/u)
        .map((line) => (line.length === 0 ? ">" : `> ${line}`))
        .join("\n"),
    );
  }
  if (item.note !== null && item.motivation !== "bookmarking") parts.push(item.note);
  const source = [
    withSection && item.section !== null ? `*${singleLine(item.section)}*` : null,
    `[Open in Elephant Ladder](${annotationLink(identifiers, item.target)})`,
  ].filter((part): part is string => part !== null);
  parts.push(`— ${source.join(" · ")}`);
  return parts.join("\n\n");
}

function base64Url(value: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(value)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/gu, "-").replace(/\//gu, "_").replace(/=+$/u, "");
}

function localizedText(value: unknown): string | null {
  if (typeof value === "string") return value;
  if (isRecord(value) && typeof value.text === "string") return value.text;
  if (Array.isArray(value)) return localizedText(value[0]);
  return null;
}

function stringList(value: unknown): string[] {
  if (typeof value === "string") return [value];
  return Array.isArray(value)
    ? value.filter((candidate): candidate is string => typeof candidate === "string")
    : [];
}

function singleLine(value: string): string {
  return value.replace(/\s+/gu, " ").trim();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
