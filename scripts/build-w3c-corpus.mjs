// Builds the W3C EPUB test corpus from one pinned upstream commit and records its
// catalog. The EPUBs themselves live in the user's cache, never in the repository:
// the committed catalog carries every test's identity, metadata, and digest, and
// `--check` proves the cache still matches it.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { zipSync } from "fflate";

export const W3C_CORPUS_SCHEMA = "elephant-w3c-corpus-v1";
export const UPSTREAM_REPOSITORY = "https://github.com/w3c/epub-tests";
export const UPSTREAM_COMMIT = "54092b4233253e9aac80e93ec4782b380b4b3403";
const UPSTREAM_LICENSE = "W3C Software and Document License";
const UPSTREAM_LICENSE_URL = "https://www.w3.org/Consortium/Legal/copyright-software";
// A fixed archive time keeps the digests a property of the upstream bytes alone.
const ARCHIVE_MTIME = new Date("2026-01-01T00:00:00Z");

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const CATALOG_PATH = resolve(root, "tests/fixtures/w3c-corpus/corpus.json");

export function corpusCacheRoot(commit = UPSTREAM_COMMIT) {
  const cache = process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache");
  return join(cache, "elephant-ladder", "w3c-epub-tests", commit);
}

export function corpusEpubPath(id, commit = UPSTREAM_COMMIT) {
  return join(corpusCacheRoot(commit), "epubs", `${id}.epub`);
}

export function readCatalog(path = CATALOG_PATH) {
  const catalog = JSON.parse(readFileSync(path, "utf8"));
  if (catalog.schema !== W3C_CORPUS_SCHEMA || !Array.isArray(catalog.tests))
    throw new Error(`${path} is not a ${W3C_CORPUS_SCHEMA} catalog`);
  return catalog;
}

async function ensureSource(cacheRoot) {
  const source = join(cacheRoot, "source", "tests");
  if (existsSync(source)) return source;
  mkdirSync(cacheRoot, { recursive: true });
  const tarball = join(cacheRoot, "source.tar.gz");
  if (!existsSync(tarball)) {
    const url = `${UPSTREAM_REPOSITORY}/archive/${UPSTREAM_COMMIT}.tar.gz`;
    process.stderr.write(`Fetching ${url}\n`);
    const response = await fetch(url);
    if (!response.ok) throw new Error(`upstream archive answered HTTP ${response.status}`);
    writeFileSync(tarball, Buffer.from(await response.arrayBuffer()));
  }
  mkdirSync(join(cacheRoot, "source"), { recursive: true });
  execFileSync("tar", [
    "-xzf",
    tarball,
    "-C",
    join(cacheRoot, "source"),
    "--strip-components=1",
    `epub-tests-${UPSTREAM_COMMIT}/tests`,
  ]);
  if (!existsSync(source)) throw new Error("the upstream archive did not contain tests/");
  return source;
}

function walk(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...walk(path));
    else if (entry.isFile()) files.push(path);
  }
  return files;
}

function text(markup, tag) {
  const match = markup.match(new RegExp(`<${tag}(?:\\s[^>]*)?>([^<]*)</${tag}>`, "u"));
  return match === null ? null : decode(match[1].trim());
}

function metas(markup, property) {
  const pattern = new RegExp(`<meta\\s+[^>]*property="${property}"[^>]*>([^<]*)</meta>`, "gu");
  return [...markup.matchAll(pattern)].map((match) => decode(match[1].trim()));
}

function decode(value) {
  return value
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&amp;", "&");
}

function packageDocument(testDirectory) {
  const container = readFileSync(join(testDirectory, "META-INF/container.xml"), "utf8");
  const rootfile = container.match(/full-path="([^"]+)"/u);
  if (rootfile === null) throw new Error(`${testDirectory}: container.xml names no rootfile`);
  return { path: rootfile[1], markup: readFileSync(join(testDirectory, rootfile[1]), "utf8") };
}

