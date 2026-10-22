import { useEffect, useRef, useState } from "react";
import { type PublicationSummary } from "../host/bootstrap/types";
import { nativeRequest } from "../host/requests";
import { CreatorList, MetadataText } from "./metadata-text";

const imageTypes = new Set([
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/svg+xml",
  "image/webp",
  "image/avif",
  "image/jxl",
]);

export function BookCover({
  publication,
  sessionToken,
}: {
  publication: PublicationSummary;
  sessionToken: string | null;
}) {
  const host = useRef<HTMLSpanElement>(null);
  const [nearViewport, setNearViewport] = useState(false);
  const [cover, setCover] = useState<{ id: string; url: string } | null>(null);

  useEffect(() => {
    if (!publication.hasCover || sessionToken === null) return;
    const element = host.current;
    if (element === null || typeof IntersectionObserver === "undefined") {
      setNearViewport(true);
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setNearViewport(true);
          observer.disconnect();
        }
      },
      { rootMargin: "320px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [publication.hasCover, publication.publicationId, sessionToken]);

  useEffect(() => {
    if (!nearViewport || !publication.hasCover || sessionToken === null) return;
    const controller = new AbortController();
    let objectUrl: string | null = null;
    void (async () => {
      try {
        const response = await nativeRequest(
          `/native/publications/${publication.publicationId}/cover`,
          { sessionToken, trusted: true },
          { signal: controller.signal },
        );
        const mime = response.headers.get("Content-Type")?.split(";", 1)[0]?.trim() ?? "";
        if (!response.ok || !imageTypes.has(mime)) return;
        const blob = await response.blob();
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(blob);
        setCover({ id: publication.publicationId, url: objectUrl });
      } catch (cause) {
        if (!(cause instanceof DOMException && cause.name === "AbortError")) {
          console.warn(`Cover unavailable for ${publication.publicationId}`, cause);
        }
      }
    })();
    return () => {
      controller.abort();
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
    };
  }, [nearViewport, publication.hasCover, publication.publicationId, sessionToken]);

  return (
    <span ref={host} className="book-cover" aria-hidden="true">
      {cover?.id === publication.publicationId ? (
        <img src={cover.url} alt="" />
      ) : (
        <span className="book-cover-placeholder">
          <span>
            <MetadataText text={publication.title} direction={publication.titleDirection} />
          </span>
          <small>
            <CreatorList
              creators={publication.creators}
              directions={publication.creatorDirections}
            />
          </small>
        </span>
      )}
    </span>
  );
}
