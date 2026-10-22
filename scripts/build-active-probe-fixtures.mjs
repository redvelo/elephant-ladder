import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { unzipSync, zipSync } from "fflate";

process.env.TZ = "UTC";

const check = process.argv.includes("--check");
const unknownArguments = process.argv.slice(2).filter((argument) => argument !== "--check");
if (unknownArguments.length > 0) {
  throw new Error(`Unknown arguments: ${unknownArguments.join(", ")}`);
}

const source = new URL(
  "../tests/fixtures/active-publications/elephant-active-probe-src/",
  import.meta.url,
);
const containerSource = new URL(
  "../tests/fixtures/active-publications/elephant-container-constrained-src/",
  import.meta.url,
);
const notice = new URL("../tests/fixtures/active-publications/NOTICE.md", import.meta.url);
const fixedDate = new Date("2026-09-08T00:00:00.000Z");
const sourceIdentifier = "urn:uuid:4bcbf614-54b9-4b97-a00e-cc465659bb30";
const sourceTitle = "Elephant Active Probe";
const scenarios = [
  "classic-script",
  "document-module-static-import",
  "document-module-dynamic-import",
  "relative-fetch",
  "root-relative-fetch",
  "remote-fetch",
  "dedicated-module-worker",
  "dedicated-worker-teardown",
  "shared-worker",
  "service-worker-registration",
  "stylesheet",
  "image",
  "scripted-svg",
  "wav-range",
  "local-storage",
  "indexed-db",
  "cookie",
  "document-domain-relaxation",
  "reader-isolation",
  "native-capability-isolation",
  "popup",
  "peer-local-storage",
  "peer-indexed-db",
  "peer-cookie",
  "peer-worker-teardown",
  "probe-complete",
  "peer-complete",
];
const entries = [
  "META-INF/container.xml",
  "EPUB/package.opf",
  "EPUB/nav.xhtml",
  "EPUB/text/probe.xhtml",
  "EPUB/text/peer.xhtml",
  "EPUB/styles/probe.css",
  "EPUB/images/probe-image.svg",
  "EPUB/images/scripted-probe.svg",
  "EPUB/js/report.js",
  "EPUB/js/classic.js",
  "EPUB/js/static-dependency.js",
  "EPUB/js/dynamic-dependency.js",
  "EPUB/js/worker-dependency.js",
  "EPUB/js/probe-module.js",
  "EPUB/js/peer-module.js",
  "EPUB/js/dedicated-module-worker.js",
  "EPUB/js/dedicated-lifecycle-worker.js",
  "EPUB/js/shared-module-worker.js",
  "EPUB/js/service-worker.js",
  "EPUB/data/relative.json",
  "EPUB/data/root-relative.json",
];
const variants = [
  {
    filename: "elephant-active-probe.epub",
    identifier: sourceIdentifier,
    title: sourceTitle,
  },
  {
    filename: "elephant-active-probe-twin.epub",
    identifier: "urn:uuid:a4522b3b-ec9e-46e9-846b-f2184562435d",
    title: "Elephant Active Probe Twin",
  },
];
const containerEntries = [
  "META-INF/container.xml",
  "EPUB/package.opf",
  "EPUB/nav.xhtml",
  "EPUB/content.xhtml",
  "EPUB/child.xhtml",
  "EPUB/child.js",
];

const sourceFiles = new Map();
for (const path of entries) {
  sourceFiles.set(path, new Uint8Array(await readFile(new URL(path, source))));
}
const mimetypeText = (await readFile(new URL("mimetype", source), "utf8")).trim();
if (mimetypeText !== "application/epub+zip") throw new Error("The EPUB mimetype source is invalid");
const mimetype = new TextEncoder().encode(mimetypeText);
const wavBase64 = await readFile(new URL("EPUB/media/exact-44-byte.wav.base64", source), "utf8");
const wav = new Uint8Array(Buffer.from(wavBase64.trim(), "base64"));
if (wav.byteLength !== 44)
  throw new Error(`Expected the WAV fixture to be 44 bytes, got ${wav.byteLength}`);
assertRuntimeScenarios();

const generated = variants.map((variant) => generateVariant(variant));
assertTwinContract(generated);
const containerFixture = await generateContainerFixture();
const allFixtures = [...generated, containerFixture];

