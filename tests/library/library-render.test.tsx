// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createStore, Provider } from "jotai";
import { afterEach, expect, test, vi } from "vitest";
import App from "../../src/app/app";
import { Library } from "../../src/library/library";
import { acquisitionAtom, publicationsAtom } from "../../src/library/state";
import { importPublicationAtom } from "../../src/library/import";
import { bootstrapAtom, readerViewAtom } from "../../src/app/session";
import { settingsAtom } from "../../src/settings/state";
import { deferredFetch, libraryBootstrap, publicationSummary } from "../support/state";
import type { Journal, JournalMutation } from "../../src/journal/model";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

/** Settings offers a key or click preference only where a fine pointer reports in. */
function attachedPointer(kind: "coarse" | "fine"): void {
  vi.stubGlobal("matchMedia", (media: string) => {
    const list = new EventTarget() as EventTarget & { media: string; matches: boolean };
    list.media = media;
    list.matches =
      kind === "fine" ? media === "(any-hover: hover)" : media === "(any-pointer: coarse)";
    return list;
  });
}

test("shell shortcuts navigate, focus search, and leave typed text and dialogs alone", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(Response.json([publicationSummary("1", "Keyboard book")]))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await screen.findByRole("heading", { name: "Keyboard book" });
  fireEvent.keyDown(document, { key: "/" });
  const search = await screen.findByRole<HTMLInputElement>("searchbox");
  await waitFor(() => expect(document.activeElement).toBe(search));
  await user.type(search, "a?q/");
  expect(search.value).toBe("a?q/");
  expect(screen.queryByRole("dialog")).toBeNull();
  search.blur();
  fireEvent.keyDown(document, { key: "1", ctrlKey: true });
  expect(screen.getByRole("button", { name: "Home" }).getAttribute("aria-current")).toBe("page");
  fireEvent.keyDown(document, { key: "?" });
  expect(screen.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
  fireEvent.keyDown(document, { key: "2", ctrlKey: true });
  expect(
    screen.getByRole("button", { name: "Home", hidden: true }).getAttribute("aria-current"),
  ).toBe("page");
});

test("shortcut help lives in Settings and restores focus when dismissed", async () => {
  attachedPointer("fine");
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(Response.json([publicationSummary("1")]))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(settingsAtom, {
    revision: 1,
    preferences: libraryBootstrap.preferences,
    device: { sideButtonsTurnPages: false },
  });
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  const navigation = screen.getByRole("navigation", { name: "Main navigation" });
  expect(
    within(navigation)
      .getAllByRole("button")
      .map((button) => button.textContent),
  ).toEqual(["Home", "Library", "Settings"]);
  await user.click(within(navigation).getByRole("button", { name: "Settings" }));
  const trigger = screen.getByRole("button", { name: /Keyboard shortcuts/ });
  await user.click(trigger);
  const help = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  expect(help.classList.contains("shortcut-help")).toBe(true);
  expect(within(help).getByRole("region", { name: "App navigation" })).toBeTruthy();
  expect(within(help).queryByRole("region", { name: "Vim navigation" })).toBeNull();
  await user.click(within(help).getByRole("button", { name: "Close keyboard shortcuts" }));
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the app consumes mouse history buttons even before the library has loaded", () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => new Promise<Response>(() => {})),
  );
  render(
    <Provider store={createStore()}>
      <App />
    </Provider>,
  );
  for (const button of [3, 4]) {
    for (const type of ["mousedown", "mouseup", "auxclick"]) {
      const event = new MouseEvent(type, { button, bubbles: true, cancelable: true });
      document.body.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    }
  }
  const primary = new MouseEvent("mousedown", { button: 0, bubbles: true, cancelable: true });
  document.body.dispatchEvent(primary);
  expect(primary.defaultPrevented).toBe(false);
});

