import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  appShell,
  expect,
  importPublication,
  publicationCard,
  publicationFrame,
  returnToLibrary,
  test,
} from "./application";

const sample = resolve(
  fileURLToPath(new URL("../../", import.meta.url)),
  "public/sample/alices-adventures-in-wonderland.epub",
);

test("the native host serves a publication the reader can open and close", async ({
  application,
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 1000 });
  await expect(appShell(page)).toBeVisible();

  await importPublication(page, sample);
  await publicationCard(page).first().click();
  await expect(page.locator('[data-reader-status="ready"]')).toBeVisible();

  const publication = await publicationFrame(page);
  expect(new URL(publication.url()).hostname).not.toBe(new URL(page.url()).hostname);
  await expect(publication.locator("body")).toBeVisible();

  // Closing revokes the reader session a round trip before the page learns of it, so a
  // diagnostics batch scheduled inside that window is still refused. The reading session
  // itself is held to refusing nothing.
  expect(application.httpFailures, "the host must refuse nothing while reading").toEqual([]);

  await returnToLibrary(page);
  expect(application.exceptions).toEqual([]);
  expect(application.frameFailures).toEqual([]);
});