if (check) {
  const noticeText = await readFile(notice, "utf8");
  for (const fixture of allFixtures) {
    const output = new URL(
      `../tests/fixtures/active-publications/${fixture.filename}`,
      import.meta.url,
    );
    const committed = new Uint8Array(await readFile(output));
    if (!Buffer.from(committed).equals(Buffer.from(fixture.bytes))) {
      throw new Error(`${fixture.filename} does not match its deterministic source`);
    }
    const expectedRow = `| \`${fixture.filename}\` | ${fixture.bytes.byteLength} | \`${fixture.digest}\` |`;
    if (!noticeText.includes(expectedRow)) {
      throw new Error(`NOTICE.md has stale size or SHA-256 data for ${fixture.filename}`);
    }
  }
  const expectedScenarios = `Scenarios (${scenarios.length}): ${scenarios.map((value) => `\`${value}\``).join(", ")}.`;
  if (!noticeText.includes(expectedScenarios)) {
    throw new Error("NOTICE.md does not contain the exact active-probe scenario contract");
  }
} else {
  for (const fixture of allFixtures) {
    const output = new URL(
      `../tests/fixtures/active-publications/${fixture.filename}`,
      import.meta.url,
    );
    await writeFile(output, fixture.bytes);
  }
}

for (const fixture of allFixtures) {
  console.log(
    `${fileURLToPath(new URL(`../tests/fixtures/active-publications/${fixture.filename}`, import.meta.url))} ${fixture.bytes.byteLength} bytes sha256:${fixture.digest}`,
  );
}

async function generateContainerFixture() {
  const mimetypeText = (await readFile(new URL("mimetype", containerSource), "utf8")).trim();
  if (mimetypeText !== "application/epub+zip")
    throw new Error("The container fixture mimetype source is invalid");
  const archiveEntries = {
    mimetype: [new TextEncoder().encode(mimetypeText), { level: 0, mtime: fixedDate }],
  };
  for (const path of containerEntries) {
    archiveEntries[path] = [
      new Uint8Array(await readFile(new URL(path, containerSource))),
      { level: 9, mtime: fixedDate },
    ];
  }
  const bytes = zipSync(archiveEntries, { level: 9, mtime: fixedDate });
  assertEpubMimetype(bytes);
  return {
    filename: "elephant-container-constrained.epub",
    bytes,
    digest: createHash("sha256").update(bytes).digest("hex"),
  };
}

function generateVariant(variant) {
  const packageSource = new TextDecoder().decode(sourceFiles.get("EPUB/package.opf"));
  const packageText = packageSource
    .replace(sourceIdentifier, variant.identifier)
    .replace(`<dc:title>${sourceTitle}</dc:title>`, `<dc:title>${variant.title}</dc:title>`);
  if (packageText === packageSource && variant.identifier !== sourceIdentifier) {
    throw new Error("The twin package identity substitutions were not applied");
  }

  const archiveEntries = {
    mimetype: [mimetype, { level: 0, mtime: fixedDate }],
  };
  for (const path of entries) {
    const bytes =
      path === "EPUB/package.opf" ? new TextEncoder().encode(packageText) : sourceFiles.get(path);
    archiveEntries[path] = [bytes, { level: 9, mtime: fixedDate }];
  }
  archiveEntries["EPUB/media/exact-44-byte.wav"] = [wav, { level: 0, mtime: fixedDate }];

  const bytes = zipSync(archiveEntries, { level: 9, mtime: fixedDate });
  assertEpubMimetype(bytes);
  return {
    ...variant,
    bytes,
    digest: createHash("sha256").update(bytes).digest("hex"),
  };
}

function assertTwinContract(fixtures) {
  const [primary, twin] = fixtures.map((fixture) => unzipSync(fixture.bytes));
  const primaryPaths = Object.keys(primary);
  const twinPaths = Object.keys(twin);
  if (JSON.stringify(primaryPaths) !== JSON.stringify(twinPaths)) {
    throw new Error("Twin archive paths differ from the primary fixture");
  }
  for (const path of primaryPaths) {
    if (path === "EPUB/package.opf") continue;
    if (!Buffer.from(primary[path]).equals(Buffer.from(twin[path]))) {
      throw new Error(`Twin resource differs from the primary fixture: ${path}`);
    }
  }
  const primaryPackage = new TextDecoder().decode(primary["EPUB/package.opf"]);
  const twinPackage = new TextDecoder().decode(twin["EPUB/package.opf"]);
  const normalizedTwin = twinPackage
    .replace(variants[1].identifier, variants[0].identifier)
    .replace(
      `<dc:title>${variants[1].title}</dc:title>`,
      `<dc:title>${variants[0].title}</dc:title>`,
    );
  if (normalizedTwin !== primaryPackage) {
    throw new Error("Twin package differs by more than identifier and title");
  }
}

function assertRuntimeScenarios() {
  const reportSource = new TextDecoder().decode(sourceFiles.get("EPUB/js/report.js"));
  const declaration = reportSource.match(
    /export const SCENARIOS = Object\.freeze\(\[([\s\S]*?)\]\);/u,
  );
  if (declaration === null) throw new Error("Could not read the runtime scenario contract");
  const runtimeScenarios = [...declaration[1].matchAll(/"([a-z0-9-]+)"/gu)].map(
    (match) => match[1],
  );
  if (JSON.stringify(runtimeScenarios) !== JSON.stringify(scenarios)) {
    throw new Error("Generator scenarios differ from the runtime report contract");
  }
}

function assertEpubMimetype(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, true) !== 0x04034b50 || view.getUint16(8, true) !== 0) {
    throw new Error("EPUB mimetype must be the first uncompressed ZIP entry");
  }
  const nameLength = view.getUint16(26, true);
  const extraLength = view.getUint16(28, true);
  const name = new TextDecoder().decode(bytes.subarray(30, 30 + nameLength));
  const dataStart = 30 + nameLength + extraLength;
  const data = new TextDecoder().decode(bytes.subarray(dataStart, dataStart + 20));
  if (name !== "mimetype" || data !== "application/epub+zip") {
    throw new Error("EPUB mimetype entry is invalid");
  }
}