test("re-rendering the library neither refetches it nor republishes its state", async () => {
  const fetch = vi.fn<typeof globalThis.fetch>((input) =>
    Promise.resolve(
      input === "/native/publications"
        ? Response.json([publicationSummary("1", "Moby-Dick"), publicationSummary("2")])
        : new Response(null, { status: 404 }),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  let publicationNotifications = 0;
  let viewNotifications = 0;
  store.sub(publicationsAtom, () => {
    publicationNotifications += 1;
  });
  store.sub(readerViewAtom, () => {
    viewNotifications += 1;
  });

  // A fresh container on every pass keeps the compiler from reusing the previous element.
  const library = () => (
    <Provider store={store}>
      <Library portalContainer={document.createElement("div")} />
    </Provider>
  );
  const { rerender } = render(library());
  await screen.findByRole("heading", { name: "Moby-Dick" });
  for (let pass = 0; pass < 5; pass += 1) rerender(library());

  expect(fetch.mock.calls.filter(([input]) => input === "/native/publications")).toHaveLength(1);
  expect(publicationNotifications).toBe(1);
  expect(viewNotifications).toBe(0);
});

test("Home separates the current book from recent reads and Library retains its search across navigation", async () => {
  const books = [
    {
      ...publicationSummary("1", "Current book"),
      lastReadAt: "2026-09-22 12:00:00",
      hasReadingPosition: true,
    },
    {
      ...publicationSummary("2", "Earlier book"),
      lastReadAt: "2026-09-21 12:00:00",
      hasReadingPosition: true,
    },
    publicationSummary("3", "Unread book"),
  ];
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );

  await screen.findByRole("heading", { name: "Current book" });
  const recents = screen.getByRole("region", { name: "Recently opened" });
  expect(within(recents).getByRole("button", { name: "Read Earlier book" })).toBeTruthy();
  expect(within(recents).queryByRole("button", { name: "Read Current book" })).toBeNull();
  expect(within(recents).queryByRole("button", { name: "Read Unread book" })).toBeNull();

  await user.click(screen.getByRole("button", { name: "Library" }));
  await user.type(screen.getByRole("searchbox"), "Unread");
  expect(screen.getByRole("button", { name: "Read Unread book" })).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByRole("button", { name: /Dictionaries/ })).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "Library" }));
  expect(screen.getByRole<HTMLInputElement>("searchbox").value).toBe("Unread");
  expect(screen.queryByRole("button", { name: "Read Current book" })).toBeNull();
});

test("book actions require an explicit selection and deletion remains confirmed", async () => {
  const fetch = vi.fn<typeof globalThis.fetch>(() =>
    Promise.resolve(Response.json([publicationSummary("1", "Moby-Dick")])),
  );
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );

  await screen.findByRole("heading", { name: "Moby-Dick" });
  expect(screen.queryByRole("button", { name: "Book actions for Moby-Dick" })).toBeNull();
  await user.click(screen.getByRole("button", { name: "Library" }));
  const overflow = screen.getByRole("button", { name: "Read Moby-Dick" });
  overflow.focus();
  await user.keyboard("{Shift>}{F10}{/Shift}");
  expect(screen.queryByRole("dialog")).toBeNull();
  const menu = await screen.findByRole("menu", { name: "Moby-Dick" });
  expect(within(menu).getByText(/^Added /u)).toBeTruthy();
  expect(within(menu).getByRole("menuitem", { name: "Reflections" })).toBeTruthy();
  expect(within(menu).getByRole("menuitem", { name: "Copy annotations as Markdown" })).toBeTruthy();
  expect(within(menu).getByRole("menuitem", { name: "Permissions…" })).toBeTruthy();
  await user.click(within(menu).getByRole("menuitem", { name: "Delete from device…" }));
  expect(screen.getByRole("heading", { name: "Delete Moby-Dick?" })).toBeTruthy();
  expect(fetch.mock.calls.some(([, init]) => init?.method === "DELETE")).toBe(false);
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.getByRole("button", { name: "Read Moby-Dick" })).toBeTruthy();
});

