import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { zipSync } from "fflate";

process.env.TZ = "UTC";

const check = process.argv.includes("--check");
const unknownArguments = process.argv.slice(2).filter((argument) => argument !== "--check");
if (unknownArguments.length > 0)
  throw new Error(`Unknown arguments: ${unknownArguments.join(", ")}`);

const root = new URL("../tests/fixtures/active-publications/", import.meta.url);
const source = new URL("elephant-remote-policy-src/", root);
const output = new URL("elephant-remote-policy.epub", root);
const notice = new URL("NOTICE.md", root);
const paths = [
  "META-INF/container.xml",
  "EPUB/package.opf",
  "EPUB/nav.xhtml",
  "EPUB/text/probe.xhtml",
  "EPUB/js/probe.js",
];
const fixedDate = new Date("2026-09-10T00:00:00.000Z");
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

const script = await readFile(new URL("EPUB/js/probe.js", source), "utf8");
const remoteUrls = [...script.matchAll(/(https?|wss?):\/\/([^/"\\]+)/gu)];
const authorities = remoteUrls.map((match) => match[2]);
if (authorities.length === 0 || authorities.some((authority) => authority !== "httpbingo.org")) {
  throw new Error("Remote fixture contains an unexpected remote authority");
}
const schemes = new Set(remoteUrls.map((match) => match[1]));
if (!["http", "https", "ws", "wss"].every((scheme) => schemes.has(scheme))) {
  throw new Error("Remote fixture must probe secure and insecure HTTP and WebSocket schemes");
}

const bytes = zipSync(archive, { level: 9, mtime: fixedDate });
assertEpubMimetype(bytes);
const digest = createHash("sha256").update(bytes).digest("hex");

if (check) {
  const committed = new Uint8Array(await readFile(output));
  if (!Buffer.from(committed).equals(Buffer.from(bytes)))
    throw new Error("elephant-remote-policy.epub does not match its deterministic source");
  const noticeText = await readFile(notice, "utf8");
  const expectedRow = `| \`elephant-remote-policy.epub\` | ${bytes.byteLength} | \`${digest}\` |`;
  if (!noticeText.includes(expectedRow))
    throw new Error("NOTICE.md has stale remote-policy fixture metadata");
} else {
  await writeFile(output, bytes);
}

console.log(`${fileURLToPath(output)} ${bytes.byteLength} bytes sha256:${digest}`);

function assertEpubMimetype(value) {
  const view = new DataView(value.buffer, value.byteOffset, value.byteLength);
  const nameLength = view.getUint16(26, true);
  const extraLength = view.getUint16(28, true);
  const name = new TextDecoder().decode(value.subarray(30, 30 + nameLength));
  const dataStart = 30 + nameLength + extraLength;
  const data = new TextDecoder().decode(value.subarray(dataStart, dataStart + 20));
  if (
    view.getUint32(0, true) !== 0x04034b50 ||
    view.getUint16(8, true) !== 0 ||
    name !== "mimetype" ||
    data !== "application/epub+zip"
  ) {
    throw new Error("EPUB mimetype must be the first uncompressed ZIP entry");
  }
}
