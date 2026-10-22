import { writeFileSync } from "node:fs";
import type { CDPSession, Locator, Page } from "@playwright/test";
import {
  captureBootstrap,
  delivered,
  expect,
  importPublication,
  policy,
  publicationCard,
  returnToLibrary,
  test,
  type PolicyDocument,
} from "./application";
import { scriptedArchive, staticArchive } from "./consent-fixture";

const DIALOG = "Book permissions";
const FIELDS = ["Active content", "Network access"] as const;
type Field = (typeof FIELDS)[number];

/** What the native host reports it could assess, independently of what the reader chose. */
const ASSESSMENT = {
  "Active content": {
    detected: "Active content detected.",
    "not-detected": "No active content detected.",
    unknown: "Could not fully assess active content.",
  },
  "Network access": {
    detected: "Network use detected.",
    "not-detected": "No network use detected.",
    unknown: "Could not fully assess network use.",
  },
} as const;

interface NativeCalls {
  put: number;
  get: number;
  open: number;
}

/** A policy response the test is holding, so a save or load can be inspected mid-flight. */
interface PausedResponse {
  readonly requestId: string;
  readonly request: { readonly method: string };
  readonly responseStatusCode?: number;
}

function dialog(page: Page): Locator {
  return page.getByRole("dialog", { name: DIALOG });
}

function choice(page: Page, field: Field): Locator {
  return dialog(page).getByRole("radiogroup", { name: field });
}

function options(page: Page, field: Field): Locator {
  return choice(page, field).getByRole("radio");
}

async function chosen(page: Page, field: Field): Promise<string> {
  const checked = options(page, field).and(page.locator('[aria-checked="true"]'));
  return (await checked.innerText()).trim();
}

async function chooseOption(page: Page, field: Field, label: string): Promise<void> {
  await options(page, field).filter({ hasText: label }).first().click();
  await expect.poll(() => chosen(page, field)).toBe(label);
}

async function assessmentText(page: Page, field: Field): Promise<string> {
  const key = field === "Active content" ? "activeContent" : "remoteResources";
  return (await dialog(page).locator(`[data-permission-assessment="${key}"]`).innerText()).trim();
}

/** The home page features only the most recent book; the library destination lists them all. */
async function showLibrary(page: Page): Promise<void> {
  const tab = page
    .getByRole("navigation", { name: "Main navigation" })
    .getByRole("button", { name: "Library", exact: true });
  if ((await tab.getAttribute("aria-current")) !== "page") await tab.click();
  await expect(tab).toHaveAttribute("aria-current", "page");
}

/** The home page renders a featured card and a list, so only the shown one can be used. */
function cardFor(page: Page, publicationId: string): Locator {
  return page
    .locator(`[data-publication-action=open][data-publication-id="${publicationId}"]`)
    .filter({ visible: true })
    .first();
}

async function openPermissions(page: Page, publicationId: string): Promise<void> {
  await showLibrary(page);
  const card = cardFor(page, publicationId);
  await expect(card).toBeEnabled();
  await card.click({ button: "right" });
  await page.getByRole("menuitem").filter({ hasText: "Permissions" }).first().click();
  await expect(dialog(page)).toBeVisible();
}

/** The dismiss control is labelled Cancel until something has been saved, then Close. */
async function closeDialog(page: Page, how: "button" | "Escape"): Promise<void> {
  if (how === "Escape") await page.keyboard.press("Escape");
  else
    await dialog(page)
      .getByRole("button", { name: /^(Cancel|Close)$/u })
      .first()
      .click();
  await expect(dialog(page)).toHaveCount(0);
}

async function save(page: Page): Promise<void> {
  await dialog(page).getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog(page).getByText("Permissions saved.")).toBeVisible();
}

