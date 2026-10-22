import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import type { Frame, Page } from "@playwright/test";
import { strToU8, zipSync } from "fflate";
import {
  expect,
  importPublication,
  openPanel,
  permissionPrompt,
  publicationCard,
  test,
} from "./application";

const NAVIGATION_LIST = ".epub-navigator-reader__navigation-list";
const PRIMARY = "iframe[data-epub-navigator-participant=primary]";

const xml = (value: string): string =>
  value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll('"', "&quot;");
const css = `/* Authored declarations are preserved; EPUB's prefixed ones gain a restatement. */
#external{-epub-hyphens:auto;-webkit-hyphens:none;hyphens:manual}
section.destination{break-before:page;min-height:90vh}
img{width:1em;height:1em}
`;
// The host restates EPUB's prefixed properties for an engine that does not implement them, next
// to the declaration they came from so a later authored standard declaration still wins the
// cascade. Everything the author wrote survives unchanged around the insertion.
const servedCss = css.replace("-epub-hyphens:auto;", "-epub-hyphens:auto;hyphens: auto;");
const embeddedCss = "#embedded{-epub-line-break:strict;-webkit-line-break:normal;line-break:auto}";
const inlineCss = "-epub-text-align-last:end;-webkit-text-align-last:left;text-align-last:start";
// The content agent restates the same prefixed properties inside a `style` element and a `style`
// attribute, where a browser drops them at parse time but leaves the authored text intact.
const restatedEmbeddedCss = embeddedCss.replace(
  "-epub-line-break:strict;",
  "-epub-line-break:strict;line-break: strict;",
);
const restatedInlineCss = inlineCss.replace(
  "-epub-text-align-last:end;",
  "-epub-text-align-last:end;text-align-last: end;",
);
const document_ = (title: string, body: string, head = ""): string =>
  `<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en"><head><title>${title}</title>${head}</head><body>${body}</body></html>`;
const image = (alt: string, title = "Wrong title"): string =>
  `<img src="label.svg" alt="${alt}" title="${title}"/>`;
const destinations = ["image", "mixed", "empty", "nested", "hidden", "repeat"];
const entries: Record<string, string> = {
  mimetype: "application/epub+zip",
  "META-INF/container.xml":
    '<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
  "EPUB/package.opf": `<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:elephant:epub34-presentation-v1</dc:identifier><dc:title>EPUB34 presentation</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-09-13T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="css" href="authored" media-type="text/css"/><item id="label" href="label.svg" media-type="image/svg+xml"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="start" href="start.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="start"/><itemref idref="chapter" linear="no"/><itemref idref="chapter"/><itemref idref="chapter"/></spine></package>`,
  "EPUB/nav.xhtml": document_(
    "Navigation",
    `<nav epub:type="toc" hidden="hidden"><h2>${image("Contents alt")}</h2><ol><li><a href="start.xhtml">Start</a></li><li><a href="chapter.xhtml#image">${image("Image alt")}</a></li><li><a href="chapter.xhtml#mixed">Before ${image("picture")} after</a></li><li><a href="chapter.xhtml#empty">${image("")}</a></li><li><a href="chapter.xhtml#nested"><span>Nested <span>span</span></span> trailing text</a></li><li hidden="hidden"><a href="chapter.xhtml#hidden">Hidden point</a></li><li><a href="chapter.xhtml#repeat">Repeated resource</a></li></ol></nav><nav epub:type="page-list" hidden="hidden"><h2>${image("Pages alt")}</h2><ol><li hidden="hidden"><a href="chapter.xhtml#image">${image("Page image")}</a></li><li><a href="chapter.xhtml#nested"><span>Page nested</span> trailing</a></li></ol></nav>`,
  ),
  "EPUB/start.xhtml": document_(
    "Start",
    '<h1>Presentation start</h1><p>Static publication: no permission prompt expected.</p><a id="repeat-link" href="chapter.xhtml#repeat">Repeated resource hyperlink</a>',
  ),
  "EPUB/chapter.xhtml": document_(
    "Presentation",
    `<h1>Authored CSS</h1><p id="external">Standard hyphens control: ab&#173;cd</p><p id="embedded">Embedded stylesheet</p><p id="inline" style="${xml(inlineCss)}">Inline declarations</p>${destinations
      .map(
        (id) =>
          `<section class="destination" id="${id}"><h2>${id}</h2><a href="chapter.xhtml#repeat">Repeated resource hyperlink</a></section>`,
      )
      .join("")}`,
    `<link rel="stylesheet" type="text/css" href="authored"/><style id="authored-style">${embeddedCss}</style>`,
  ),
  "EPUB/authored": css,
  "EPUB/label.svg":
    '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="navy"/></svg>',
};
const archive = zipSync(
  Object.fromEntries(
    Object.entries(entries).map(([name, text]) => [
      name,
      [
        strToU8(text),
        { level: name === "mimetype" ? 0 : 6, mtime: new Date("2026-01-01T00:00:00Z") },
      ],
    ]),
  ),
);

