// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { creatorsDirection } from "../../src/library/metadata-direction";
import { CreatorList, MetadataText } from "../../src/library/metadata-text";

afterEach(cleanup);

test("a declared direction reaches the markup", () => {
  render(<MetadataText text="CSS: הרפתקה חדשה!" direction="rtl" />);
  expect(screen.getByText("CSS: הרפתקה חדשה!").getAttribute("dir")).toBe("rtl");
});

test("an undeclared direction is inferred from the text, not inherited from the app", () => {
  render(<MetadataText text="Plain title" direction={null} />);
  expect(screen.getByText("Plain title").getAttribute("dir")).toBe("auto");
});

test("auto is passed through so the engine infers from the text", () => {
  render(<MetadataText text="Inferred" direction="auto" />);
  expect(screen.getByText("Inferred").getAttribute("dir")).toBe("auto");
});

test("each creator carries its own direction rather than one joined run", () => {
  render(
    <CreatorList
      creators={["Dave Cramer", "Ivan Herman", "Unstated"]}
      directions={["rtl", "ltr", null]}
    />,
  );
  // A right-to-left name written in Latin letters is the case inference gets wrong, so
  // the declaration has to survive to the element.
  expect(screen.getByText("Dave Cramer").getAttribute("dir")).toBe("rtl");
  expect(screen.getByText("Ivan Herman").getAttribute("dir")).toBe("ltr");
  expect(screen.getByText("Unstated").getAttribute("dir")).toBe("auto");
});

test("creators read as one list, separated once between names", () => {
  const { container } = render(
    <CreatorList creators={["First", "Second"]} directions={[null, null]} />,
  );
  expect(container.textContent).toBe("First, Second");
});

test("a creator without a recorded direction is not an error", () => {
  render(<CreatorList creators={["Only"]} directions={[]} />);
  expect(screen.getByText("Only").getAttribute("dir")).toBe("auto");
});

test("a block takes the direction its creators share, since only a block aligns", () => {
  // A name with no right-to-left characters cannot reorder, so the declaration shows
  // only as the line sitting at the right, which the block decides.
  expect(creatorsDirection(["rtl"])).toBe("rtl");
  expect(creatorsDirection(["rtl", "rtl"])).toBe("rtl");
});

test("creators that disagree leave the block to inference", () => {
  expect(creatorsDirection(["rtl", "ltr"])).toBeNull();
  expect(creatorsDirection(["rtl", null])).toBeNull();
  expect(creatorsDirection([null])).toBeNull();
  expect(creatorsDirection([])).toBeNull();
});
