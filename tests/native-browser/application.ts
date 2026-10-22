import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";
import {
  chromium,
  test as base,
  expect,
  type Browser,
  type Frame,
  type Locator,
  type Page,
} from "@playwright/test";

export { expect };

const root = fileURLToPath(new URL("../../", import.meta.url));
// A build with an explicit `--target` writes under the triple; one without writes straight to
// `debug`. Both are checked so the suite runs wherever the application was built.
const executable = process.platform === "win32" ? "elephant-ladder.exe" : "elephant-ladder";
const hostTriples: Record<string, string | undefined> = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
};
const hostTriple = hostTriples[`${process.platform}-${process.arch}`];
const binary =
  process.env.ELEPHANT_NATIVE_BINARY ??
  [
    ...(hostTriple === undefined
      ? []
      : [resolve(root, "src-tauri/target", hostTriple, "debug", executable)]),
    resolve(root, "src-tauri/target/debug", executable),
  ].find((candidate) => existsSync(candidate)) ??
  resolve(root, "src-tauri/target/debug", executable);

/** `storage.rs` names the application database; `library.sqlite` was its pre-release schema. */
const LIBRARY_DATABASE = "library.v2.sqlite";
const KNOWN_ROUTES = [
  "/native/reading-position",
  "/native/annotations/mutations",
  "/native/client-error",
  "/native/publication-policy",
  "/native/preferences",
  "/native/reader/network-activity",
  "/native/diagnostics/v1",
];
/**
 * A frame reports one terminal outcome: `committed`/`prepared` with `complete`, `cancelled` with
 * `superseded` or `closed`, and `failed` with the reason it failed for. Only the last is a defect,
 * so only those are collected; the reasons are listed to reject an unrecognised one.
 */
const FRAME_FAILURE_REASONS = [
  "load-error",
  "unexpected-navigation",
  "effect-execution-failed",
  "readiness-timeout",
  "layout-conflict",
  "layout-unavailable",
];

export interface HttpFailure {
  readonly status: number;
  readonly route: string;
}

export interface FrameFailure {
  readonly reason: string;
  readonly occurrence: unknown;
  readonly outcome: unknown;
}

export interface AnnotationRecord {
  readonly type: string;
  readonly occurrence: number;
  readonly target: Record<string, number>;
}

export interface ReadingPosition {
  readonly rollProgression: number;
  readonly target: Record<string, number>;
}

export interface NativeData {
  readonly annotations: readonly AnnotationRecord[];
  readonly positions: readonly ReadingPosition[];
}

export interface Bootstrap {
  readonly mode: string;
  readonly sessionToken: string;
  readonly libraryDocumentId: string;
  readonly readerSessionId: string;
  readonly positionRevision?: number;
  readonly initialReadingPosition?: unknown;
  readonly publication?: { readonly mount: { readonly baseUrl: string } };
}

export type PolicyChoice = "default" | "enabled" | "disabled" | "allowed" | "blocked" | null;

export interface PolicyOverrides {
  readonly activeContent: PolicyChoice;
  readonly remoteResources: PolicyChoice;
}

export interface PolicyDocument {
  readonly policy: {
    readonly publication: PolicyOverrides;
    readonly global: unknown;
  };
  readonly effective: { readonly activeContent: string; readonly remoteResources: string };
  readonly relevance: { readonly activeContent: string; readonly remoteResources: string };
  readonly revisions: unknown;
}

export interface Application {
  readonly page: Page;
  /** Isolated XDG/HOME tree the native host wrote everything under. */
  readonly profile: string;
  readonly exceptions: readonly string[];
  readonly httpFailures: readonly HttpFailure[];
  readonly frameFailures: readonly FrameFailure[];
  nativeData(): NativeData;
}

/**
 * Development builds take their debugging port from the environment, so parallel workers never
 * contend for one. A port is reserved per launch rather than per worker, because a worker's
 * previous application can still be releasing its own when the next test starts.
 */
function reservePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = createServer();
    probe.on("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (address === null || typeof address === "string") {
        probe.close(() => reject(new Error("could not reserve a debugging port")));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
}

