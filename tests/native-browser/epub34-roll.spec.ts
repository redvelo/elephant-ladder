import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Frame, Locator, Page } from "@playwright/test";
import { unzipSync } from "fflate";
import {
  closeAnnotations,
  expect,
  importPublication,
  openPanel,
  permissionPrompt,
  publicationCard,
  returnToLibrary,
  stable,
  test,
} from "./application";

const manifestPath = resolve(
  fileURLToPath(new URL("../../", import.meta.url)),
  "../navigator/tests/fixtures/epub34/manifest.json",
);
const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as {
  readonly fixtures: readonly {
    readonly id: string;
    readonly path: string;
    readonly sha256: string;
  }[];
};

const ROLL = "[data-epub-navigator-roll]";
const ACTIVE =
  '[data-epub-navigator-roll-current="true"] iframe[data-epub-navigator-participant="primary"]';
const BOOKMARK_CARD = ".epub-navigator-reader__annotation-target";
const AUTHORED_WIDTH = 1674;
const AUTHORED_HEIGHT = 1378;
const FRAME_CAP = 12;
const ROLL_URNS = [
  "urn:epub-navigator:roll-progression",
  "urn:epub-navigator:roll-inline-progression",
  "urn:epub-navigator:roll-zoom",
  "urn:epub-navigator:roll-occurrence",
] as const;

interface RollState {
  readonly occurrence: number;
  readonly y: number;
  readonly x: number;
  readonly width: number;
  readonly height: number;
  readonly viewport: number;
  readonly frames: number;
}

interface FrameGeometry {
  readonly occurrence: number;
  readonly top: number;
  readonly bottom: number;
  readonly width: number;
  readonly height: number;
  readonly iw: number;
  readonly ih: number;
}

const hash = (bytes: Uint8Array): string => createHash("sha256").update(bytes).digest("hex");

async function readState(page: Page): Promise<RollState | null> {
  try {
    return await evaluateState(page);
  } catch (cause) {
    // Returning to the library re-delivers the reader page, so a poll can straddle its navigation.
    const destroyed =
      cause instanceof Error && cause.message.includes("Execution context was destroyed");
    if (destroyed) return null;
    throw cause;
  }
}

function evaluateState(page: Page): Promise<RollState | null> {
  return page.evaluate(
    ([roll, active]) => {
      const container = document.querySelector(roll);
      const frame = document.querySelector<HTMLElement>(active);
      if (container === null || frame === null) return null;
      const box = frame.getBoundingClientRect();
      const view = container.getBoundingClientRect();
      return {
        occurrence: Number(frame.dataset.epubNavigatorOccurrence),
        y: Math.max(0, (view.top - box.top) / box.height),
        x: container.scrollLeft,
        width: box.width,
        height: box.height,
        viewport: container.clientWidth,
        frames: document.querySelectorAll("iframe").length,
      };
    },
    [ROLL, ACTIVE] as const,
  );
}

async function settled(
  page: Page,
  accept: (state: RollState) => boolean = () => true,
): Promise<RollState> {
  const state = await stable(
    async () => {
      const value = await readState(page);
      if (value !== null) {
        expect(value.frames, "frames held, including pending ones").toBeLessThanOrEqual(FRAME_CAP);
      }
      return value;
    },
    (value) => value !== null && accept(value),
  );
  return state!;
}

async function wheel(page: Page, deltaY: number): Promise<void> {
  await expect(
    page.locator("[role=dialog]"),
    "a modal dialog cannot be wheeled through",
  ).toHaveCount(0);
  const box = await page.locator(ROLL).boundingBox();
  if (box === null) throw new Error("the roll container is not laid out");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, deltaY);
}

/** Only the selected tab's panel is mounted, so entries are read from it rather than the document. */
function navigationEntries(page: Page): Locator {
  return page.getByRole("tabpanel").locator(".epub-navigator-reader__navigation-list button");
}

/** The panel animates, so opening it and selecting a tab is retried as one step. */
async function navigation(page: Page, tab: "Contents" | "Pages"): Promise<void> {
  const trigger = page.getByRole("tab", { name: tab, exact: true });
  await expect(async () => {
    await openPanel(page, "Navigation");
    await trigger.click({ timeout: 3_000 });
    await expect(trigger).toHaveAttribute("aria-selected", "true", { timeout: 2_000 });
  }).toPass();
}

async function openQuickMenu(page: Page): Promise<void> {
  await page.getByRole("button", { name: /^Open quick menu/u }).click();
}

async function frameFor(page: Page, url: string): Promise<Frame> {
  await expect.poll(() => page.frames().some((frame) => frame.url() === url)).toBe(true);
  return page.frames().find((frame) => frame.url() === url)!;
}