test("Home shows saved progress and preserves a reflection draft through close and reopen", async () => {
  const book = publicationSummary("1", "Current book");
  const journal: Journal = {
    summary: {
      chapter: "Chapter seven",
      pages: { current: 42, total: 200, accuracy: "calibrated" },
    },
    draft: null,
    entries: [],
  };
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(async (input, init) => {
      if (input === "/native/publications") return Response.json([book]);
      const mutation =
        typeof init?.body === "string" ? (JSON.parse(init.body) as JournalMutation) : null;
      if (mutation?.action === "begin" && journal.draft === null) {
        journal.draft = {
          id: "draft",
          text: "",
          prompt: 0,
          context: journal.summary,
          createdAt: "2026-09-22T12:00:00Z",
          updatedAt: "2026-09-22T12:00:00Z",
        };
      }
      if (mutation?.action === "update" && journal.draft !== null)
        journal.draft.text = mutation.text;
      if (mutation?.action === "publish" && journal.draft !== null) {
        journal.entries.push(journal.draft);
        journal.draft = null;
      }
      return Response.json(journal);
    }),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await screen.findByText("Chapter seven");
  expect(screen.getByText("42 / ≈ 200 pages")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: /Add a reflection/ }));
  const composer = await screen.findByRole("textbox");
  await waitFor(() => expect((composer as HTMLTextAreaElement).disabled).toBe(false));
  await user.type(composer, "Remember this idea");
  await user.click(screen.getByRole("button", { name: "Close" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: /Continue draft/ }));
  await waitFor(() =>
    expect(screen.getByRole<HTMLTextAreaElement>("textbox").value).toBe("Remember this idea"),
  );
  await user.click(screen.getByRole("button", { name: "Save reflection" }));
  await screen.findByRole("heading", { name: "Previous reflections" });
  expect(journal.draft).toBeNull();
  expect(journal.entries[0]?.text).toBe("Remember this idea");
});

test("Library sort and language menus change the visible books", async () => {
  const books = [
    { ...publicationSummary("1", "Zebra"), language: "en" },
    { ...publicationSummary("2", "Apple"), language: "fr" },
  ];
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await screen.findByRole("heading", { name: "Zebra" });
  await user.click(screen.getByRole("button", { name: "Library" }));
  await user.click(screen.getByRole("combobox", { name: "Sort books" }));
  await user.click(await screen.findByRole("option", { name: "Title" }));
  expect(
    screen
      .getAllByRole("button", { name: /^Read / })
      .map((button) => button.getAttribute("aria-label")),
  ).toEqual(["Read Apple", "Read Zebra"]);
  await user.click(screen.getByRole("combobox", { name: "Language" }));
  await user.click(await screen.findByRole("option", { name: "fr" }));
  expect(screen.queryByRole("button", { name: "Read Zebra" })).toBeNull();
  expect(screen.getByRole("button", { name: "Read Apple" })).toBeTruthy();
});

test("a finished notice clears itself while a failure waits to be dismissed", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.resolve(Response.json([publicationSummary("1", "Moby-Dick")]))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  store.set(acquisitionAtom, {
    status: "complete",
    fileName: "moby.epub",
    browserMilliseconds: 12,
    disposition: "created",
    publication: publicationSummary("1", "Moby-Dick"),
  });
  vi.useFakeTimers();
  try {
    render(
      <Provider store={store}>
        <Library portalContainer={document.createElement("div")} />
      </Provider>,
    );
    expect(screen.getByRole("status").textContent).toContain("Imported");

    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(screen.queryByRole("status")).toBeNull();
    expect(store.get(acquisitionAtom)).toStrictEqual({ status: "idle" });

    act(() => {
      store.set(acquisitionAtom, {
        status: "failed",
        fileName: "moby.epub",
        message: "That file is not an EPUB.",
      });
    });
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(screen.getByRole("alert").textContent).toContain("That file is not an EPUB.");

    act(() => {
      screen.getByRole("button", { name: "Dismiss" }).click();
    });
    expect(screen.queryByRole("alert")).toBeNull();
  } finally {
    vi.useRealTimers();
  }
});