test("book permissions are reviewed, staged and saved without touching anything else", async ({
  application,
  page,
}, testInfo) => {
  test.setTimeout(240_000);
  const scripted = testInfo.outputPath("permissions-scripted.epub");
  const unscripted = testInfo.outputPath("permissions-static.epub");
  writeFileSync(scripted, scriptedArchive);
  writeFileSync(unscripted, staticArchive);

  const calls: NativeCalls = { put: 0, get: 0, open: 0 };
  page.on("request", (request) => {
    const { pathname } = new URL(request.url());
    if (/^\/native\/publications\/[^/]+\/open$/u.test(pathname)) calls.open++;
    if (!/^\/native\/publications\/[^/]+\/policy$/u.test(pathname)) return;
    if (request.method() === "PUT") calls.put++;
    if (request.method() === "GET") calls.get++;
  });

  // One policy response can be held back, so a committed write or a pending read can be
  // inspected while the dialog is still waiting for it. The request itself is never replayed:
  // the host authorizes it by the browser's own origin and fetch context.
  let hold: "PUT" | "GET" | null = null;
  let held: PausedResponse | null = null;
  let releaseFailed = false;
  const statuses: number[] = [];
  const session: CDPSession = await page.context().newCDPSession(page);
  await session.send("Fetch.enable", {
    patterns: [{ urlPattern: "*/native/publications/*/policy", requestStage: "Response" }],
  });
  session.on("Fetch.requestPaused", (event) => {
    const paused = event as unknown as PausedResponse;
    if (paused.request.method === "PUT" && paused.responseStatusCode !== undefined) {
      statuses.push(paused.responseStatusCode);
    }
    if (hold === paused.request.method) {
      hold = null;
      held = paused;
      return;
    }
    void session
      .send("Fetch.continueRequest", { requestId: paused.requestId })
      .catch(() => (releaseFailed = true));
  });
  const substitute = async (paused: PausedResponse, status: number): Promise<void> => {
    await session.send("Fetch.fulfillRequest", {
      requestId: paused.requestId,
      responseCode: status,
      responseHeaders: [{ name: "Content-Type", value: "application/json" }],
      body: Buffer.from("{}").toString("base64"),
    });
  };

  await captureBootstrap(page);
  await importPublication(page, scripted);
  const first = (await publicationCard(page).first().getAttribute("data-publication-id"))!;
  await page
    .locator("input[data-qualification-action=import-publication]")
    .setInputFiles(unscripted);
  // The home page features one book and lists them all, so cards outnumber publications.
  const publicationIds = async (): Promise<string[]> => [
    ...new Set(
      await publicationCard(page).evaluateAll((nodes) =>
        nodes.map((node) => (node as HTMLElement).dataset.publicationId ?? ""),
      ),
    ),
  ];
  await expect.poll(async () => (await publicationIds()).length).toBe(2);
  const second = (await publicationIds()).find((id) => id !== first)!;

  const initial = await policy(page, first);
  const secondInitial = await policy(page, second);

  await test.step("the native review keeps its four keys and its own assessment", async () => {
    expect(Object.keys(initial).sort()).toEqual(["effective", "policy", "relevance", "revisions"]);
    expect(initial.relevance, "the scripted and remote fixture").toEqual({
      activeContent: "detected",
      remoteResources: "detected",
    });
    expect(secondInitial.relevance, "the static fixture").toEqual({
      activeContent: "not-detected",
      remoteResources: "not-detected",
    });
  });

  await test.step("reviewing never reopens the publication and never writes", async () => {
    const before = { ...calls };
    await openPermissions(page, first);
    await expect(dialog(page).locator(".app-dialog-title")).toBeFocused();
    for (const field of FIELDS) {
      await expect(options(page, field)).toHaveCount(3);
      expect(await assessmentText(page, field)).toBe(`Assessment: ${ASSESSMENT[field].detected}`);
    }
    expect(await chosen(page, "Active content")).toBe("App default");
    expect(await chosen(page, "Network access")).toBe("App default");
    // The library must not act behind an open modal.
    await expect(page.locator(".library-shell")).toHaveAttribute("aria-busy", "true");
    await expect(
      page.locator("input[data-qualification-action=import-publication]"),
    ).toBeDisabled();
    expect(calls.open, "reviewing must not call native open").toBe(before.open);
    expect(calls.put).toBe(before.put);
  });

  await test.step("edits stage until Save, and Cancel discards them", async () => {
    const before = calls.put;
    await chooseOption(page, "Active content", "Allow");
    await chooseOption(page, "Network access", "Block");
    expect(calls.put, "a staged edit must not write").toBe(before);
    await closeDialog(page, "button");
    expect(calls.put).toBe(before);
    expect(await policy(page, first)).toEqual(initial);
  });

  await test.step("each book saves its own overrides", async () => {
    await openPermissions(page, first);
    await chooseOption(page, "Active content", "Allow");
    await chooseOption(page, "Network access", "Block");
    await save(page);
    expect((await policy(page, first)).policy.publication).toEqual({
      activeContent: "enabled",
      remoteResources: "blocked",
    });
    expect(await policy(page, second), "the other book must be untouched").toEqual(secondInitial);
    await closeDialog(page, "button");

    await openPermissions(page, second);
    await chooseOption(page, "Active content", "Disable");
    await chooseOption(page, "Network access", "Allow");
    await save(page);
    await closeDialog(page, "button");

    await openPermissions(page, first);
    expect(await chosen(page, "Active content")).toBe("Allow");
    expect(await chosen(page, "Network access")).toBe("Block");
    await closeDialog(page, "button");
  });

  await test.step("a staged reset that is not saved changes nothing", async () => {
    for (const exit of ["button", "Escape"] as const) {
      const before = { put: calls.put, document: await policy(page, first) };
      await openPermissions(page, first);
      await dialog(page).getByRole("button", { name: "Reset to app defaults" }).click();
      expect(await chosen(page, "Active content")).toBe("App default");
      expect(await chosen(page, "Network access")).toBe("App default");
      await closeDialog(page, exit);
      expect(calls.put).toBe(before.put);
      expect(await policy(page, first)).toEqual(before.document);
    }
  });

  await test.step("a concurrent change makes the save retry and merge", async () => {
    await openPermissions(page, first);
    await chooseOption(page, "Network access", "Allow");
    await chooseOption(page, "Network access", "Block");
    await chooseOption(page, "Active content", "Disable");
    await policy(page, first, { activeContent: "enabled", remoteResources: "allowed" });
    const from = statuses.length;
    await save(page);
    expect(statuses.slice(from), "one conflict then one commit").toEqual([409, 200]);
    expect(await chosen(page, "Active content")).toBe("Disable");
    expect(
      await chosen(page, "Network access"),
      "the untouched field keeps the concurrent change",
    ).toBe("Allow");
    expect((await policy(page, first)).policy.publication).toEqual({
      activeContent: "disabled",
      remoteResources: "allowed",
    });
    await closeDialog(page, "button");
  });

  await test.step("a committed write with a lost response recovers authoritatively", async () => {
    await openPermissions(page, first);
    await chooseOption(page, "Active content", "Allow");
    await chooseOption(page, "Network access", "Block");
    const beforePut = calls.put;
    const beforeGet = calls.get;
    hold = "PUT";
    await dialog(page).getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(() => held !== null).toBe(true);
    expect(held!.responseStatusCode, "the native write must have committed").toBe(200);

    const form = "#publication-permissions-form";
    const controls = dialog(page).locator(`${form} button, ${form} [role=radio]`);
    for (const control of await controls.all()) await expect(control).toBeDisabled();
    // The header close affordance stays clickable, so it must refuse the close itself.
    await dialog(page).getByRole("button", { name: "Close", exact: true }).first().click();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Enter");
    await expect(dialog(page), "a pending write cannot be closed").toBeVisible();
    await expect(dialog(page).getByText("Permissions saved.")).toHaveCount(0);
    expect(calls.put, "a pending write must not duplicate").toBe(beforePut + 1);

    // The write really happened; only its answer is replaced.
    await substitute(held!, 503);
    held = null;
    await expect(dialog(page).getByText("The save could not be confirmed.")).toBeVisible();
    await expect(dialog(page).getByText("Current saved permissions are shown")).toBeVisible();
    await expect(dialog(page).getByText("Permissions saved.")).toHaveCount(0);
    await expect.poll(() => chosen(page, "Active content")).toBe("Allow");
    expect(await chosen(page, "Network access")).toBe("Block");
    expect(calls.get, "recovery must re-read the saved permissions").toBeGreaterThan(beforeGet);
    expect(calls.put, "recovery must not retry the write").toBe(beforePut + 1);
    expect((await policy(page, first)).policy.publication).toEqual({
      activeContent: "enabled",
      remoteResources: "blocked",
    });
    await closeDialog(page, "button");
  });

  await test.step("a late read cannot contaminate another book", async () => {
    hold = "GET";
    await openPermissions(page, first);
    await expect.poll(() => held !== null).toBe(true);
    const late = held!;
    held = null;
    await closeDialog(page, "Escape");

    await openPermissions(page, second);
    await expect.poll(() => options(page, "Active content").count()).toBe(3);
    await session.send("Fetch.continueRequest", { requestId: late.requestId }).catch(() => {
      // Closing the dialog can abort the paused request before it can be answered.
    });
    await page.waitForTimeout(300);
    expect(await chosen(page, "Active content")).toBe("Disable");
    expect(await chosen(page, "Network access")).toBe("Allow");
    await closeDialog(page, "button");
  });

  await test.step("a failed read hides the controls and Retry loading recovers", async () => {
    hold = "GET";
    await openPermissions(page, first);
    await expect.poll(() => held !== null).toBe(true);
    await substitute(held!, 503);
    held = null;
    const retry = dialog(page).getByRole("button", { name: "Retry loading" });
    await expect(retry).toBeVisible();
    await expect(options(page, "Active content"), "no stale controls").toHaveCount(0);
    await retry.click();
    await expect.poll(() => chosen(page, "Active content")).toBe("Allow");
    await closeDialog(page, "button");
  });

  await test.step("every control is reachable and fits in both themes and sizes", async () => {
    const app = page.locator(".elephant-app");
    const original = await app.getAttribute("data-navigator-color-scheme");
    try {
      for (const [width, height] of [
        [360, 640],
        [1100, 760],
      ] as const) {
        await page.setViewportSize({ width, height });
        for (const theme of ["light", "dark"] as const) {
          await app.evaluate(
            (node, value) => node.setAttribute("data-navigator-color-scheme", value),
            theme,
          );
          await openPermissions(page, first);
          const overflow = await dialog(page).evaluate((node) => {
            const box = node.getBoundingClientRect();
            return (
              node.scrollWidth > node.clientWidth + 1 ||
              document.documentElement.scrollWidth > innerWidth ||
              box.left < 0 ||
              box.right > innerWidth ||
              box.top < 0 ||
              box.bottom > innerHeight
            );
          });
          expect(overflow, `${String(width)} ${theme} overflow`).toBe(false);

          const dark = await dialog(page).evaluate((node) => {
            const context = document.createElement("canvas").getContext("2d")!;
            context.fillStyle = getComputedStyle(node).backgroundColor;
            context.fillRect(0, 0, 1, 1);
            const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
            return red! + green! + blue! < 384;
          });
          expect(dark, `${String(width)} ${theme} surface`).toBe(theme === "dark");

          const reached = new Set<string>();
          await page.keyboard.press("Tab");
          for (let press = 0; press < 14; press++) {
            const focus = await page.evaluate(() => {
              const active = document.activeElement;
              if (active === null) return null;
              const style = getComputedStyle(active);
              const box = active.getBoundingClientRect();
              return {
                name: active.getAttribute("aria-label") ?? active.textContent?.trim() ?? "",
                inDialog: active.closest("[role=dialog]") !== null,
                ring:
                  active.matches(":focus-visible") &&
                  ((style.outlineStyle !== "none" && Number.parseFloat(style.outlineWidth) >= 1) ||
                    style.boxShadow !== "none"),
                onScreen:
                  box.top >= 0 &&
                  box.bottom <= innerHeight &&
                  box.left >= 0 &&
                  box.right <= innerWidth,
              };
            });
            if (focus !== null && focus.inDialog && focus.name !== "") {
              expect(focus.ring, `${focus.name} focus ring`).toBe(true);
              expect(focus.onScreen, `${focus.name} on screen`).toBe(true);
              reached.add(focus.name);
            }
            await page.keyboard.press("Tab");
            await page.waitForTimeout(80);
          }
          expect(
            reached.size,
            `${String(width)} ${theme} reached ${[...reached].join(", ")}`,
          ).toBeGreaterThanOrEqual(4);
          await closeDialog(page, "button");
        }
      }
    } finally {
      await app.evaluate((node, value) => {
        if (value === null) node.removeAttribute("data-navigator-color-scheme");
        else node.setAttribute("data-navigator-color-scheme", value);
      }, original);
      await page.setViewportSize({ width: 1400, height: 1000 });
    }
  });

  const seeded = await test.step("reading seeds website storage the reset must keep", async () => {
    await showLibrary(page);
    await cardFor(page, first).click();
    const configuration = await delivered(page, "reading");
    await expect(page.locator('[data-reader-status="ready"]')).toBeVisible({ timeout: 60_000 });
    const origin = new URL(configuration.publication!.mount.baseUrl).origin;
    await expect(
      page.locator("[data-publication-action=site-data]"),
      "library permissions must not leak into the reader",
    ).toHaveCount(0);

    const frame = page.frames().find((candidate) => candidate.url().startsWith(origin))!;
    const sentinels = await frame.evaluate(async () => {
      localStorage.setItem("permissions-retention", "retained");
      document.cookie = "permissions-retention=retained; SameSite=None; Secure; path=/";
      const database = await new Promise<IDBDatabase>((resolve, reject) => {
        const request = indexedDB.open("permissions-retention", 1);
        request.onupgradeneeded = () => request.result.createObjectStore("sentinel");
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
      try {
        const value = await new Promise((resolve, reject) => {
          const transaction = database.transaction("sentinel", "readwrite");
          const store = transaction.objectStore("sentinel");
          store.put("retained", "marker");
          const read = store.get("marker");
          transaction.oncomplete = () => resolve(read.result);
          transaction.onerror = () => reject(transaction.error);
        });
        return {
          localStorage: localStorage.getItem("permissions-retention") === "retained",
          cookie: document.cookie.split("; ").includes("permissions-retention=retained"),
          indexedDB: value === "retained",
        };
      } finally {
        database.close();
      }
    });
    expect(Object.values(sentinels).every(Boolean), "website storage sentinels").toBe(true);
    await returnToLibrary(page);
    return { origin, position: configuration };
  });

  await test.step("a saved reset keeps everything it does not own", async () => {
    const before = application.nativeData();
    const secondSaved = await policy(page, second);
    await openPermissions(page, first);
    await dialog(page).getByRole("button", { name: "Reset to app defaults" }).click();
    await save(page);
    const reset: PolicyDocument = await policy(page, first);
    expect(reset.policy.publication).toEqual({ activeContent: null, remoteResources: null });
    expect(reset.relevance, "the assessment is the host's, not the reader's").toEqual(
      initial.relevance,
    );
    expect(reset.policy.global).toEqual(initial.policy.global);
    expect(reset.effective).toEqual(initial.effective);
    expect(await policy(page, second)).toEqual(secondSaved);
    expect(application.nativeData(), "reset must keep positions and annotations").toEqual(before);
    await closeDialog(page, "button");

    await openPermissions(page, first);
    expect(await chosen(page, "Active content")).toBe("App default");
    expect(await chosen(page, "Network access")).toBe("App default");
    await closeDialog(page, "button");
  });

  await test.step("the next open asks again and keeps the stored origin", async () => {
    await showLibrary(page);
    await cardFor(page, first).click();
    await page.getByRole("button", { name: "Allow active content", exact: true }).click();
    await page.getByRole("button", { name: "Keep blocked", exact: true }).click();
    const configuration = await delivered(page, "reading");
    await expect(page.locator('[data-reader-status="ready"]')).toBeVisible({ timeout: 60_000 });
    expect(new URL(configuration.publication!.mount.baseUrl).origin, "stored origin").toBe(
      seeded.origin,
    );

    const frame = page.frames().find((candidate) => candidate.url().startsWith(seeded.origin))!;
    const sentinels = await frame.evaluate(async () => {
      const database = await new Promise<IDBDatabase>((resolve, reject) => {
        const request = indexedDB.open("permissions-retention", 1);
        request.onupgradeneeded = () => request.result.createObjectStore("sentinel");
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
      try {
        const value = await new Promise((resolve, reject) => {
          const transaction = database.transaction("sentinel", "readonly");
          const read = transaction.objectStore("sentinel").get("marker");
          transaction.oncomplete = () => resolve(read.result);
          transaction.onerror = () => reject(transaction.error);
        });
        return {
          localStorage: localStorage.getItem("permissions-retention") === "retained",
          cookie: document.cookie.split("; ").includes("permissions-retention=retained"),
          indexedDB: value === "retained",
        };
      } finally {
        database.close();
      }
    });
    expect(sentinels, "a permissions reset must not delete website data").toEqual({
      localStorage: true,
      cookie: true,
      indexedDB: true,
    });
    await returnToLibrary(page);
  });

  expect(releaseFailed, "every held policy response was released").toBe(false);
  await session.send("Fetch.disable");
  await session.detach();
  expect(application.exceptions).toEqual([]);
  expect(application.frameFailures).toEqual([]);
});
