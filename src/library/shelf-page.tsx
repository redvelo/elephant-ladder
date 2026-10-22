import { AddIcon, Button, NavigatorPaginationFooter } from "epub-navigator/ui";
import { Input } from "@base-ui/react/input";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { type PublicationSummary } from "../host/bootstrap/types";
import { dateTimestamp } from "../lib/format";
import { useElementSize } from "../lib/use-element-size";
import { AppSelect } from "../ui/select";
import { shelfPagerAtom } from "../app/state";
import { bootstrapAtom } from "../app/session";
import { settingsAtom } from "../settings/state";
import type { BookInteractions } from "./actions";
import { BookCover } from "./book-cover";
import { BookMenu, type BookAction } from "./book-menu";
import {
  libraryLanguageAtom,
  libraryPageAtom,
  libraryQueryAtom,
  librarySortAtom,
  normalize,
} from "./filter";
import "./shelf.css";

export function LibraryPage({
  publications,
  onImport,
  portalContainer,
  onBookAction,
  ...actions
}: BookInteractions & {
  publications: readonly PublicationSummary[];
  onImport: () => void;
  portalContainer: HTMLElement | null;
  onBookAction: (action: BookAction, publication: PublicationSummary, trigger: HTMLElement) => void;
}) {
  const settings = useAtomValue(settingsAtom);
  const bootstrap = useAtomValue(bootstrapAtom);
  const [query, setQuery] = useAtom(libraryQueryAtom);
  const [sort, setSort] = useAtom(librarySortAtom);
  const [language, setLanguage] = useAtom(libraryLanguageAtom);
  const [page, setPage] = useAtom(libraryPageAtom);
  const [scrollTop, setScrollTop] = useState(0);
  const [focused, setFocused] = useState<string | null>(null);
  const reachFor = useRef<string | null>(null);
  const awaitingG = useRef(false);
  const viewport = useRef<HTMLDivElement>(null);
  const fontMeasure = useRef<HTMLSpanElement>(null);
  const size = useElementSize(viewport);
  const fontSize = useElementSize(fontMeasure).width || 16;
  const start = useRef<{ x: number; y: number } | null>(null);
  // The page sizes the covers, not the reverse: the height settles how many whole rows a
  // page holds and how tall a cover may be, the width then takes as many as fit, and what
  // is left over becomes the space between rows.
  const scale = fontSize / 16;
  const preferred = (size.width < 500 ? 210 : size.width < 900 ? 270 : 330) * scale;
  const available = Math.max(0, size.height - 4);
  const rows = Math.max(1, Math.round((available + 16) / (preferred + 16)));
  const fitted = Math.min((available - rows * 16) / rows, 360 * scale, size.width * 1.5);
  const columns =
    fitted < 1 ? 1 : Math.max(1, Math.round((size.width + 16) / ((fitted * 2) / 3 + 16)));
  const coverHeight = Math.max(
    0,
    Math.min(fitted, ((size.width - (columns - 1) * 16) / columns) * 1.5),
  );
  const rowGap = Math.max(0, (available - rows * coverHeight) / rows);
  const rowHeight = Math.max(1, coverHeight + rowGap);
  const terms = normalize(query).split(" ").filter(Boolean);
  const books = publications
    .filter((book) => {
      if (language !== "" && book.language !== language) return false;
      const text = normalize(`${book.title} ${book.creators.join(" ")}`);
      return terms.every((term) => text.includes(term));
    })
    .sort((a, b) => {
      if (sort === "title") return a.title.localeCompare(b.title);
      if (sort === "author")
        return (
          a.creators.join(", ").localeCompare(b.creators.join(", ")) ||
          a.title.localeCompare(b.title)
        );
      if (sort === "recent") {
        const recent = dateTimestamp(b.lastReadAt) - dateTimestamp(a.lastReadAt);
        if (recent) return recent;
      }
      return (
        dateTimestamp(b.importedAt) - dateTimestamp(a.importedAt) || a.title.localeCompare(b.title)
      );
    });
  const bookRows = Math.max(1, Math.ceil(books.length / columns));
  const pageHeight = rows * rowHeight;
  const pages = Math.max(1, Math.ceil(bookRows / rows));
  const maxScroll = (pages - 1) * pageHeight;
  const currentPage =
    maxScroll - scrollTop < 1 ? pages - 1 : Math.min(pages - 1, Math.round(scrollTop / pageHeight));
  const firstRow = Math.max(0, Math.floor(scrollTop / rowHeight) - 2);
  const firstBook = firstRow * columns;
  const visible = books.slice(firstBook, firstBook + (rows + 5) * columns);
  const gridStyle = {
    "--book-columns": columns,
    "--cover-height": `${coverHeight}px`,
    "--row-gap": `${rowGap}px`,
    paddingTop: firstRow * rowHeight,
    paddingBottom: Math.max(
      0,
      (pages * rows - firstRow - Math.ceil(visible.length / columns)) * rowHeight + rowGap,
    ),
  } as CSSProperties;
  const languages = [
    ...new Set(publications.flatMap((book) => (book.language === null ? [] : [book.language]))),
  ].sort();
  const showPage = (target: number) => {
    const next = Math.max(0, Math.min(pages - 1, target));
    const top = Math.min(next * pageHeight, maxScroll);
    setPage(next);
    // The rendered window follows the position this asked for rather than the scroll event
    // that reports it, so the books of the new page exist as soon as it is the page.
    setScrollTop(top);
    if (viewport.current !== null) viewport.current.scrollTop = top;
  };
  const movePage = (delta: number) => showPage(currentPage + delta);
  // The device's page buttons reach the shelf through the same pager its footer uses.
  const setShelfPager = useSetAtom(shelfPagerAtom);
  const turnShelf = useRef(movePage);
  useEffect(() => {
    turnShelf.current = movePage;
  });
  useEffect(() => {
    setShelfPager(() => (delta: number) => turnShelf.current(delta));
    return () => setShelfPager(null);
  }, [setShelfPager]);
  const perPage = rows * columns;
  // The shelf follows the reader's keyboard choice: hjkl steer it, gg and G reach its ends.
  const vim = (settings?.preferences ?? bootstrap?.preferences)?.keyboardMode === "vim";
  const focusedIndex = books.findIndex((book) => book.publicationId === focused);
  // The shelf's own tab stop: where focus lands when it arrives from outside.
  const restingIndex = Math.min(currentPage * perPage, Math.max(0, books.length - 1));
  const moveFocus = (to: number) => {
    const next = Math.max(0, Math.min(books.length - 1, to));
    const book = books[next];
    if (book === undefined) return;
    // The book asked for may be on another page, which unmounts the one focus is on. The
    // request is remembered so focus lands once the page holding it has rendered.
    reachFor.current = book.publicationId;
    setFocused(book.publicationId);
    showPage(Math.floor(next / perPage));
  };
  const walkShelf = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const from = focusedIndex < 0 ? restingIndex : focusedIndex;
    // Arrows and paging keys always steer a focused grid: that is how a grid is read with a
    // keyboard, not a shortcut. The letters are the reader's vim mode, so they follow it.
    const steps: Record<string, number | undefined> = {
      ArrowRight: 1,
      ArrowLeft: -1,
      ArrowDown: columns,
      ArrowUp: -columns,
      PageDown: perPage,
      PageUp: -perPage,
      ...(vim ? { l: 1, h: -1, j: columns, k: -columns } : {}),
    };
    const step = steps[event.key];
    const pendingG = awaitingG.current;
    awaitingG.current = false;
    if (step !== undefined) moveFocus(from + step);
    else if (event.key === "Home") moveFocus(0);
    else if (event.key === "End" || (vim && event.key === "G")) moveFocus(books.length - 1);
    else if (vim && event.key === "g") {
      if (pendingG) moveFocus(0);
      else awaitingG.current = true;
    } else return;
    event.preventDefault();
  };
  useLayoutEffect(() => {
    const element = viewport.current;
    if (element === null || page === 0 || pageHeight <= 0 || element.scrollTop !== 0) return;
    element.scrollTop = Math.min(page * pageHeight, maxScroll);
  }, [page, pageHeight, maxScroll]);
  // A book reached by key is focused once the page holding it has rendered.
  useLayoutEffect(() => {
    const wanted = reachFor.current;
    if (wanted === null) return;
    const shelf = viewport.current;
    const book = [
      ...(shelf?.querySelectorAll<HTMLElement>('[data-publication-action="open"]') ?? []),
    ].find((candidate) => candidate.dataset.publicationId === wanted);
    if (book === undefined) return;
    reachFor.current = null;
    book.focus();
  }, [focused, page, firstBook]);

  return (
    <section className="library-page" aria-label="Library">
      <div className="library-toolbar">
        <div className="library-search-row">
          <Input
            type="search"
            aria-label="Search books and authors"
            placeholder="Search books and authors"
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setPage(0);
              setScrollTop(0);
              if (viewport.current !== null) viewport.current.scrollTop = 0;
            }}
          />
          <Button
            variant="ghost"
            size="icon-lg"
            disabled={actions.disabled}
            onClick={onImport}
            aria-label="Import EPUB"
          >
            <AddIcon aria-hidden="true" />
          </Button>
        </div>
        <div className="library-filter-row">
          <AppSelect
            label="Sort books"
            container={portalContainer}
            options={
              [
                { value: "recent", label: "Recently read" },
                { value: "added", label: "Recently added" },
                { value: "title", label: "Title" },
                { value: "author", label: "Author" },
              ] as const
            }
            value={sort}
            onChange={(value) => {
              setSort(value);
              setPage(0);
              setScrollTop(0);
              if (viewport.current !== null) viewport.current.scrollTop = 0;
            }}
          />
          <AppSelect
            label="Language"
            container={portalContainer}
            options={[
              { value: "", label: "All languages" },
              ...languages.map((value) => ({ value, label: value })),
            ]}
            value={language}
            onChange={(value) => {
              setLanguage(value);
              setPage(0);
              setScrollTop(0);
              if (viewport.current !== null) viewport.current.scrollTop = 0;
            }}
          />
          <span className="library-result-count" role="status">
            {books.length.toLocaleString()} books
          </span>
        </div>
      </div>
      <div
        ref={viewport}
        className="library-grid-viewport"
        onScroll={(event) => {
          const top = event.currentTarget.scrollTop;
          setScrollTop(top);
          setPage(
            maxScroll - top < 1 ? pages - 1 : Math.min(pages - 1, Math.round(top / pageHeight)),
          );
        }}
        onTouchStartCapture={(event) => {
          const touch = event.touches[0];
          start.current =
            touch === undefined || event.touches.length !== 1
              ? null
              : { x: touch.clientX, y: touch.clientY };
        }}
        onTouchEndCapture={(event) => {
          const touch = event.changedTouches[0];
          const origin = start.current;
          start.current = null;
          if (touch === undefined || origin === null) return;
          const dx = touch.clientX - origin.x;
          if (Math.abs(dx) > 60 && Math.abs(dx) > Math.abs(touch.clientY - origin.y) * 2)
            movePage(dx < 0 ? 1 : -1);
        }}
      >
        <span ref={fontMeasure} className="library-font-measure" aria-hidden="true" />
        {books.length === 0 ? (
          <p className="library-no-results">No books match your search.</p>
        ) : (
          <ol
            className="book-grid"
            style={gridStyle}
            onKeyDown={walkShelf}
            onFocusCapture={(event) => {
              const id =
                event.target.closest<HTMLElement>("[data-publication-id]")?.dataset.publicationId;
              if (id !== undefined) setFocused(id);
            }}
          >
            {visible.map((book, index) => (
              <li key={book.publicationId}>
                <BookMenu
                  publication={book}
                  sessionToken={actions.sessionToken}
                  disabled={actions.disabled}
                  container={portalContainer}
                  tabIndex={
                    (focusedIndex < 0 ? restingIndex : focusedIndex) === firstBook + index ? 0 : -1
                  }
                  onAction={onBookAction}
                  onOpen={() => actions.onOpen(book)}
                >
                  <BookCover publication={book} sessionToken={actions.sessionToken} />
                </BookMenu>
              </li>
            ))}
          </ol>
        )}
      </div>
      <NavigatorPaginationFooter
        className="library-pagination"
        page={currentPage}
        pageCount={pages}
        pageLabel="Library"
        onPageChange={movePage}
      />
    </section>
  );
}