function buildTest(testDirectory, directoryName) {
  const files = walk(testDirectory)
    .map((path) => relative(testDirectory, path).split(sep).join("/"))
    .sort((left, right) =>
      left === "mimetype" ? -1 : right === "mimetype" ? 1 : left < right ? -1 : 1,
    );
  if (files[0] !== "mimetype") throw new Error(`${directoryName}: no mimetype file`);
  const entries = {};
  for (const path of files) {
    entries[path] = [
      new Uint8Array(readFileSync(join(testDirectory, path))),
      { level: path === "mimetype" ? 0 : 9, mtime: ARCHIVE_MTIME },
    ];
  }
  const bytes = zipSync(entries);
  const opf = packageDocument(testDirectory);
  const uniqueId = opf.markup.match(/unique-identifier="([^"]+)"/u)?.[1] ?? null;
  const identifier =
    (uniqueId === null
      ? null
      : opf.markup.match(
          new RegExp(`<dc:identifier[^>]*id="${uniqueId}"[^>]*>([^<]*)</dc:identifier>`, "u"),
        )?.[1]) ?? text(opf.markup, "dc:identifier");
  if (identifier === null) throw new Error(`${directoryName}: package has no dc:identifier`);
  return {
    id: directoryName,
    identifier: identifier.trim(),
    title: text(opf.markup, "dc:title") ?? directoryName,
    description: text(opf.markup, "dc:description") ?? "",
    coverage: text(opf.markup, "dc:coverage") ?? "",
    collections: metas(opf.markup, "belongs-to-collection"),
    references: metas(opf.markup, "dcterms:isReferencedBy"),
    modified: metas(opf.markup, "dcterms:modified")[0] ?? null,
    scripted: /properties="[^"]*\bscripted\b[^"]*"/u.test(opf.markup),
    remoteResources: /properties="[^"]*\bremote-resources\b[^"]*"/u.test(opf.markup),
    spineLength: (opf.markup.match(/<itemref\b/gu) ?? []).length,
    template: directoryName.startsWith("xx-"),
    bytes: bytes.byteLength,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    archive: bytes,
  };
}

export async function buildCorpus({ check = false } = {}) {
  const cacheRoot = corpusCacheRoot();
  const source = await ensureSource(cacheRoot);
  const epubs = join(cacheRoot, "epubs");
  mkdirSync(epubs, { recursive: true });
  const tests = [];
  for (const entry of readdirSync(source, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const testDirectory = join(source, entry.name);
    if (!existsSync(join(testDirectory, "META-INF/container.xml"))) continue;
    const test = buildTest(testDirectory, entry.name);
    const destination = join(epubs, `${test.id}.epub`);
    if (
      !existsSync(destination) ||
      createHash("sha256").update(readFileSync(destination)).digest("hex") !== test.sha256
    ) {
      writeFileSync(destination, test.archive);
    }
    const { archive: _archive, ...record } = test;
    tests.push(record);
  }
  tests.sort((left, right) => (left.id < right.id ? -1 : 1));
  const duplicate = tests.find(
    (test, index) => tests.findIndex((other) => other.id === test.id) !== index,
  );
  if (duplicate !== undefined) throw new Error(`duplicate test identifier ${duplicate.id}`);
  const catalog = {
    schema: W3C_CORPUS_SCHEMA,
    upstream: {
      repository: UPSTREAM_REPOSITORY,
      commit: UPSTREAM_COMMIT,
      license: UPSTREAM_LICENSE,
      licenseUrl: UPSTREAM_LICENSE_URL,
    },
    archive: {
      mtime: ARCHIVE_MTIME.toISOString(),
      packer: "fflate zipSync, mimetype stored first",
    },
    tests,
  };
  const serialized = `${JSON.stringify(catalog, null, 2)}\n`;
  if (check) {
    if (!existsSync(CATALOG_PATH)) throw new Error(`${CATALOG_PATH} is missing`);
    if (readFileSync(CATALOG_PATH, "utf8") !== serialized) {
      throw new Error(`${CATALOG_PATH} does not match the pinned upstream build`);
    }
  } else {
    mkdirSync(resolve(CATALOG_PATH, ".."), { recursive: true });
    writeFileSync(CATALOG_PATH, serialized);
  }
  return { catalog, epubs, cacheRoot };
}

const invokedDirectly =
  process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  const check = process.argv.includes("--check");
  const unknown = process.argv.slice(2).filter((argument) => argument !== "--check");
  if (unknown.length > 0) {
    process.stderr.write(`unknown arguments: ${unknown.join(" ")}\n`);
    process.exit(2);
  }
  buildCorpus({ check })
    .then(({ catalog, epubs }) => {
      const size = catalog.tests.reduce((total, test) => total + test.bytes, 0);
      process.stdout.write(
        `${check ? "Verified" : "Built"} ${catalog.tests.length} W3C EPUB tests at ${UPSTREAM_COMMIT.slice(0, 12)} (${(size / 1_048_576).toFixed(1)} MiB) under ${epubs}\n`,
      );
      const stat = statSync(CATALOG_PATH);
      process.stdout.write(`Catalog ${relative(root, CATALOG_PATH)} (${stat.size} bytes)\n`);
    })
    .catch((error) => {
      process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
      process.exit(1);
    });
}
