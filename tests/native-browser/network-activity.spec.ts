import { writeFileSync } from "node:fs";
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
import { scriptedArchive } from "./consent-fixture";

const PROBE = "https://example.com/?elephant-network-smoke=";
const NETWORK_ICON = '[title="Recent network activity"]';
const CHROME_EXPIRY_MS = 30_000;

interface ProbeEvidence {
  requests: number;
  responses: number;
  data: number;
  status: number | null;
  disk: boolean | null;
  worker: boolean | null;
  prefetch: boolean | null;
  ipPresent: boolean | null;
  ipLoopback: boolean | null;
}

interface Snapshot {
  readonly available: boolean;
  readonly last: number | null;
}

function consentPrompt(page: Page, title: string) {
  return page.getByRole("alertdialog").filter({ hasText: title });
}

async function expectPrompt(page: Page, title: string): Promise<void> {
  const prompt = consentPrompt(page, title);
  await expect(prompt).toBeVisible();
  // Neither choice may let the reader start.
  expect(await bootstrapMode(page)).toBe("library");
  await expect(page.locator(".product-reader")).toHaveCount(0);
  const overflowing = await prompt.evaluate((dialog) => {
    // The dialog's paginated overflow adds pager chrome that is absent at one page.
    const parts = [
      dialog,
      ...dialog.querySelectorAll(
        ".app-dialog-title,.app-dialog-description,.app-dialog-actions button",
      ),
    ];
    return parts
      .map((part) => {
        const box = part.getBoundingClientRect();
        const reasons: string[] = [];
        if (box.width <= 0 || box.height <= 0) reasons.push("empty");
        if (box.left < 0 || box.top < 0) reasons.push("before origin");
        if (box.right > innerWidth) reasons.push(`right ${box.right} > ${innerWidth}`);
        if (box.bottom > innerHeight) reasons.push(`bottom ${box.bottom} > ${innerHeight}`);
        if (part.scrollWidth > part.clientWidth + 1) reasons.push("scrolls sideways");
        const name = `${part.tagName.toLowerCase()} ${(part.textContent ?? "").trim().slice(0, 40)}`;
        return reasons.length === 0 ? null : `${name}: ${reasons.join(", ")}`;
      })
      .filter((reason) => reason !== null);
  });
  expect(overflowing, `the ${title} prompt must fit the viewport`).toEqual([]);
}

async function bootstrapMode(page: Page): Promise<string | undefined> {
  return page.evaluate(
    () => (window as unknown as { __nativeBootstrap?: { mode: string } }).__nativeBootstrap?.mode,
  );
}

async function idleLibrary(page: Page): Promise<void> {
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(publicationCard(page).first()).toBeEnabled();
  await delivered(page, "library");
}

/** Opens the single imported publication and waits for whatever the policy makes happen next. */
async function openPublication(page: Page): Promise<void> {
  await idleLibrary(page);
  await publicationCard(page).first().click();
}

async function publicationFrame(page: Page, origin: string): Promise<Frame> {
  await expect.poll(() => page.frames().some((frame) => frame.url().startsWith(origin))).toBe(true);
  return page.frames().find((frame) => frame.url().startsWith(origin))!;
}

/** Reads the authored script marker from inside the publication frame. */
async function readerProbe(page: Page, expected: string | null, remembered = false): Promise<void> {
  if (remembered) await expect(page.getByRole("alertdialog")).toHaveCount(0);
  const configuration = await delivered(page, "reading");
  await expect(page.locator('[data-reader-status="ready"]')).toBeVisible({ timeout: 60_000 });
  const origin = new URL(configuration.publication!.mount.baseUrl).origin;
  const frame = await publicationFrame(page, origin);
  await expect
    .poll(() => frame.evaluate(() => document.documentElement.getAttribute("data-consent-probe")))
    .toBe(expected);
}

