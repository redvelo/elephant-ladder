import { expect, test } from "vitest";

import { resolvedMetadataDirection } from "../../src/host/bootstrap/publication";

test("an element decides its own direction before the package does", () => {
  expect(resolvedMetadataDirection("rtl", "ltr")).toBe("rtl");
  expect(resolvedMetadataDirection("rtl", null)).toBe("rtl");
  expect(resolvedMetadataDirection("ltr", "rtl")).toBe("ltr");
});

test("an explicit auto is a declaration and outranks the package", () => {
  expect(resolvedMetadataDirection("auto", "rtl")).toBe("auto");
});

test("an element that says nothing inherits the package", () => {
  expect(resolvedMetadataDirection(null, "rtl")).toBe("rtl");
  expect(resolvedMetadataDirection(undefined, "ltr")).toBe("ltr");
});

test("silence everywhere states no direction, which is not auto", () => {
  expect(resolvedMetadataDirection(null, null)).toBeNull();
  expect(resolvedMetadataDirection(undefined, undefined)).toBeNull();
});

test("a token outside the three EPUB accepts is passed over, not honoured", () => {
  expect(resolvedMetadataDirection("sideways", null)).toBeNull();
  // An unrecognised element token must not block what the package declared.
  expect(resolvedMetadataDirection("", "rtl")).toBe("rtl");
  expect(resolvedMetadataDirection("sideways", "rtl")).toBe("rtl");
});
