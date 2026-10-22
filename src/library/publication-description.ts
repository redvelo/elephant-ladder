import { useEffect, useState } from "react";
import { isRecord } from "../host/guards";
import { nativeRequest } from "../host/requests";

export function descriptionText(markup: string): string {
  const template = document.createElement("template");
  template.innerHTML = markup;
  template.content.querySelectorAll("script, style").forEach((element) => element.remove());
  template.content.querySelectorAll("p, div, br, li").forEach((element) => element.append(" "));
  return (template.content.textContent ?? "").replace(/\s+/g, " ").trim();
}

export function usePublicationDescription(
  publicationId: string,
  token: string | null,
): string | null {
  const [description, setDescription] = useState<{ id: string; text: string } | null>(null);
  useEffect(() => {
    if (token === null) return undefined;
    const controller = new AbortController();
    void nativeRequest(
      `/native/publications/${encodeURIComponent(publicationId)}/details`,
      { sessionToken: token },
      { signal: controller.signal },
    )
      .then(async (response) => {
        if (!response.ok) return;
        const value: unknown = await response.json();
        if (
          !controller.signal.aborted &&
          isRecord(value) &&
          typeof value.description === "string"
        ) {
          setDescription({ id: publicationId, text: descriptionText(value.description) });
        }
      })
      .catch(() => {});
    return () => controller.abort();
  }, [publicationId, token]);
  return description?.id === publicationId ? description.text || null : null;
}