export const test = base.extend<{ application: Application; page: Page }>({
  application: async ({}, use, testInfo) => {
    // Short enough for the Unix-domain socket paths CEF places under TMPDIR.
    const profile = mkdtempSync(join(tmpdir(), "elephant-native-"));
    for (const name of ["home", "data", "cache", "config", "state", "tmp"]) {
      mkdirSync(join(profile, name));
    }
    const log: string[] = [];
    const port = await reservePort();
    const child = spawn(binary, [], {
      env: {
        ...process.env,
        HOME: join(profile, "home"),
        XDG_DATA_HOME: join(profile, "data"),
        XDG_CACHE_HOME: join(profile, "cache"),
        XDG_CONFIG_HOME: join(profile, "config"),
        XDG_STATE_HOME: join(profile, "state"),
        TMPDIR: join(profile, "tmp"),
        ELEPHANT_DEVTOOLS_PORT: String(port),
        GDK_BACKEND: "x11",
        WINIT_UNIX_BACKEND: "x11",
        ELEPHANT_READER_ASSETS: resolve(root, "dist"),
      },
      stdio: ["ignore", "ignore", "pipe"],
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => log.push(chunk));

    let browser: Browser | null = null;
    try {
      const endpoint = await announcedEndpoint(
        () => log.join(""),
        () => child.exitCode,
      );
      browser = await chromium.connectOverCDP(endpoint);
      const page = await readerPage(browser);
      const exceptions: string[] = [];
      const httpFailures: HttpFailure[] = [];
      const frameFailures: FrameFailure[] = [];
      page.on("pageerror", (error) => exceptions.push(error.message));
      page.on("response", (response) => {
        if (response.status() < 400 || httpFailures.length >= 16) return;
        const { pathname } = new URL(response.url());
        httpFailures.push({
          status: response.status(),
          route: KNOWN_ROUTES.includes(pathname) ? pathname : "redacted",
        });
      });
      page.on("request", (request) => {
        if (new URL(request.url()).pathname !== "/native/diagnostics/v1") return;
        for (const record of diagnosticRecords(request.postData())) {
          if (frameFailures.length >= 12) return;
          if (record.event !== "frameLifecycle" || record.outcome !== "failed") continue;
          if (!FRAME_FAILURE_REASONS.includes(String(record.reason))) continue;
          frameFailures.push({
            reason: String(record.reason),
            occurrence: record.occurrence,
            outcome: record.outcome,
          });
        }
      });
      await page.context().tracing.start({ screenshots: true, snapshots: true });

      await use({
        page,
        profile,
        exceptions,
        httpFailures,
        frameFailures,
        nativeData: () => nativeData(profile),
      });

      await testInfo.attach("observations", {
        body: JSON.stringify({ exceptions, httpFailures, frameFailures }, null, 2),
        contentType: "application/json",
      });
      if (testInfo.status === testInfo.expectedStatus) {
        await page.context().tracing.stop();
      } else {
        const trace = testInfo.outputPath("trace.zip");
        await page.context().tracing.stop({ path: trace });
        await testInfo.attach("trace", { path: trace, contentType: "application/zip" });
        const failure = await page.screenshot({ timeout: 10_000 }).catch(() => null);
        if (failure !== null) {
          await testInfo.attach("failure", { body: failure, contentType: "image/png" });
        }
      }
    } catch (cause) {
      throw new Error(
        `${message(cause)}\n--- application output ---\n${log.join("").slice(-4000)}`,
      );
    } finally {
      // A wedged renderer never answers Browser.close; the process is killed next anyway.
      await Promise.race([
        browser?.close().catch(() => undefined),
        new Promise((done) => setTimeout(done, 10_000)),
      ]);
      child.kill("SIGTERM");
      await Promise.race([
        new Promise((done) => child.once("exit", done)),
        new Promise((done) => setTimeout(done, 5_000)),
      ]);
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      if (testInfo.status === testInfo.expectedStatus) {
        rmSync(profile, { recursive: true, force: true });
      } else {
        testInfo.annotations.push({ type: "application profile", description: profile });
      }
    }
  },
  page: async ({ application }, use) => {
    await use(application.page);
  },
});

/**
 * Waits until a reading can be taken `repeats` times in a row without changing.
 *
 * Roll geometry, native sidecar writes and panel animations all settle asynchronously, so a
 * single accepted reading can still be mid-flight.
 */
export async function stable<T>(
  read: () => Promise<T>,
  accept: (value: T) => boolean = () => true,
  { repeats = 4, timeout = 30_000 } = {},
): Promise<T> {
  let previous: string | null = null;
  let seen = 0;
  let latest: T | null = null;
  await expect
    .poll(
      async () => {
        const value = await read();
        const key = JSON.stringify(value);
        seen = value !== null && accept(value) && key === previous ? seen + 1 : 0;
        previous = key;
        latest = value;
        return seen;
      },
      { timeout, intervals: [150] },
    )
    .toBeGreaterThanOrEqual(repeats);
  return latest as T;
}

export function appShell(page: Page): Locator {
  return page.getByRole("navigation", { name: "Main navigation" });
}

/** The publication document runs in its own cross-origin frame, which attaches after the reader reports ready. */
export async function publicationFrame(page: Page): Promise<Frame> {
  const frames = (): readonly Frame[] =>
    page.frames().filter((frame) => frame !== page.mainFrame() && frame.url().startsWith("http"));
  await expect.poll(() => frames().length).toBeGreaterThan(0);
  return frames()[0]!;
}

/** The publication-capability prompt, which a static publication must never raise. */
export function permissionPrompt(page: Page): Locator {
  return page.getByRole("alertdialog").filter({ hasText: "Publication capability" });
}

export function publicationCard(page: Page): Locator {
  return page.locator("[data-publication-action=open]");
}

export async function importPublication(page: Page, file: string): Promise<void> {
  await page.locator("input[data-qualification-action=import-publication]").setInputFiles(file);
  await expect(publicationCard(page).first()).toBeVisible({ timeout: 120_000 });
}

export async function returnToLibrary(page: Page): Promise<void> {
  await page.getByRole("button", { name: /^Open quick menu/u }).click();
  await page.getByRole("button", { name: /library$/iu }).click();
  await expect(publicationCard(page).first()).toBeVisible();
  await expect(page.locator("iframe")).toHaveCount(0);
  await expect(page.locator("[data-epub-navigator-roll]")).toHaveCount(0);
}

/**
 * A panel the reader is still animating shut still answers to its close control, so whether the
 * toolbar button opens or closes the panel is decided again on every attempt.
 */
export async function openPanel(page: Page, name: "Annotations" | "Navigation"): Promise<void> {
  const close = page.getByRole("button", { name: `Close ${name.toLowerCase()} panel` });
  const open = page.getByRole("button", { name, exact: true });
  await expect(async () => {
    if ((await close.count()) === 0) await open.click();
    await expect(close).toBeVisible({ timeout: 2_000 });
  }).toPass();
}

export async function closeAnnotations(page: Page): Promise<void> {
  const close = page.getByRole("button", { name: "Close annotations panel" });
  await expect(async () => {
    if ((await close.count()) > 0) await close.click();
    await expect(close).toHaveCount(0, { timeout: 2_000 });
    await expect(page.locator("[role=dialog]")).toHaveCount(0, { timeout: 2_000 });
  }).toPass();
}

/**
 * Captures the native bootstrap the host delivers to the reader page.
 *
 * The page is delivered once with a single-use nonce, so the hook has to be installed and the
 * page re-delivered before anything can read it.
 */
export async function captureBootstrap(page: Page): Promise<Bootstrap> {
  await expect(appShell(page)).toBeVisible();
  await page.addInitScript(() => {
    const parse = JSON.parse;
    JSON.parse = function parseAndCapture(this: unknown, ...args: Parameters<typeof parse>) {
      const value = parse(...args) as Bootstrap | null;
      if (value?.sessionToken !== undefined && ["library", "reading"].includes(value.mode)) {
        (window as unknown as { __nativeBootstrap?: Bootstrap }).__nativeBootstrap = value;
      }
      return value;
    };
  });
  await page.reload();
  return delivered(page, "library");
}

/** Opening or closing a publication re-delivers the reader page, so a read can straddle it. */
async function bootstrap(page: Page): Promise<Bootstrap | undefined> {
  try {
    return await page.evaluate(
      () => (window as unknown as { __nativeBootstrap?: Bootstrap }).__nativeBootstrap,
    );
  } catch (cause) {
    const destroyed =
      cause instanceof Error && cause.message.includes("Execution context was destroyed");
    if (destroyed) return undefined;
    throw cause;
  }
}

export async function delivered(page: Page, mode: "library" | "reading"): Promise<Bootstrap> {
  let configuration: Bootstrap | undefined;
  await expect
    .poll(async () => {
      configuration = await bootstrap(page);
      return configuration?.mode;
    })
    .toBe(mode);
  return configuration!;
}

/** Reads, and optionally writes, one publication's policy through the authenticated native route. */
export function policy(
  page: Page,
  publicationId: string,
  overrides?: PolicyOverrides,
): Promise<PolicyDocument> {
  return page.evaluate(
    async ([id, write]) => {
      const configuration = (window as unknown as { __nativeBootstrap: Bootstrap })
        .__nativeBootstrap;
      const path = `/native/publications/${encodeURIComponent(id)}/policy`;
      const headers = {
        Authorization: `Bearer ${configuration.sessionToken}`,
        "X-Elephant-Library-Document": configuration.libraryDocumentId,
      };
      const response = await fetch(path, { headers, cache: "no-store" });
      if (!response.ok) throw new Error("policy read failed");
      const current = (await response.json()) as PolicyDocument;
      if (write === null) return current;
      const saved = await fetch(path, {
        method: "PUT",
        headers: { ...headers, "Content-Type": "application/json" },
        body: JSON.stringify({ baseRevisions: current.revisions, policy: write }),
      });
      if (!saved.ok) throw new Error("policy write failed");
      return saved.json() as Promise<PolicyDocument>;
    },
    [publicationId, overrides ?? null] as const,
  );
}

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function diagnosticRecords(body: string | null): readonly Record<string, unknown>[] {
  if (body === null) return [];
  try {
    const parsed = JSON.parse(body) as { readonly records?: readonly Record<string, unknown>[] };
    return parsed.records ?? [];
  } catch {
    return [];
  }
}

/** The window can still be loading when the debugging endpoint answers. */
async function readerPage(browser: Browser): Promise<Page> {
  const reader = (): Page | undefined =>
    browser
      .contexts()
      .flatMap((context) => context.pages())
      .find((page) => page.url().includes("reader.elephant-ladder.localhost"));
  await expect.poll(() => reader() !== undefined, { timeout: 30_000, intervals: [100] }).toBe(true);
  return reader()!;
}

/**
 * Development builds choose their own debugging port, so the announced endpoint is the only
 * one a driver can rely on; packaged builds announce nothing because they open no port.
 */
async function announcedEndpoint(
  output: () => string,
  exitCode: () => number | null,
): Promise<string> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (exitCode() !== null) throw new Error(`the application exited with code ${exitCode()}`);
    const announced = /DevTools listening on ws:\/\/(127\.0\.0\.1:\d+)\//u.exec(output());
    if (announced !== null) return `http://${announced[1]}`;
    await new Promise((done) => setTimeout(done, 250));
  }
  throw new Error(
    "the application announced no DevTools endpoint: it must be a development build, and no other instance may hold its port",
  );
}

function nativeData(profile: string): NativeData {
  const data = join(profile, "data");
  const databases = readdirSync(data, { recursive: true })
    .map(String)
    .filter((path) => path.endsWith(`/${LIBRARY_DATABASE}`));
  if (databases.length !== 1) throw new Error("expected exactly one isolated library database");
  const database = new DatabaseSync(join(data, databases[0]!), { readOnly: true });
  try {
    return {
      annotations: database
        .prepare(
          "SELECT record_type, occurrence, target_json FROM annotations ORDER BY record_order",
        )
        .all()
        .map((row) => ({
          type: String(row.record_type),
          occurrence: Number(row.occurrence),
          target: JSON.parse(String(row.target_json)) as Record<string, number>,
        })),
      positions: database
        .prepare("SELECT position_json FROM reading_positions")
        .all()
        .map((row) => {
          const position = JSON.parse(String(row.position_json)) as {
            readonly rollProgression: number;
            readonly targetJson: string;
          };
          return {
            rollProgression: position.rollProgression,
            target: JSON.parse(position.targetJson) as Record<string, number>,
          };
        }),
    };
  } finally {
    database.close();
  }
}