test("consent gates authored script and the host observes real remote traffic", async ({
  application,
  page,
}, testInfo) => {
  test.setTimeout(240_000);
  const fixture = testInfo.outputPath("network-smoke.epub");
  writeFileSync(fixture, scriptedArchive);

  await captureBootstrap(page);
  await page.setViewportSize({ width: 360, height: 640 });
  await importPublication(page, fixture);
  const id = (await publicationCard(page).first().getAttribute("data-publication-id"))!;

  const initial = await policy(page, id);
  expect(initial.policy.publication).toEqual({ activeContent: null, remoteResources: null });
  expect(
    [initial.effective.activeContent, initial.effective.remoteResources],
    "a fresh profile must ask for both permissions",
  ).toEqual(["ask", "ask"]);

  const unchanged = async (): Promise<void> => {
    const current = await policy(page, id);
    expect(current.policy.publication).toEqual({ activeContent: null, remoteResources: null });
    expect(current.revisions).toEqual(initial.revisions);
  };

  await test.step("cancelling the first prompt writes nothing", async () => {
    await openPublication(page);
    await expectPrompt(page, "Allow active content?");
    await consentVisuals(page, testInfo);
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await idleLibrary(page);
    await unchanged();
  });

  await test.step("escaping the second prompt writes nothing", async () => {
    await openPublication(page);
    await expectPrompt(page, "Allow active content?");
    await page.getByRole("button", { name: "Allow active content", exact: true }).click();
    await expectPrompt(page, "Allow network access?");
    await unchanged();
    await page.keyboard.press("Escape");
    await idleLibrary(page);
    await unchanged();
  });

  await test.step("denying both keeps the authored script from running", async () => {
    await openPublication(page);
    await expectPrompt(page, "Allow active content?");
    await page.getByRole("button", { name: "Keep disabled", exact: true }).click();
    await expectPrompt(page, "Allow network access?");
    await page.getByRole("button", { name: "Keep blocked", exact: true }).click();
    await readerProbe(page, null);
    await returnToLibrary(page);
    const denied = await policy(page, id);
    expect(denied.policy.publication).toEqual({
      activeContent: "disabled",
      remoteResources: "blocked",
    });
  });

  await test.step("a remembered decision does not prompt again", async () => {
    await openPublication(page);
    await readerProbe(page, null, true);
    await returnToLibrary(page);
  });

  await test.step("allowing only active content runs the script with network blocked", async () => {
    await policy(page, id, { activeContent: null, remoteResources: null });
    await openPublication(page);
    await expectPrompt(page, "Allow active content?");
    await page.getByRole("button", { name: "Allow active content", exact: true }).click();
    await expectPrompt(page, "Allow network access?");
    await page.getByRole("button", { name: "Keep blocked", exact: true }).click();
    await readerProbe(page, "ran");
    await returnToLibrary(page);
    const independent = await policy(page, id);
    expect(independent.policy.publication).toEqual({
      activeContent: "enabled",
      remoteResources: "blocked",
    });
    expect([independent.effective.activeContent, independent.effective.remoteResources]).toEqual([
      "enabled",
      "blocked",
    ]);

    await openPublication(page);
    await readerProbe(page, "ran", true);
    await returnToLibrary(page);
  });

  await test.step("the host observes a real remote request and expires its chrome", async () => {
    await policy(page, id, { activeContent: "enabled", remoteResources: "allowed" });
    await page.setViewportSize({ width: 1400, height: 1000 });
    await openPublication(page);
    const configuration = await delivered(page, "reading");
    await expect(page.locator('[data-reader-status="ready"]')).toBeVisible({ timeout: 60_000 });
    const origin = new URL(configuration.publication!.mount.baseUrl).origin;
    const frame = await publicationFrame(page, origin);

    // Playwright does not surface cache provenance or the peer address, so the probe's own
    // observation is read from a DevTools session on the publication frame itself.
    const session = await page.context().newCDPSession(frame);
    const evidence: ProbeEvidence = {
      requests: 0,
      responses: 0,
      data: 0,
      status: null,
      disk: null,
      worker: null,
      prefetch: null,
      ipPresent: null,
      ipLoopback: null,
    };
    const probes = new Set<string>();
    await session.send("Network.enable");
    session.on("Network.requestWillBeSent", (event) => {
      if (!event.request.url.startsWith(PROBE)) return;
      probes.add(event.requestId);
      evidence.requests++;
    });
    session.on("Network.responseReceived", (event) => {
      if (!probes.has(event.requestId)) return;
      evidence.responses++;
      evidence.status = event.response.status;
      evidence.disk = event.response.fromDiskCache ?? null;
      evidence.worker = event.response.fromServiceWorker ?? null;
      evidence.prefetch = event.response.fromPrefetchCache ?? null;
      evidence.ipPresent = Boolean(event.response.remoteIPAddress);
      evidence.ipLoopback = /^(127\.|\[?::1\]?$)/u.test(event.response.remoteIPAddress ?? "");
    });
    session.on("Network.dataReceived", (event) => {
      if (probes.has(event.requestId) && event.encodedDataLength > 0) evidence.data++;
    });

    const snapshot = (): Promise<Snapshot | null> =>
      page.evaluate(async () => {
        const configuration_ = (
          window as unknown as {
            __nativeBootstrap: { sessionToken: string; readerSessionId: string };
          }
        ).__nativeBootstrap;
        const response = await fetch("/native/reader/network-activity", {
          headers: {
            Authorization: `Bearer ${configuration_.sessionToken}`,
            "X-Elephant-Reader-Session": configuration_.readerSessionId,
          },
          cache: "no-store",
        });
        if (!response.ok) return null;
        const value = (await response.json()) as {
          readonly available: boolean;
          readonly lastObservedAt: number | null;
        };
        return { available: value.available, last: value.lastObservedAt };
      });

    await expect.poll(async () => (await snapshot())?.available).toBe(true);
    const before = await snapshot();

    const fetched = await frame.evaluate(
      (url) =>
        fetch(url + String(Date.now()), { cache: "no-store", mode: "no-cors" }).then(
          () => true,
          () => false,
        ),
      PROBE,
    );
    expect(fetched, "the publication's own HTTPS request must leave the device").toBe(true);

    await expect
      .poll(async () => {
        const current = await snapshot();
        return (
          current?.available === true && current.last !== null && current.last > (before?.last ?? 0)
        );
      })
      .toBe(true);
    await expect
      .poll(() => ({ ...evidence }), { timeout: 30_000 })
      .toMatchObject({
        status: 200,
        disk: false,
        worker: false,
        prefetch: false,
        ipPresent: true,
        ipLoopback: false,
      });
    expect(evidence.requests).toBeGreaterThan(0);
    expect(evidence.responses).toBeGreaterThan(0);

    await expect(page.locator('[data-reader-status="network"]')).toBeVisible();
    await expect(page.locator(NETWORK_ICON)).toBeVisible();

    const observed = (await snapshot())!.last;
    const started = Date.now();
    await expect
      .poll(
        async () => {
          const current = await snapshot();
          expect(current?.available, "the host observation must not change while expiring").toBe(
            true,
          );
          expect(current!.last, "the host timestamp must not refresh from polling").toBe(observed);
          if (Date.now() - started <= CHROME_EXPIRY_MS) return false;
          return (
            (await page.locator('[data-reader-status="ready"]').count()) > 0 &&
            (await page.locator(NETWORK_ICON).count()) === 0
          );
        },
        { timeout: 90_000, intervals: [2_000] },
      )
      .toBe(true);

    await session.detach();
  });

  await test.step("the reading page keeps no Tauri IPC authority", async () => {
    const guarded = await page.evaluate(() => {
      try {
        (
          window as unknown as { __TAURI_INTERNALS__: { postMessage(value: unknown): void } }
        ).__TAURI_INTERNALS__.postMessage({});
        return false;
      } catch (cause) {
        return String(cause).includes("Tauri IPC is disabled");
      }
    });
    expect(guarded).toBe(true);
  });

  expect(application.exceptions).toEqual([]);
  expect(application.frameFailures).toEqual([]);
});

