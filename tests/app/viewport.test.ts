import { readFileSync } from "node:fs";
import { expect, test } from "vitest";

test("the compact keyboard overlays without changing the pagination viewport", () => {
  const document = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
  expect(document).toMatch(
    /name="viewport"\s+content="[^"]*interactive-widget=overlays-content[^"]*"/,
  );
});

test("the reader viewport covers display cutouts so chrome can inset itself", () => {
  const document = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
  expect(document).toMatch(/name="viewport"\s+content="[^"]*viewport-fit=cover[^"]*"/);
});
