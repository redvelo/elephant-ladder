import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Frame, Page } from "@playwright/test";
import {
  captureBootstrap,
  delivered,
  expect,
  importPublication,
  policy,
  publicationCard,
  returnToLibrary,
  test,
} from "./application";

const fixture = resolve(
  fileURLToPath(new URL("../../", import.meta.url)),
  "tests/fixtures/active-publications/johnfactotum-epub-test.epub",
);
const DESTINATION = 'a[href="https://example.com"]';

interface Dispatch {
  readonly url: string;
  readonly session: string | undefined;
}

interface Prepared {
  readonly url: string;
  readonly origin: string;
  readonly session: string | undefined;
}

async function publicationFrame(page: Page, origin: string): Promise<Frame> {
  await expect.poll(() => page.frames().some((frame) => frame.url().startsWith(origin))).toBe(true);
  const frame = page.frames().find((candidate) => candidate.url().startsWith(origin))!;
  await expect(frame.locator(DESTINATION)).toBeAttached();
  return frame;
}

test("external destinations only reach the browser through one approved dispatch", async ({
  application,
  page,
}) => {
  const dispatches: Dispatch[] = [];
  const prepared: Prepared[] = [];

  // Installed before anything can activate a destination: a dispatch that escapes this route
  // reaches the native host, which opens the real browser.
  await page.route("**/native/open-external-url", async (route) => {
    const request = route.request();
    if (request.method() !== "POST") {
      await route.continue();
      return;
    }
    const body = JSON.parse(request.postData() ?? "{}") as { readonly url: string };
    dispatches.push({ url: body.url, session: request.headers()["x-elephant-reader-session"] });
    await route.fulfill({ status: 204 });
  });
  page.on("response", (response) => {
    if (new URL(response.url()).pathname !== "/native/prepare-external-url") return;
    void response
      .json()
      .then((value: { readonly url: string; readonly origin: string }) => {
        prepared.push({
          ...value,
          session: response.request().headers()["x-elephant-reader-session"],
        });
      })
      .catch(() => {
        // A retired reader can drop its own prepare body; the assertions need one of them.
      });
  });

  await captureBootstrap(page);

  await test.step("import the fixture and allow its active content", async () => {
    await importPublication(page, fixture);
    const id = await publicationCard(page).first().getAttribute("data-publication-id");
    await policy(page, id!, { activeContent: "enabled", remoteResources: "allowed" });
  });

  for (const [width, height] of [
    [1910, 1000],
    [360, 640],
  ] as const) {
    await test.step(`${width}x${height}`, async () => {
      await page.setViewportSize({ width, height });
      await publicationCard(page).first().click();
      const configuration = await delivered(page, "reading");
      const origin = new URL(configuration.publication!.mount.baseUrl).origin;
      const session = configuration.readerSessionId;
      let frame = await publicationFrame(page, origin);

      await test.step("an authored HTTP image is refused by the publication policy", async () => {
        const blocked = await frame.evaluate(async () => {
          const image = document.querySelector<HTMLImageElement>('img[src^="http:"]');
          if (image === null) return null;
          const violated = new Promise<boolean>((done) => {
            document.addEventListener(
              "securitypolicyviolation",
              (event) => {
                if (event.blockedURI === image.src && event.effectiveDirective === "img-src") {
                  done(event.disposition === "enforce");
                }
              },
              { once: true },
            );
          });
          image.src = image.getAttribute("src") ?? "";
          return violated;
        });
        expect(blocked, "the authored HTTP image must raise an enforced violation").toBe(true);
        expect(
          await frame.evaluate(
            () => document.querySelector<HTMLImageElement>('img[src^="http:"]')?.naturalWidth,
          ),
          "the refused image must not load",
        ).toBe(0);
      });

      const activate = async (): Promise<void> => {
        // The reader paginates, so a destination can sit outside the viewport at phone width.
        const destination = frame.locator(DESTINATION);
        try {
          await destination.click({ timeout: 3_000 });
        } catch {
          await destination.dispatchEvent("click");
        }
      };
      const dialog = page.getByRole("alertdialog", { name: "Open in your browser?" });
      const shownUrl = page.locator(".external-navigation-url");

      await test.step("activation prompts without dispatching", async () => {
        const before = dispatches.length;
        await activate();
        await expect(dialog).toBeVisible();
        expect(dispatches.length, "a destination must not dispatch before approval").toBe(before);

        await expect.poll(() => prepared.length).toBeGreaterThan(0);
        const normalized = prepared.at(-1)!;
        expect(normalized.url).toBe("https://example.com/");
        expect(normalized.origin).toBe("https://example.com");
        expect(normalized.session, "the prepare carries the reader session").toBe(session);
        await expect(shownUrl).toHaveText(normalized.url);
        await expect(dialog.locator(".app-dialog-description bdi")).toHaveText(normalized.origin);
      });

      await test.step("the prompt follows the application theme", async () => {
        const application_ = page.locator(".elephant-app");
        const original = await application_.getAttribute("data-navigator-color-scheme");
        const visuals: { readonly background: readonly number[] }[] = [];
        for (const theme of ["light", "dark"]) {
          await application_.evaluate(
            (node, value) => node.setAttribute("data-navigator-color-scheme", value),
            theme,
          );
          const visual = await page.evaluate(() => {
            const app = document.querySelector(".elephant-app")!;
            const dialog = document.querySelector<HTMLElement>("[role=alertdialog]")!;
            const style = getComputedStyle(dialog);
            const inherited = getComputedStyle(app);
            const box = dialog.getBoundingClientRect();
            const context = document.createElement("canvas").getContext("2d")!;
            const rgba = (color: string): number[] => {
              context.clearRect(0, 0, 1, 1);
              context.fillStyle = color;
              context.fillRect(0, 0, 1, 1);
              return [...context.getImageData(0, 0, 1, 1).data];
            };
            const background = rgba(style.backgroundColor);
            const foreground = rgba(style.color);
            const border = rgba(style.borderTopColor);
            return {
              contained: app.contains(dialog),
              inherited: ["--popover", "--popover-foreground", "--border", "--font-sans"].every(
                (token) =>
                  style.getPropertyValue(token).trim() !== "" &&
                  style.getPropertyValue(token) === inherited.getPropertyValue(token),
              ),
              opaque: background[3] === 255 && style.opacity === "1",
              border:
                Number.parseFloat(style.borderTopWidth) >= 1 &&
                style.borderTopStyle === "solid" &&
                border[3] === 255 &&
                border.slice(0, 3).some((value, index) => value !== background[index]),
              foreground:
                foreground[3] === 255 &&
                foreground.slice(0, 3).some((value, index) => value !== background[index]),
              fits:
                box.left >= 0 &&
                box.top >= 0 &&
                box.right <= innerWidth &&
                box.bottom <= innerHeight &&
                dialog.scrollWidth <= dialog.clientWidth + 1,
              background,
            };
          });
          for (const key of [
            "contained",
            "inherited",
            "opaque",
            "border",
            "foreground",
            "fits",
          ] as const) {
            expect(visual[key], `${theme} dialog ${key}`).toBe(true);
          }
          visuals.push(visual);
        }
        expect(visuals[0]!.background, "the prompt must answer the theme").not.toEqual(
          visuals[1]!.background,
        );
        await application_.evaluate((node, value) => {
          if (value === null) node.removeAttribute("data-navigator-color-scheme");
          else node.setAttribute("data-navigator-color-scheme", value);
        }, original);
      });

      await test.step("a pending destination is never replaced or queued", async () => {
        const before = dispatches.length;
        const prepares = prepared.length;
        await frame.evaluate(() => {
          const anchor = document.createElement("a");
          anchor.href = "https://example.org/other";
          document.body.append(anchor);
          for (let index = 0; index < 5; index++) anchor.click();
          anchor.remove();
        });
        await activate();
        await page.waitForTimeout(200);
        await expect(shownUrl).toHaveText("https://example.com/");
        expect(prepared.length, "a pending prompt must not prepare again").toBe(prepares);
        expect(dispatches.length).toBe(before);
      });

      await test.step("Cancel leaves a cooldown that refuses a new prompt", async () => {
        const before = dispatches.length;
        const prepares = prepared.length;
        await page.getByRole("button", { name: "Cancel", exact: true }).click();
        await activate();
        await page.waitForTimeout(1_300);
        await expect(dialog).toHaveCount(0);
        expect(dispatches.length).toBe(before);
        expect(prepared.length).toBe(prepares);
      });

      await test.step("Escape dismisses without dispatching", async () => {
        const before = dispatches.length;
        await activate();
        await expect(dialog).toBeVisible();
        // The prompt takes focus as it opens, and a keystroke before that reaches nothing.
        await expect(dialog.locator(".app-dialog-title")).toBeFocused();
        await page.keyboard.press("Escape");
        await page.waitForTimeout(1_200);
        await expect(dialog).toHaveCount(0);
        expect(dispatches.length).toBe(before);
      });

      await test.step("approval dispatches the shown destination exactly once", async () => {
        const before = dispatches.length;
        await activate();
        await expect(dialog).toBeVisible();
        const displayed = await shownUrl.innerText();
        await page.getByRole("button", { name: "Open in browser", exact: true }).click();
        await expect.poll(() => dispatches.length).toBe(before + 1);
        await activate();
        await page.waitForTimeout(1_300);
        const dispatch = dispatches.at(-1)!;
        expect(dispatch.url).toBe(displayed);
        expect(dispatch.url).toBe("https://example.com/");
        expect(dispatch.session).toBe(session);
        expect(dispatches.length, "approval must not queue a second dispatch").toBe(before + 1);
        await expect(dialog).toHaveCount(0);
      });

      await test.step("an authored window.location is recoverable", async () => {
        const before = dispatches.length;
        const invalidate = async (): Promise<void> => {
          await frame.locator('[onclick^="window.location"]').dispatchEvent("click");
          await expect(page.getByText("Section unavailable")).toBeVisible();
        };
        await invalidate();
        await page.getByRole("button", { name: "Try again", exact: true }).click();
        await expect(page.getByText("Section unavailable")).toHaveCount(0);
        frame = await publicationFrame(page, origin);

        await invalidate();
        await returnToLibrary(page);
        expect(dispatches.length, "recovery must not dispatch").toBe(before);
      });
    });
  }

  expect(application.exceptions).toEqual([]);
  expect(application.frameFailures).toEqual([]);
});