/**
 * The consent prompt is the only thing between a publication and the device, so it is checked
 * for contrast, containment and keyboard focus in every theme the product can be shown in.
 */
async function consentVisuals(page: Page, testInfo: { outputPath(name: string): string }) {
  const app = page.locator(".elephant-app");
  const original = await app.getAttribute("data-navigator-color-scheme");
  const failures: string[] = [];
  try {
    for (const [width, height] of [
      [360, 640],
      [1100, 760],
    ] as const) {
      await page.setViewportSize({ width, height });
      for (const [selected, media, forced] of [
        ["light", "dark", false],
        ["dark", "light", false],
        ["system", "light", false],
        ["system", "dark", false],
        ["system", "light", true],
        ["system", "dark", true],
      ] as const) {
        await page.emulateMedia({
          colorScheme: media,
          forcedColors: forced ? "active" : "none",
        });
        await app.evaluate(
          (node, value) => node.setAttribute("data-navigator-color-scheme", value),
          selected,
        );
        await page.evaluate(() =>
          document.fonts.ready.then(
            () =>
              new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
          ),
        );
        if (forced && !(await page.evaluate(() => matchMedia("(forced-colors: active)").matches))) {
          continue;
        }

        // Real Tab input must reach every action and paint a focus-visible ring on it. The
        // dialog's paginated overflow is focusable too, so Tab does not only visit actions.
        const visited = new Set<number>();
        let unringed = 0;
        let checks = await inspectConsentDialog(page);
        for (let press = 0; press < 10 && visited.size < 3; press++) {
          await page.keyboard.press("Tab");
          // Focus and the React update that paints its ring land after the keystroke.
          await page.waitForTimeout(150);
          checks = await inspectConsentDialog(page);
          if (checks.activeAction < 0) continue;
          if (!checks.keyboardFocus) unringed++;
          visited.add(checks.activeAction);
        }

        const name = `consent-${width}x${height}-${selected}-${media}${forced ? "-forced" : ""}`;
        const image = await page.screenshot({ path: testInfo.outputPath(`${name}.png`) });
        // Forced-color text backplates can disagree with computed foreground and background.
        const painted = await page.evaluate(async (data: string) => {
          const image_ = new Image();
          image_.src = `data:image/png;base64,${data}`;
          await image_.decode();
          const canvas = document.createElement("canvas");
          canvas.width = image_.width;
          canvas.height = image_.height;
          const context = canvas.getContext("2d", { willReadFrequently: true })!;
          context.drawImage(image_, 0, 0);
          const actions = "[role=alertdialog] .app-dialog-actions button";
          return [...document.querySelectorAll(actions)].map((button) => {
            const range = document.createRange();
            range.selectNodeContents(button);
            const box = range.getBoundingClientRect();
            const x = Math.ceil(box.left) + 1;
            const y = Math.ceil(box.top) + 1;
            const width_ = Math.floor(box.right) - x - 1;
            const height_ = Math.floor(box.bottom) - y - 1;
            if (width_ <= 0 || height_ <= 0 || x < 0 || y < 0) return false;
            if (x + width_ > image_.width || y + height_ > image_.height) return false;
            const pixels = context.getImageData(x, y, width_, height_).data;
            const levels: number[] = [];
            for (let index = 0; index < pixels.length; index += 4) {
              levels.push(
                [0.2126, 0.7152, 0.0722].reduce((sum, weight, channel) => {
                  const value = pixels[index + channel]! / 255;
                  return (
                    sum +
                    weight * (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
                  );
                }, 0),
              );
            }
            levels.sort((left, right) => left - right);
            // Ignore isolated edge and antialias pixels; require real ink inside the text box.
            const high = levels[Math.floor(levels.length * 0.95)]!;
            const low = levels[Math.floor(levels.length * 0.05)]!;
            return (high + 0.05) / (low + 0.05) >= 4.5;
          });
        }, image.toString("base64"));

        const required = [
          "contained",
          "inheritedTheme",
          "opaque",
          "border",
          "noShadow",
          "foregroundContrast",
          "bodyContrast",
          "buttonContrast",
          "buttonFont",
          "backdropVisible",
          "geometry",
        ] as const;
        const failed: string[] = required.filter((key) => !checks[key]);
        if (painted.length !== 3 || !painted.every(Boolean)) failed.push("paintedButtonLabels");
        const focusAll = visited.size === 3 && unringed === 0;
        const modeMatches =
          checks.mediaDark === (media === "dark") &&
          checks.forcedColors === forced &&
          (forced ||
            checks.darkSurface ===
              (selected === "system" ? media === "dark" : selected === "dark"));
        if (failed.length > 0 || !focusAll || !modeMatches) {
          failures.push(`${name}: ${failed.join(",")} focusAll=${focusAll} mode=${modeMatches}`);
        }
      }
    }
    expect(failures, "consent prompt presentation").toEqual([]);
  } finally {
    await app.evaluate((node, value) => {
      if (value === null) node.removeAttribute("data-navigator-color-scheme");
      else node.setAttribute("data-navigator-color-scheme", value);
    }, original);
    await page.emulateMedia({ colorScheme: null, forcedColors: null });
    await page.setViewportSize({ width: 360, height: 640 });
  }
}

function inspectConsentDialog(page: Page) {
  return page.evaluate(() => {
    const app = document.querySelector(".elephant-app")!;
    const dialog = document.querySelector<HTMLElement>("[role=alertdialog]")!;
    const style = getComputedStyle(dialog);
    const inherited = getComputedStyle(app);
    const context = document.createElement("canvas").getContext("2d", {
      willReadFrequently: true,
    })!;
    // Canvas resolves modern CSS colors, including oklch, to sRGB for contrast.
    const rgba = (color: string): number[] => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = color;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data].map((value, index) =>
        index === 3 ? value / 255 : value,
      );
    };
    const over = (front: number[], back: number[]): number[] =>
      front.slice(0, 3).map((value, index) => value * front[3]! + back[index]! * (1 - front[3]!));
    const luminance = (color: number[]): number =>
      color
        .slice(0, 3)
        .map((value) => {
          const channel = value / 255;
          return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
        })
        .reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index]!, 0);
    const contrast = (left: number[], right: number[]): number =>
      (Math.max(luminance(left), luminance(right)) + 0.05) /
      (Math.min(luminance(left), luminance(right)) + 0.05);

    const background = rgba(style.backgroundColor);
    const tokens = [
      "--background",
      "--foreground",
      "--popover",
      "--popover-foreground",
      "--muted-foreground",
      "--border",
      "--primary",
      "--primary-foreground",
      "--ring",
      "--font-sans",
    ];
    const buttons = [...dialog.querySelectorAll(".app-dialog-actions button")];
    const texts = [
      ...dialog.querySelectorAll(".app-dialog-kicker,.app-dialog-title,.app-dialog-description"),
    ];
    const probe = document.createElement("span");
    probe.style.fontFamily = "var(--font-sans)";
    app.append(probe);
    const expectedFont = getComputedStyle(probe).fontFamily;
    probe.remove();
    const backdrop = document.querySelector(".app-dialog-backdrop");
    const forced = matchMedia("(forced-colors: active)").matches;
    const box = dialog.getBoundingClientRect();
    const active = document.activeElement;
    const focus = active === null ? null : getComputedStyle(active);
    const outline =
      focus !== null &&
      !["none", "hidden"].includes(focus.outlineStyle) &&
      Number.parseFloat(focus.outlineWidth) >= 1 &&
      rgba(focus.outlineColor)[3]! > 0 &&
      contrast(over(rgba(focus.outlineColor), background), background) >= 3;
    const ringColors = focus?.boxShadow.match(/(?:rgba?|oklch|color)\([^)]*\)/gu) ?? [];
    const shadowRing =
      focus?.boxShadow !== "none" &&
      ringColors.some(
        (color) => rgba(color)[3]! > 0 && contrast(over(rgba(color), background), background) > 1.1,
      ) &&
      /3px/u.test(focus?.boxShadow ?? "");

    return {
      contained: app.contains(dialog) && backdrop !== null && app.contains(backdrop),
      inheritedTheme: tokens.every(
        (token) =>
          style.getPropertyValue(token).trim() !== "" &&
          style.getPropertyValue(token).trim() === inherited.getPropertyValue(token).trim(),
      ),
      opaque: background[3] === 1 && style.opacity === "1",
      border: ["Top", "Right", "Bottom", "Left"].every((side) => {
        const width = style.getPropertyValue(`border-${side.toLowerCase()}-width`);
        const kind = style.getPropertyValue(`border-${side.toLowerCase()}-style`);
        const color = style.getPropertyValue(`border-${side.toLowerCase()}-color`);
        return (
          Number.parseFloat(width) >= 1 &&
          !["none", "hidden"].includes(kind) &&
          rgba(color)[3]! > 0 &&
          contrast(over(rgba(color), background), background) >= 3
        );
      }),
      // A shadow set to fully transparent layers paints nothing; what matters is that none of
      // it is visible, not how the stylesheet spells it.
      noShadow:
        style.boxShadow === "none" ||
        (style.boxShadow.match(/(?:rgba?|oklch|color)\([^)]*\)/gu) ?? []).every(
          (layer) => rgba(layer)[3] === 0,
        ),
      foregroundContrast: contrast(over(rgba(style.color), background), background) >= 4.5,
      bodyContrast:
        texts.length === 3 &&
        texts.every(
          (text) =>
            contrast(over(rgba(getComputedStyle(text).color), background), background) >= 4.5,
        ),
      buttonContrast:
        buttons.length === 3 &&
        buttons.every((button) => {
          const own = getComputedStyle(button);
          const surface = over(rgba(own.backgroundColor), background);
          return contrast(over(rgba(own.color), surface), surface) >= 4.5 && own.opacity === "1";
        }),
      buttonFont:
        buttons.length === 3 &&
        buttons.every((button) => getComputedStyle(button).fontFamily === expectedFont) &&
        expectedFont.includes("Inter"),
      backdropVisible:
        forced ||
        (backdrop !== null &&
          rgba(getComputedStyle(backdrop).backgroundColor)[3]! >= 0.4 &&
          getComputedStyle(backdrop).opacity === "1"),
      geometry:
        box.width > 0 &&
        box.height > 0 &&
        box.left >= 0 &&
        box.top >= 0 &&
        box.right <= innerWidth &&
        box.bottom <= innerHeight &&
        dialog.scrollWidth <= dialog.clientWidth + 1,
      activeAction: active === null ? -1 : buttons.indexOf(active),
      keyboardFocus:
        active !== null &&
        buttons.includes(active) &&
        active.matches(":focus-visible") &&
        (outline || shadowRing),
      darkSurface: luminance(background) < 0.5,
      mediaDark: matchMedia("(prefers-color-scheme: dark)").matches,
      forcedColors: forced,
    };
  });
}
