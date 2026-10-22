import { expect, test } from "vitest";
import {
  annotationFromRecord,
  annotationLink,
  annotationMarkdown,
  annotationSetMarkdown,
  readExportedAnnotationSet,
} from "../../src/annotations/markdown";

const target = {
  source: "chapter.xhtml",
  selector: [{ type: "TextPositionSelector", start: 4, end: 12 }],
};

const exported = {
  "@context": "http://www.idpf.org/epub/annotations/v1",
  type: "AnnotationSet",
  about: {
    "dc:identifier": ["urn:isbn:9780000000000", "urn:blake3:abc"],
    "dc:title": "Moby-Dick",
  },
  items: [
    {
      id: "urn:uuid:1",
      type: "Annotation",
      motivation: "commenting",
      target: {
        ...target,
        meta: {
          headings: [{ level: 1, txt: "Loomings" }],
          text: { highlight: "Call me\nIshmael." },
        },
      },
      body: { type: "TextualBody", value: "Opening line", format: "text/plain" },
    },
    {
      id: "urn:uuid:2",
      type: "Annotation",
      motivation: "bookmarking",
      target: { source: "chapter.xhtml" },
    },
    {
      id: "urn:uuid:3",
      type: "Annotation",
      motivation: "highlighting",
      target: {
        ...target,
        meta: { headings: [{ level: 1, txt: "Loomings" }], text: { highlight: "Whenever" } },
      },
    },
  ],
};

test("annotation links carry book identity and the target without meta", () => {
  const link = new URL(annotationLink(["urn:isbn:1", "urn:blake3:ff"], target));
  expect(link.protocol).toBe("elephant-ladder:");
  expect(link.searchParams.get("h")).toBe("ff");
  expect(link.searchParams.get("id")).toBe("urn:isbn:1");
  const encoded = link.searchParams.get("target")!.replaceAll("-", "+").replaceAll("_", "/");
  expect(JSON.parse(atob(encoded))).toEqual(target);
});

test("a book set renders grouped quotes and notes without bookmarks", () => {
  const set = readExportedAnnotationSet(exported)!;
  const link = annotationLink(exported.about["dc:identifier"], target);
  expect(annotationSetMarkdown(set)).toBe(
    [
      "## Moby-Dick",
      "### Loomings",
      "> Call me\n> Ishmael.",
      "Opening line",
      `— [Open in Elephant Ladder](${link})`,
      "> Whenever",
      `— [Open in Elephant Ladder](${link})\n`,
    ].join("\n\n"),
  );
});

test("a reader record renders with its section and no network data", () => {
  const annotation = annotationFromRecord({
    type: "note",
    id: "3",
    targetJson: JSON.stringify(target),
    occurrence: 0,
    excerpt: "Whenever",
    sectionLabel: "Loomings",
    text: "A note",
  });
  const link = annotationLink(["urn:blake3:abc"], target);
  expect(annotationMarkdown(["urn:blake3:abc"], annotation)).toBe(
    `> Whenever\n\nA note\n\n— *Loomings* · [Open in Elephant Ladder](${link})\n`,
  );
  expect(readExportedAnnotationSet({ items: [{ id: 1 }] })).toBeNull();
});
