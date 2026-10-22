import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { zipSync, strToU8 } from "fflate";

process.env.TZ = "UTC";
if (process.argv.slice(2).some((arg) => arg !== "--check")) throw new Error("Only --check is supported");
const check = process.argv.includes("--check");
for (const id of ["static-fxl-ltr", "static-fxl-rtl", "static-fxl-mixed-svg"]) {
  const rtl = id === "static-fxl-rtl";
  const mixed = id === "static-fxl-mixed-svg";
  const paths = ["center.xhtml", "first.xhtml", mixed ? "second.svg" : "second.xhtml"];
  const sides = ["center", rtl ? "right" : "left", rtl ? "left" : "right"];
  const files = {
    mimetype: "application/epub+zip",
    "META-INF/container.xml": '<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
    "EPUB/package.opf": `<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:elephant:${id}</dc:identifier><dc:title>${id}</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-09-14T00:00:00Z</meta><meta property="rendition:layout">pre-paginated</meta><meta property="rendition:spread">both</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>${paths.map((path, i) => `<item id="p${i}" href="${path}" media-type="${path.endsWith("svg") ? "image/svg+xml" : "application/xhtml+xml"}"/>`).join("")}</manifest><spine page-progression-direction="${rtl ? "rtl" : "ltr"}">${paths.map((_, i) => `<itemref idref="p${i}" properties="rendition:page-spread-${sides[i]}"/>`).join("")}</spine></package>`,
    "EPUB/nav.xhtml": `<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body><nav epub:type="toc"><ol>${paths.map((path, i) => `<li><a href="${path}">${sides[i]}</a></li>`).join("")}</ol></nav></body></html>`,
  };
  paths.forEach((path, i) => {
    const side = sides[i];
    // Identical 300 x 400 viewports and edge-to-edge midlines expose seam errors.
    files[`EPUB/${path}`] = path.endsWith("svg")
      ? `<svg xmlns="http://www.w3.org/2000/svg" width="300" height="400" viewBox="0 0 300 400"><rect width="300" height="400" fill="#dcebdc"/><path d="M0 200H300" stroke="#163c48" stroke-width="4"/><text x="30" y="80" font-size="24">${side} / SVG</text></svg>`
      : `<html xmlns="http://www.w3.org/1999/xhtml"><head><title>${side}</title><meta name="viewport" content="width=300,height=400"/><style>html,body{margin:0;width:300px;height:400px;background:#e9dfcf}h1{position:absolute;left:30px;top:40px;font:24px serif}hr{position:absolute;top:198px;left:0;width:300px;height:4px;margin:0;border:0;background:#163c48}</style></head><body><h1>${side} / XHTML</h1><hr/></body></html>`;
  });
  const bytes = zipSync(Object.fromEntries(Object.entries(files).map(([path, text]) => [path, [strToU8(text), { level: path === "mimetype" ? 0 : 9, mtime: new Date("2026-09-14T00:00:00Z") }]])));
  const url = new URL(`${id}.epub`, import.meta.url);
  if (check) {
    if (!Buffer.from(bytes).equals(await readFile(url))) throw new Error(`${id} is not reproducible`);
  } else await writeFile(url, bytes);
  console.log(`${id} ${bytes.length} ${createHash("sha256").update(bytes).digest("hex")}`);
}