async function primaryFrame(page: Page): Promise<Frame> {
  const source = await page.locator(PRIMARY).getAttribute("src");
  if (source === null) throw new Error("no primary participant frame is mounted");
  await expect.poll(() => page.frames().some((frame) => frame.url() === source)).toBe(true);
  return page.frames().find((frame) => frame.url() === source)!;
}

/** The panel animates, so opening it and selecting a tab is retried as one step. */
async function navigation(page: Page, tab: "Contents" | "Pages"): Promise<void> {
  const trigger = page.getByRole("tab", { name: tab, exact: true });
  await expect(async () => {
    await openPanel(page, "Navigation");
    await trigger.click({ timeout: 3_000 });
    await expect(trigger).toHaveAttribute("aria-selected", "true", { timeout: 2_000 });
  }).toPass();
  await expect(page.getByRole("tabpanel").locator(NAVIGATION_LIST).first()).toBeVisible();
}

async function showsFragment(page: Page, fragment: string): Promise<void> {
  const frame = await primaryFrame(page);
  await expect(frame.locator(`#${fragment} h2`)).toBeInViewport();
}

test("authored presentation survives native serving and generated navigation", async ({
  application,
  page,
}, testInfo) => {
  const fixture = testInfo.outputPath("presentation.epub");
  writeFileSync(fixture, archive);
  const digest = createHash("sha256").update(archive).digest("hex");

  const stylesheets: { status: number; mime: string | null; text: string }[] = [];
  page.on("response", (response) => {
    if (!new URL(response.url()).pathname.endsWith("/authored")) return;
    void response
      .text()
      .then((text) => {
        stylesheets.push({
          status: response.status(),
          mime: response.headers()["content-type"] ?? null,
          text,
        });
      })
      .catch(() => {
        // A frame can be retired before its body is read; the assertions below need one body.
      });
  });

  await page.setViewportSize({ width: 1400, height: 1000 });

  await test.step("ordinary static import without a permission prompt", async () => {
    await importPublication(page, fixture);
    await publicationCard(page).first().click();
    await expect
      .poll(
        async () => {
          if ((await permissionPrompt(page).count()) > 0) return "permission-popup";
          return page.locator("[data-reader-status]").first().getAttribute("data-reader-status");
        },
        { timeout: 120_000 },
      )
      .toBe("ready");
  });

  await test.step("an authored hyperlink selects the first linear repeated resource", async () => {
    const start = await primaryFrame(page);
    await start.locator("#repeat-link").click();
    await expect
      .poll(async () =>
        (await page.locator(PRIMARY).getAttribute("src"))?.includes("/chapter.xhtml"),
      )
      .toBe(true);
    await showsFragment(page, "repeat");
    // The spine is start, non-linear chapter, chapter, chapter: the hyperlink must take the
    // first linear occurrence, not the non-linear one before it or the repeat after it.
    await expect(page.locator(PRIMARY)).toHaveAttribute("data-epub-navigator-occurrence", "2");
  });

  await test.step("extensionless declared CSS keeps every authored byte and gains the restatement", async () => {
    await expect.poll(() => stylesheets.length).toBeGreaterThan(0);
    for (const stylesheet of stylesheets) {
      expect(stylesheet.status).toBe(200);
      expect(stylesheet.mime?.split(";")[0]).toBe("text/css");
      expect(stylesheet.text).toBe(servedCss);
      // The restatement is additive: no authored declaration is rewritten or dropped.
      for (const authored of [
        "-epub-hyphens:auto",
        "-webkit-hyphens:none",
        "hyphens:manual",
        "section.destination{break-before:page;min-height:90vh}",
        "img{width:1em;height:1em}",
      ]) {
        expect(stylesheet.text).toContain(authored);
      }
    }
  });

  await test.step("embedded and inline authored CSS keep every authored byte", async () => {
    const frame = await primaryFrame(page);
    // Authored nodes only: the reader adds instrumentation of its own to the document.
    const authored = await frame.evaluate(() => ({
      embedded: document.getElementById("authored-style")?.textContent ?? null,
      inline: document.getElementById("inline")?.getAttribute("style") ?? null,
      hyphens: getComputedStyle(document.getElementById("external")!).hyphens,
    }));
    expect(authored.embedded).toBe(restatedEmbeddedCss);
    expect(authored.inline).toBe(restatedInlineCss);
    // Additive in both places: the authored declarations survive in their authored order.
    for (const [subject, declarations] of [
      [
        authored.embedded,
        ["-epub-line-break:strict", "-webkit-line-break:normal", "line-break:auto"],
      ],
      [
        authored.inline,
        ["-epub-text-align-last:end", "-webkit-text-align-last:left", "text-align-last:start"],
      ],
    ] as const) {
      let cursor = -1;
      for (const declaration of declarations) {
        const found = subject!.indexOf(declaration, cursor + 1);
        expect(found, `${declaration} survives in order`).toBeGreaterThan(cursor);
        cursor = found;
      }
    }
    // A later authored standard declaration still wins, which is why order matters.
    expect(authored.hyphens, "the standard hyphens control").toBe("manual");
  });

  await test.step("the generated Contents list keeps authored labels and hidden points", async () => {
    await navigation(page, "Contents");
    const heading = page.getByRole("tabpanel").locator(`${NAVIGATION_LIST} h3`).first();
    await expect(heading, "an image-only heading uses its alt text").toHaveText("Contents alt");
    await expect(heading).toBeInViewport();

    const text = await page.locator(".epub-navigator-reader__authored-navigation").innerText();
    for (const label of [
      "Image alt",
      "Before picture after",
      "Untitled section",
      "Nested span trailing text",
      "Hidden point",
      "Repeated resource",
    ]) {
      expect(text, `generated label ${label}`).toContain(label);
    }
    expect(text, "an image title must not replace its alt text").not.toContain("Wrong title");
  });

  await test.step("every Contents point activates its authored fragment", async () => {
    for (const [label, fragment] of [
      ["Image alt", "image"],
      ["Before picture after", "mixed"],
      ["Untitled section", "empty"],
      ["Nested span trailing text", "nested"],
      ["Hidden point", "hidden"],
    ] as const) {
      await navigation(page, "Contents");
      await page
        .getByRole("tabpanel")
        .locator(`${NAVIGATION_LIST} button`)
        .filter({ hasText: label })
        .first()
        .click();
      await expect(page.locator(NAVIGATION_LIST)).toHaveCount(0);
      await showsFragment(page, fragment);
      await expect(page.locator(PRIMARY)).toHaveAttribute("data-epub-navigator-occurrence", "2");
    }
  });

  await test.step("the authored page list keeps its heading and targets", async () => {
    await navigation(page, "Pages");
    const heading = page.getByRole("tabpanel").locator(`${NAVIGATION_LIST} h3`).first();
    await expect(heading).toHaveText("Pages alt");
    await expect(heading).toBeInViewport();

    for (const [label, fragment] of [
      ["Page image", "image"],
      ["Page nested trailing", "nested"],
    ] as const) {
      await navigation(page, "Pages");
      await page
        .getByRole("tabpanel")
        .locator(`${NAVIGATION_LIST} button`)
        .filter({ hasText: label })
        .first()
        .click();
      await expect(page.locator(NAVIGATION_LIST)).toHaveCount(0);
      await showsFragment(page, fragment);
    }
  });

  expect(
    createHash("sha256").update(readFileSync(fixture)).digest("hex"),
    "the imported archive must be unchanged",
  ).toBe(digest);
  expect(application.exceptions).toEqual([]);
  expect(application.frameFailures).toEqual([]);
});