for (const fixture of manifest.fixtures) {
  test(`fixed-layout roll: ${fixture.id}`, async ({ application, page }) => {
    const archivePath = resolve(dirname(manifestPath), fixture.path);
    const source = readFileSync(archivePath);
    expect(hash(source), "pinned official archive").toBe(fixture.sha256);
    const archive = unzipSync(source);

    await page.setViewportSize({ width: 1400, height: 1000 });

    await test.step("import and open the official archive", async () => {
      await importPublication(page, archivePath);
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

    const initial = await test.step("width fit and adjacent frames", async () => {
      const geometry = await stable(
        () =>
          page.evaluate(
            ([roll]) =>
              [
                ...document.querySelectorAll<HTMLIFrameElement>(
                  `${roll} iframe[data-epub-navigator-participant="primary"]`,
                ),
              ]
                .map((frame) => {
                  const box = frame.getBoundingClientRect();
                  return {
                    occurrence: Number(frame.dataset.epubNavigatorOccurrence),
                    top: box.top,
                    bottom: box.bottom,
                    width: box.width,
                    height: box.height,
                    iw: frame.clientWidth,
                    ih: frame.clientHeight,
                  };
                })
                .sort((left, right) => left.occurrence - right.occurrence),
            [ROLL] as const,
          ),
        (frames: readonly FrameGeometry[]) =>
          frames.length >= 2 &&
          frames.every((frame) => frame.iw === AUTHORED_WIDTH && frame.ih === AUTHORED_HEIGHT),
      );
      const state = await settled(page);
      for (const frame of geometry) {
        expect(Math.abs(frame.width - state.viewport), "authored width fit").toBeLessThan(2);
        expect(
          Math.abs(frame.width / frame.height - AUTHORED_WIDTH / AUTHORED_HEIGHT),
          "authored aspect ratio",
        ).toBeLessThan(0.002);
      }
      for (let index = 1; index < geometry.length; index++) {
        const previous = geometry[index - 1]!;
        const current = geometry[index]!;
        if (current.occurrence !== previous.occurrence + 1) continue;
        expect(Math.abs(current.top - previous.bottom), "gap between adjacent frames").toBeLessThan(
          1,
        );
      }
      return state;
    });

    await test.step("wheeling across the spine retains the original frame", async () => {
      const original = await page.locator(ACTIVE).elementHandle();
      await wheel(page, initial.height + 100);
      await settled(page, (state) => state.occurrence > initial.occurrence);
      await wheel(page, -(initial.height + 100));
      await settled(page, (state) => state.occurrence === initial.occurrence);
      expect(
        await original?.evaluate((node) => node.isConnected),
        "the returned-to frame must be the retained one",
      ).toBe(true);
    });

    await test.step("authored Contents and Pages overlays", async () => {
      await navigation(page, "Contents");
      const titles = await navigationEntries(page).allInnerTexts();
      const authored = ["Title page", ..."ABCDEFGH".split("").map((letter) => `Letter ${letter}`)];
      expect(titles).toHaveLength(authored.length);
      for (const [index, title] of authored.entries()) {
        expect(titles[index]?.trim().startsWith(title), `authored title ${title}`).toBe(true);
      }
      await navigationEntries(page).nth(7).click();
      await settled(page, (state) => state.occurrence === 7);

      await navigation(page, "Pages");
      const entries = await navigationEntries(page).count();
      if (entries < 2) {
        // The official archives carry no authored page-list, so activation cannot be qualified.
        await expect(page.getByText("Pages unavailable.")).toBeVisible();
        await page.getByRole("button", { name: "Close navigation panel" }).click();
      } else {
        await navigationEntries(page).nth(0).click();
        await settled(page, (state) => state.occurrence === 0);
      }
    });

    await test.step("native raw images and direct image wrappers", async () => {
      const occurrence = fixture.id.endsWith("mixed") ? 3 : fixture.id.endsWith("-svg") ? 1 : 0;
      const direct = fixture.id.includes("in-spine") || fixture.id.endsWith("mixed");
      await navigation(page, "Contents");
      await navigationEntries(page).nth(occurrence).click();
      await settled(page, (state) => state.occurrence === occurrence);

      const source = await page.locator(ACTIVE).getAttribute("src");
      if (source === null) throw new Error("the active frame has no source");
      if (direct) {
        expect(
          new URL(source).searchParams.get("epub-navigator-image-document"),
          "direct image wrapper query",
        ).toBe("1");
      }

      const frame = await frameFor(page, source);
      const image = await stable(
        () =>
          frame.evaluate(() => {
            const img = document.querySelector("img");
            const svg = document.querySelector<SVGImageElement>("image");
            if (img?.complete === true && img.naturalWidth > 0) {
              return {
                url: img.currentSrc,
                width: img.naturalWidth,
                height: img.naturalHeight,
                fallback: /unsupported|failed to load/iu.test(document.body?.innerText ?? ""),
              };
            }
            if (svg !== null) {
              return {
                // An authored SVG image can carry either `href` or `xlink:href`.
                url: new URL(svg.href.baseVal, document.baseURI).href,
                width: 0,
                height: 0,
                fallback: false,
              };
            }
            return null;
          }),
        (value) => value !== null,
        { repeats: 1 },
      );
      if (image === null) throw new Error("the publication frame loaded no image");
      if (direct) {
        expect(image.width, "native image width").toBe(AUTHORED_WIDTH);
        expect(image.height, "native image height").toBe(AUTHORED_HEIGHT);
        expect(image.fallback, "native image fallback").toBe(false);
      }

      const member = Object.keys(archive).find((path) =>
        decodeURIComponent(new URL(image.url).pathname).endsWith(`/${path}`),
      );
      expect(member?.endsWith(".png"), "raw image maps to an unchanged archive PNG").toBe(true);
      expect(new URL(image.url).search, "inner image requests the queryless raw PNG").toBe("");

      // Same-origin read inside the publication frame: no credentials, headers or bodies leave it.
      const raw = await frame.evaluate(async (url) => {
        const response = await fetch(url);
        const buffer = await response.arrayBuffer();
        const view = new DataView(buffer);
        const digest = await crypto.subtle.digest("SHA-256", buffer);
        return {
          status: response.status,
          mime: response.headers.get("content-type")?.split(";")[0] ?? null,
          bytes: buffer.byteLength,
          signature: [...new Uint8Array(buffer, 0, 8)],
          width: view.getUint32(16),
          height: view.getUint32(20),
          sha256: [...new Uint8Array(digest)]
            .map((byte) => byte.toString(16).padStart(2, "0"))
            .join(""),
        };
      }, image.url);
      const bytes = archive[member!]!;
      expect(raw.status).toBe(200);
      expect(raw.mime).toBe("image/png");
      expect(raw.bytes).toBe(bytes.length);
      expect(raw.sha256, "served bytes are the exact ZIP member").toBe(hash(bytes));
      expect(raw.signature, "PNG signature").toEqual([137, 80, 78, 71, 13, 10, 26, 10]);
      expect(raw.width, "PNG IHDR width").toBe(AUTHORED_WIDTH);
      expect(raw.height, "PNG IHDR height").toBe(AUTHORED_HEIGHT);
    });

    const bookmarks = await test.step("zoom and two same-page bookmarks", async () => {
      await openQuickMenu(page);
      await page.getByRole("button", { name: "Adjust fixed-page zoom" }).click();
      await page.getByRole("button", { name: "Zoom in", exact: true }).click();
      await settled(page, (state) => Math.abs(state.width / state.viewport - 1.25) < 0.002);
      await page.getByRole("button", { name: "Reset zoom to fit" }).click();
      await settled(page, (state) => Math.abs(state.width / state.viewport - 1) < 0.002);
      await page.getByRole("button", { name: "Zoom in", exact: true }).click();
      await settled(page, (state) => Math.abs(state.width / state.viewport - 1.25) < 0.002);
      await page.getByRole("button", { name: "Close zoom controls" }).click();

      await wheel(page, 80);
      await openPanel(page, "Annotations");
      const first = await settled(page);
      await page.getByRole("button", { name: "Add bookmark at current location" }).click();
      await expect(page.locator(BOOKMARK_CARD)).toHaveCount(1);
      await closeAnnotations(page);

      await wheel(page, 120);
      const second = await settled(page);
      expect(second.occurrence, "both bookmarks sit on the same page").toBe(first.occurrence);
      expect(second.y - first.y, "the two bookmarks need distinct offsets").toBeGreaterThan(0.03);

      await openPanel(page, "Annotations");
      await page.getByRole("button", { name: "Add bookmark at current location" }).click();
      await expect(page.locator(BOOKMARK_CARD)).toHaveCount(2);
      await closeAnnotations(page);
      await settled(page);
      return [first, second] as const;
    });

    await test.step("the native sidecar matches the embedded progression", async () => {
      const [, second] = bookmarks;
      const position = await stable(
        () => {
          const { positions } = application.nativeData();
          return Promise.resolve(positions.length === 1 ? positions[0]! : null);
        },
        (value) =>
          value !== null &&
          value.target[ROLL_URNS[3]] === second.occurrence &&
          Math.abs(value.rollProgression - second.y) < 0.002 &&
          value.target[ROLL_URNS[2]] === 1.25,
      );
      if (position === null) throw new Error("the native reading position never settled");
      const sidecar = position.rollProgression;
      const embedded = position.target[ROLL_URNS[0]]!;
      const ulp = Math.max(Number.MIN_VALUE, 2 ** (Math.floor(Math.log2(Math.abs(embedded))) - 52));
      expect(
        Math.abs(embedded - sidecar),
        "sidecar and embedded progression within one f64 ULP",
      ).toBeLessThanOrEqual(ulp);
    });

    await test.step("bookmarks restore offset and zoom after a library reopen", async () => {
      const [first, second] = bookmarks;
      await closeAnnotations(page);
      await returnToLibrary(page);
      await publicationCard(page).first().click();
      await settled(
        page,
        (state) =>
          state.occurrence === second.occurrence &&
          Math.abs(state.y - second.y) < 0.015 &&
          Math.abs(state.width - second.width) < 2 &&
          Math.abs(state.x - second.x) < 2,
      );

      const restored: RollState[] = [];
      for (let index = 0; index < 2; index++) {
        if ((await page.locator(BOOKMARK_CARD).count()) === 0) await openPanel(page, "Annotations");
        await page.locator(BOOKMARK_CARD).nth(index).click();
        await settled(page);
        // Compact activation closes itself; the desktop panel can stay open.
        await closeAnnotations(page);
        restored.push(await settled(page));
      }

      const expected = [first, second].toSorted((left, right) => left.y - right.y);
      const actual = restored.toSorted((left, right) => left.y - right.y);
      for (const [index, target] of expected.entries()) {
        const value = actual[index]!;
        expect(value.occurrence, "restored occurrence").toBe(target.occurrence);
        expect(Math.abs(value.y - target.y), "restored offset").toBeLessThan(0.015);
        expect(Math.abs(value.width - target.width), "restored zoom").toBeLessThan(2);
        expect(Math.abs(value.x - target.x), "restored pan").toBeLessThan(2);
      }
      expect(
        actual[1]!.y - actual[0]!.y,
        "restored bookmarks collapsed to one offset",
      ).toBeGreaterThan(0.03);
    });

    await test.step("native persisted target policy", async () => {
      const { annotations, positions } = application.nativeData();
      expect(annotations).toHaveLength(2);
      expect(annotations.every((record) => record.type === "bookmark")).toBe(true);

      const stored = annotations.toSorted(
        (left, right) => (left.target[ROLL_URNS[0]] ?? 0) - (right.target[ROLL_URNS[0]] ?? 0),
      );
      const expected = [bookmarks[0], bookmarks[1]].toSorted((left, right) => left.y - right.y);
      for (const [index, record] of stored.entries()) {
        for (const urn of ROLL_URNS) expect(typeof record.target[urn], urn).toBe("number");
        const capture = expected[index]!;
        expect(record.target[ROLL_URNS[3]], "stored occurrence").toBe(capture.occurrence);
        expect(record.occurrence, "the row's own occurrence column").toBe(capture.occurrence);
        expect(
          Math.abs((record.target[ROLL_URNS[0]] ?? 0) - capture.y),
          "stored progression",
        ).toBeLessThan(0.002);
        expect(record.target[ROLL_URNS[2]], "stored zoom").toBe(1.25);
        expect(
          Math.abs((record.target[ROLL_URNS[1]] ?? 0) - capture.x / capture.width),
          "stored pan",
        ).toBeLessThan(0.002);
      }
      expect(
        Math.abs((stored[0]?.target[ROLL_URNS[0]] ?? 0) - (stored[1]?.target[ROLL_URNS[0]] ?? 0)),
        "stored bookmarks collapsed same-page progression",
      ).toBeGreaterThan(0.03);

      expect(positions).toHaveLength(1);
      const position = positions[0]!;
      for (const urn of ROLL_URNS) expect(typeof position.target[urn], urn).toBe("number");
      expect(
        Math.abs((position.target[ROLL_URNS[0]] ?? 0) - position.rollProgression),
        "reading position progression",
      ).toBeLessThan(1e-6);
    });

    await test.step("resize preserves the reading place, closing removes the roll", async () => {
      const before = await settled(page);
      await page.setViewportSize({ width: 1200, height: 900 });
      const after = await settled(page, (state) => state.viewport < before.viewport);
      expect(after.occurrence, "occurrence across resize").toBe(before.occurrence);
      expect(Math.abs(after.y - before.y), "progression across resize").toBeLessThan(0.015);
      await returnToLibrary(page);
    });

    expect(hash(readFileSync(archivePath)), "the archive must be unchanged").toBe(fixture.sha256);
    expect(application.exceptions).toEqual([]);
    expect(application.frameFailures).toEqual([]);
  });
}