test("the welcome starts a new library from the bundled sample", async () => {
  const sample = new Blob([new Uint8Array([80, 75, 3, 4])], { type: "application/epub+zip" });
  const imported: { body: unknown }[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>((input, init) => {
    const path = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (path === "/native/publications") return Promise.resolve(Response.json([]));
    if (path === "/sample/alices-adventures-in-wonderland.epub")
      return Promise.resolve(new Response(sample));
    if (path === "/native/publications/import") {
      imported.push({ body: init?.body });
      return Promise.resolve(
        Response.json({
          disposition: "created",
          publication: publicationSummary("1", "Alice's Adventures in Wonderland"),
        }),
      );
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.createElement("div")} />
    </Provider>,
  );

  await user.click(await screen.findByRole("button", { name: "Open the sample book" }));

  await screen.findByRole("heading", { name: "Alice's Adventures in Wonderland" });
  expect(imported).toHaveLength(1);
  const body = imported[0]?.body;
  expect(body).toBeInstanceOf(File);
  expect((body as File).name).toBe("alices-adventures-in-wonderland.epub");
  expect((body as File).type).toBe("application/epub+zip");
});

test("a publication handed over by the platform is imported and opened", async () => {
  const handed = new Blob([new Uint8Array([80, 75, 3, 4])], { type: "application/epub+zip" });
  const publication = publicationSummary("1", "Handed book");
  let pendingAnswered = false;
  const posted: { body: unknown }[] = [];
  let opened = 0;
  const fetch = vi.fn<typeof globalThis.fetch>((input, init) => {
    const path = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (path === "/native/publications") return Promise.resolve(Response.json([]));
    if (path.startsWith("/native/opened-publications/pending")) {
      if (pendingAnswered) return new Promise<Response>(() => {});
      pendingAnswered = true;
      return Promise.resolve(
        Response.json({
          openId: "o".repeat(64),
          fileName: "handed.epub",
          sizeBytes: 4,
          intent: "read",
        }),
      );
    }
    if (path === `/native/opened-publications/${"o".repeat(64)}/bytes`)
      return Promise.resolve(new Response(handed));
    if (path === "/native/publications/import") {
      posted.push({ body: init?.body });
      return Promise.resolve(Response.json({ disposition: "created", publication }));
    }
    if (path.endsWith("/open")) {
      opened += 1;
      return Promise.resolve(new Response(null, { status: 500 }));
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  render(
    <Provider store={store}>
      <App />
    </Provider>,
  );

  await waitFor(() => expect(posted).toHaveLength(1));
  const body = posted[0]?.body;
  expect(body).toBeInstanceOf(File);
  expect((body as File).name).toBe("handed.epub");
  expect((body as File).type).toBe("application/epub+zip");
  await waitFor(() => expect(opened).toBe(1));
});

test("importing several books reports once for the set and carries on past one that fails", async () => {
  const posted: string[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>((input, init) => {
    const path = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (path === "/native/publications") return Promise.resolve(Response.json([]));
    if (path === "/native/publications/import") {
      const file = init?.body as File;
      posted.push(file.name);
      if (file.name === "broken.epub")
        return Promise.resolve(new Response("not an EPUB", { status: 422 }));
      return Promise.resolve(
        Response.json({
          disposition: posted.length === 1 ? "created" : "existing",
          publication: publicationSummary(String(posted.length), file.name),
        }),
      );
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  const { container } = render(
    <Provider store={store}>
      <Library portalContainer={document.createElement("div")} />
    </Provider>,
  );

  const picker = container.querySelector<HTMLInputElement>(
    'input[data-qualification-action="import-publication"]',
  );
  expect(picker).not.toBeNull();
  await user.upload(picker!, [
    new File(["a"], "first.epub", { type: "application/epub+zip" }),
    new File(["b"], "broken.epub", { type: "application/epub+zip" }),
    new File(["c"], "third.epub", { type: "application/epub+zip" }),
  ]);

  // One that cannot be read never stops the books queued behind it.
  await waitFor(() => expect(posted).toStrictEqual(["first.epub", "broken.epub", "third.epub"]));
  const notice = await screen.findByRole("alert");
  expect(notice.textContent).toContain("Imported 1 book");
  expect(notice.textContent).toContain("1 book already in your library");
  expect(notice.textContent).toContain("1 book could not be read");
});

test("an import of several books counts its way through them", async () => {
  const { fetch, requests } = deferredFetch();
  vi.stubGlobal("fetch", vi.fn(fetch));
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  const { container } = render(
    <Provider store={store}>
      <Library portalContainer={document.createElement("div")} />
    </Provider>,
  );
  requests.find((request) => request.url === "/native/publications")?.respond(Response.json([]));

  const picker = container.querySelector<HTMLInputElement>(
    'input[data-qualification-action="import-publication"]',
  );
  await user.upload(picker!, [
    new File(["a"], "first.epub", { type: "application/epub+zip" }),
    new File(["b"], "second.epub", { type: "application/epub+zip" }),
  ]);

  const notice = await screen.findByRole("status");
  expect(notice.textContent).toContain("Importing first.epub");
  expect(notice.textContent).toContain("1 of 2");

  const first = requests.find((request) => request.url === "/native/publications/import");
  first?.respond(
    Response.json({ disposition: "created", publication: publicationSummary("1", "First") }),
  );
  await waitFor(() => expect(screen.getByRole("status").textContent).toContain("2 of 2"));
  expect(screen.getByRole("status").textContent).toContain("Importing second.epub");
});

test("a dropped publication is kept in the library rather than opened", async () => {
  const dropped = new Blob([new Uint8Array([80, 75, 3, 4])], { type: "application/epub+zip" });
  let pendingAnswered = false;
  let imported = 0;
  let opened = 0;
  const fetch = vi.fn<typeof globalThis.fetch>((input) => {
    const path = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (path === "/native/publications") return Promise.resolve(Response.json([]));
    if (path.startsWith("/native/opened-publications/pending")) {
      if (pendingAnswered) return new Promise<Response>(() => {});
      pendingAnswered = true;
      return Promise.resolve(
        Response.json({
          openId: "d".repeat(64),
          fileName: "dropped.epub",
          sizeBytes: 4,
          intent: "keep",
        }),
      );
    }
    if (path === `/native/opened-publications/${"d".repeat(64)}/bytes`)
      return Promise.resolve(new Response(dropped));
    if (path === "/native/publications/import") {
      imported += 1;
      return Promise.resolve(
        Response.json({
          disposition: "created",
          publication: publicationSummary("1", "Dropped book"),
        }),
      );
    }
    if (path.endsWith("/open")) {
      opened += 1;
      return Promise.resolve(new Response(null, { status: 500 }));
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  render(
    <Provider store={store}>
      <App />
    </Provider>,
  );

  await waitFor(() => expect(imported).toBe(1));
  await screen.findByRole("heading", { name: "Dropped book" });
  expect(opened).toBe(0);
});

test("an import waits out a host that is still closing a book", async () => {
  vi.useFakeTimers();
  try {
    let conflicts = 0;
    const fetch = vi.fn<typeof globalThis.fetch>((input) => {
      const path =
        typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (path === "/native/publications") return Promise.resolve(Response.json([]));
      if (path === "/native/publications/import") {
        conflicts += 1;
        if (conflicts <= 2)
          return Promise.resolve(
            new Response("publication import is unavailable while reading", { status: 409 }),
          );
        return Promise.resolve(
          Response.json({
            disposition: "created",
            publication: publicationSummary("1", "Patient book"),
          }),
        );
      }
      return Promise.resolve(new Response(null, { status: 404 }));
    });
    vi.stubGlobal("fetch", fetch);
    const store = createStore();
    store.set(bootstrapAtom, libraryBootstrap);
    const pending = store.set(
      importPublicationAtom,
      new File(["a"], "patient.epub", { type: "application/epub+zip" }),
    );

    await vi.advanceTimersByTimeAsync(2000);
    await pending;
    expect(conflicts).toBe(3);
    expect(store.get(acquisitionAtom)).toMatchObject({
      status: "complete",
      fileName: "patient.epub",
    });
  } finally {
    vi.useRealTimers();
  }
});

test("two handed books import one after another rather than at once", async () => {
  const book = () => new Response(new Blob([new Uint8Array([80, 75, 3, 4])]));
  const arrivals = [
    { openId: "a".repeat(64), fileName: "first.epub", sizeBytes: 4, intent: "keep" },
    { openId: "b".repeat(64), fileName: "second.epub", sizeBytes: 4, intent: "keep" },
  ];
  let offered = 0;
  let importing = 0;
  let overlapped = false;
  const finish: ((response: Response) => void)[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>((input) => {
    const path = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (path === "/native/publications") return Promise.resolve(Response.json([]));
    if (path.startsWith("/native/opened-publications/pending")) {
      const arrival = arrivals[offered];
      if (arrival === undefined) return new Promise<Response>(() => {});
      offered += 1;
      return Promise.resolve(Response.json(arrival));
    }
    if (path.endsWith("/bytes")) return Promise.resolve(book());
    if (path === "/native/publications/import") {
      importing += 1;
      if (importing > 1) overlapped = true;
      return new Promise<Response>((resolve) => {
        finish.push((response) => {
          importing -= 1;
          resolve(response);
        });
      });
    }
    return Promise.resolve(new Response(null, { status: 404 }));
  });
  vi.stubGlobal("fetch", fetch);
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  render(
    <Provider store={store}>
      <App />
    </Provider>,
  );

  await waitFor(() => expect(finish).toHaveLength(1));
  // The second arrival may be offered while the first is still streaming; it must wait.
  await waitFor(() => expect(offered).toBe(2));
  expect(finish).toHaveLength(1);
  finish[0]?.(
    Response.json({ disposition: "created", publication: publicationSummary("1", "First") }),
  );
  await waitFor(() => expect(finish).toHaveLength(2));
  finish[1]?.(
    Response.json({ disposition: "created", publication: publicationSummary("2", "Second") }),
  );
  await waitFor(() => expect(overlapped).toBe(false));
});

test("the shelf is one tab stop and the keys walk it, turning pages as they go", async () => {
  const books = Array.from({ length: 24 }, (_, index) =>
    publicationSummary(String(index + 1), `Book ${String(index + 1).padStart(2, "0")}`),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await user.click(await screen.findByRole("button", { name: "Library" }));

  const shelf = () => screen.getAllByRole("button", { name: /^Read Book/ });
  await waitFor(() => expect(shelf().length).toBeGreaterThan(1));
  // One resting tab stop, whatever the shelf holds.
  expect(shelf().filter((book) => book.tabIndex === 0)).toHaveLength(1);

  const first = shelf()[0];
  first?.focus();
  expect(document.activeElement).toBe(first);

  await user.keyboard("{ArrowRight}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 02");
  await user.keyboard("{ArrowLeft}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");
  await user.keyboard("{End}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 24");
  await user.keyboard("{Home}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");

  // Arrows never walk off the shelf.
  await user.keyboard("{ArrowLeft}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");
  expect(shelf().filter((book) => book.tabIndex === 0)).toHaveLength(1);
});

test("paging keys carry focus onto the page they turn to", async () => {
  const books = Array.from({ length: 60 }, (_, index) =>
    publicationSummary(String(index + 1), `Book ${String(index + 1).padStart(2, "0")}`),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await user.click(await screen.findByRole("button", { name: "Library" }));
  const first = await screen.findByRole("button", { name: "Read Book 01" });
  first.focus();

  await user.keyboard("{PageDown}");
  const afterTurn = document.activeElement?.getAttribute("aria-label");
  expect(afterTurn).not.toBe("Read Book 01");
  expect(afterTurn).toMatch(/^Read Book \d\d$/);

  await user.keyboard("{PageUp}");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");
});

test("vim keys steer the shelf only when the reader keyboard is set to vim", async () => {
  const books = Array.from({ length: 12 }, (_, index) =>
    publicationSummary(String(index + 1), `Book ${String(index + 1).padStart(2, "0")}`),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  const { rerender } = render(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );
  await user.click(await screen.findByRole("button", { name: "Library" }));
  (await screen.findByRole("button", { name: "Read Book 01" })).focus();

  // The bootstrap's standard keyboard leaves the letters to the page.
  await user.keyboard("l");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");

  store.set(settingsAtom, {
    revision: 1,
    preferences: { ...libraryBootstrap.preferences, keyboardMode: "vim" },
    device: { sideButtonsTurnPages: false },
  });
  rerender(
    <Provider store={store}>
      <Library portalContainer={document.body} />
    </Provider>,
  );

  await user.keyboard("l");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 02");
  await user.keyboard("h");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");
  await user.keyboard("G");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 12");
  await user.keyboard("gg");
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Read Book 01");
});

test("a device page button turns the reader's page and pages the shelf", async () => {
  const books = Array.from({ length: 40 }, (_, index) =>
    publicationSummary(String(index + 1), `Book ${String(index + 1).padStart(2, "0")}`),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <App />
    </Provider>,
  );
  await user.click(await screen.findByRole("button", { name: "Library" }));
  await screen.findByRole("button", { name: "Read Book 01" });

  // The device asks the page directly: this is the hook the activity calls.
  const turnPage = window.__elephantLadderTurnPage;
  expect(turnPage).toBeTypeOf("function");
  const pageLabel = () => screen.getByRole("status", { name: /Library page/ }).textContent;
  const before = pageLabel();
  expect(turnPage?.(1)).toBe(true);
  await waitFor(() => expect(pageLabel()).not.toBe(before));
  expect(turnPage?.(-1)).toBe(true);
  await waitFor(() => expect(pageLabel()).toBe(before));
});

test("a dialog and the settings page take the device page buttons from the shelf", async () => {
  const books = Array.from({ length: 40 }, (_, index) =>
    publicationSummary(String(index + 1), `Book ${String(index + 1).padStart(2, "0")}`),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof globalThis.fetch>(() => Promise.resolve(Response.json(books))),
  );
  const store = createStore();
  store.set(bootstrapAtom, libraryBootstrap);
  const user = userEvent.setup();
  render(
    <Provider store={store}>
      <App />
    </Provider>,
  );
  await user.click(await screen.findByRole("button", { name: "Library" }));
  await screen.findByRole("button", { name: "Read Book 01" });
  const turnPage = window.__elephantLadderTurnPage;
  // Read behind the accessibility tree: a surface over the shelf takes the shelf out of it.
  const shelfPage = () => document.querySelector(".library-pagination output")?.textContent;
  const resting = shelfPage();

  // The dialog's own pages answer the buttons, and the shelf it covers stays where it was.
  await user.keyboard("?");
  await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
  expect(turnPage?.(1)).toBe(true);
  expect(shelfPage()).toBe(resting);

  await user.click(screen.getByRole("button", { name: "Close keyboard shortcuts" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  await user.click(screen.getByRole("button", { name: "Settings" }));
  expect(turnPage?.(1)).toBe(true);
  expect(shelfPage()).toBe(resting);
});
