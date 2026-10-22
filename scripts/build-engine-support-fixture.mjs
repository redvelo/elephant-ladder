import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { zipSync } from "fflate";

process.env.TZ = "UTC";

const check = process.argv.includes("--check");
const unknownArguments = process.argv.slice(2).filter((argument) => argument !== "--check");
if (unknownArguments.length > 0)
  throw new Error(`Unknown arguments: ${unknownArguments.join(", ")}`);

const root = new URL("../tests/fixtures/engine-support/", import.meta.url);
const source = new URL("elephant-engine-support-src/", root);
const output = new URL("elephant-engine-support.epub", root);
const notice = new URL("NOTICE.md", root);
const paths = [
  "META-INF/container.xml",
  "EPUB/package.opf",
  "EPUB/nav.xhtml",
  "EPUB/probe.xhtml",
  "EPUB/probe.css",
  "EPUB/js/probe.js",
  "EPUB/media/probe.png",
  "EPUB/media/probe.jpg",
  "EPUB/media/probe.gif",
  "EPUB/media/probe.webp",
  "EPUB/media/probe.avif",
  "EPUB/media/probe.jxl",
  "EPUB/media/probe.mp3",
  "EPUB/media/probe.m4a",
  "EPUB/media/probe.opus",
];
const fixedDate = new Date("2026-10-05T00:00:00.000Z");
const mimetypeText = (await readFile(new URL("mimetype", source), "utf8")).trim();
if (mimetypeText !== "application/epub+zip") throw new Error("The EPUB mimetype source is invalid");

const archive = {
  mimetype: [new TextEncoder().encode(mimetypeText), { level: 0, mtime: fixedDate }],
};
for (const path of paths)
  archive[path] = [
    new Uint8Array(await readFile(new URL(path, source))),
    { level: 9, mtime: fixedDate },
  ];

// The probe asks the engine; it must never reach for anything outside the container.
const script = await readFile(new URL("EPUB/js/probe.js", source), "utf8");
if (/(https?|wss?):\/\//u.test(script)) throw new Error("The probe names a remote resource");

// Every media type the probe decodes has to be declared, or a refusal would be the
// manifest's rather than the engine's.
const manifest = await readFile(new URL("EPUB/package.opf", source), "utf8");
for (const [, path] of script.matchAll(/"(media\/probe\.[a-z0-9]+)"/gu)) {
  if (!manifest.includes(`href="${path}"`)) throw new Error(`${path} is not in the manifest`);
}

const bytes = zipSync(archive, { level: 9, mtime: fixedDate });
const digest = createHash("sha256").update(bytes).digest("hex");
const expectedRow = `| \`elephant-engine-support.epub\` | ${bytes.byteLength} | \`${digest}\` |`;

if (check) {
  const committed = new Uint8Array(await readFile(output));
  if (!Buffer.from(committed).equals(Buffer.from(bytes)))
    throw new Error("elephant-engine-support.epub does not match its deterministic source");
  if (!(await readFile(notice, "utf8")).includes(expectedRow))
    throw new Error("NOTICE.md has stale engine-support fixture metadata");
  process.stdout.write(`${fileURLToPath(output)} matches its source (${digest})\n`);
} else {
  await writeFile(output, bytes);
  process.stdout.write(`${fileURLToPath(output)} ${bytes.byteLength} bytes\n${expectedRow}\n`);
}
