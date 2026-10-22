import type { MountedPublication } from "epub-navigator";
import { hasExactKeys, isRecord } from "../guards";
import type {
  MetadataDirection,
  PublicationSummary,
  ReadingBootstrap,
  ResolvedPublicationPolicy,
} from "./types";

/**
 * How a metadata element reads, as EPUB resolves it: the element's own `dir` when it has
 * one, otherwise the package's, otherwise undeclared.
 *
 * Undeclared is not `auto`. An explicit `auto` asks for the direction to be inferred from
 * the text, while silence leaves the choice to the reading system.
 */
export function resolvedMetadataDirection(
  element: string | null | undefined,
  packageDirection: string | null | undefined,
): MetadataDirection | null {
  // A token EPUB does not define is not a declaration, so it is passed over rather than
  // blocking what the package said.
  const declared = (value: string | null | undefined): MetadataDirection | null =>
    value === "ltr" || value === "rtl" || value === "auto" ? value : null;
  return declared(element) ?? declared(packageDirection);
}

export function bootstrapPublicationSummary(configuration: ReadingBootstrap): PublicationSummary {
  const metadata = configuration.publication.facts.package.metadata;
  const packageDirection = configuration.publication.facts.package.dir;
  const namedCreators = metadata.creator.filter(
    ({ content }) => content !== null && content.trim() !== "",
  );
  return {
    publicationId: configuration.publicationId,
    title: metadata.title[0]?.content ?? "Untitled publication",
    titleDirection: resolvedMetadataDirection(metadata.title[0]?.dir, packageDirection),
    creators: namedCreators
      .map(({ content }) => content)
      .filter((content): content is string => content !== null),
    creatorDirections: namedCreators.map(({ dir }) =>
      resolvedMetadataDirection(dir, packageDirection),
    ),
    language: configuration.publication.facts.package.metadata.language[0]?.content ?? null,
    readingDirection:
      configuration.publication.facts.package.spine.pageProgressionDirection ?? null,
    importedAt: "",
    lastReadAt: null,
    hasReadingPosition: configuration.initialReadingPosition !== null,
    hasCover: false,
  };
}

export function isPublicationSummary(value: unknown): value is PublicationSummary {
  return (
    isRecord(value) &&
    typeof value.publicationId === "string" &&
    /^[0-9a-f]{32}$/.test(value.publicationId) &&
    typeof value.title === "string" &&
    value.title.length > 0 &&
    Array.isArray(value.creators) &&
    value.creators.every((creator) => typeof creator === "string" && creator.length > 0) &&
    (value.language === null || typeof value.language === "string") &&
    (value.readingDirection === null ||
      value.readingDirection === "ltr" ||
      value.readingDirection === "rtl" ||
      value.readingDirection === "default") &&
    typeof value.importedAt === "string" &&
    value.importedAt.length > 0 &&
    (value.lastReadAt === null || typeof value.lastReadAt === "string") &&
    typeof value.hasReadingPosition === "boolean" &&
    value.hasReadingPosition === (value.lastReadAt !== null) &&
    typeof value.hasCover === "boolean"
  );
}

export function isMountedPublication(value: unknown): value is MountedPublication {
  return (
    isRecord(value) &&
    hasExactKeys(value, new Set(["facts", "mount"])) &&
    isRecord(value.facts) &&
    isRecord(value.mount) &&
    hasExactKeys(value.mount, new Set(["baseUrl", "remoteResources", "contentOrigin"])) &&
    typeof value.mount.baseUrl === "string" &&
    (value.mount.remoteResources === "allowed" || value.mount.remoteResources === "blocked") &&
    value.mount.contentOrigin === "isolated-publication-origin"
  );
}

export function isResolvedPublicationPolicy(value: unknown): value is ResolvedPublicationPolicy {
  return (
    isRecord(value) &&
    hasExactKeys(value, new Set(["activeContent", "remoteResources"])) &&
    (value.activeContent === "enabled" || value.activeContent === "disabled") &&
    (value.remoteResources === "allowed" || value.remoteResources === "blocked")
  );
}
