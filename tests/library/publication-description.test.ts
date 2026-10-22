// @vitest-environment jsdom
import { expect, test } from "vitest";
import { descriptionText } from "../../src/library/publication-description";

test("book descriptions decode entities and display inert plain text", () => {
  expect(
    descriptionText(
      "<p>A book&#8217;s description.</p><p>Another paragraph.</p><script>bad()</script><style>body{}</style>",
    ),
  ).toBe("A book’s description. Another paragraph.");
  expect(descriptionText("<img src=x onerror=bad()>Plain text")).toBe("Plain text");
});
