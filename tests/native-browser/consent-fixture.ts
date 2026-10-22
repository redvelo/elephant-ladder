import { strToU8, zipSync } from "fflate";

const navigation = `<?xml version="1.0"?>
  <html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en">
    <head><title>Navigation</title></head>
    <body><nav epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body>
  </html>`;

function packageDocument({
  identifier,
  title,
  scripted,
}: {
  identifier: string;
  title: string;
  scripted: boolean;
}): string {
  const chapter = `<item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"${
    scripted ? ' properties="scripted"' : ""
  }/>`;
  const remote = scripted
    ? '<item id="remote" href="https://example.com/consent-probe.png" media-type="image/png"/>'
    : "";
  return `<?xml version="1.0"?>
    <package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
      <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
        <dc:identifier id="id">${identifier}</dc:identifier>
        <dc:title>${title}</dc:title><dc:language>en</dc:language>
        <meta property="dcterms:modified">2026-09-12T00:00:00Z</meta>
      </metadata>
      <manifest>
        <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
        ${chapter}
        ${remote}
      </manifest>
      <spine><itemref idref="chapter"/></spine>
    </package>`;
}

function chapter(scripted: boolean): string {
  const script = scripted
    ? '<script><![CDATA[document.documentElement.setAttribute("data-consent-probe", "ran");]]></script>'
    : "";
  return `<?xml version="1.0"?>
    <html xmlns="http://www.w3.org/1999/xhtml" lang="en">
      <head><title>Chapter</title></head>
      <body><h1>Consent fixture</h1><p>Quiet body.</p>${script}</body>
    </html>`;
}

function archive(options: { identifier: string; title: string; scripted: boolean }): Uint8Array {
  return zipSync({
    mimetype: [strToU8("application/epub+zip"), { level: 0 }],
    "META-INF/container.xml": strToU8(`<?xml version="1.0"?>
      <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
        <rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles>
      </container>`),
    "EPUB/package.opf": strToU8(packageDocument(options)),
    "EPUB/nav.xhtml": strToU8(navigation),
    "EPUB/chapter.xhtml": strToU8(chapter(options.scripted)),
  });
}

/**
 * Scripted, with one remote manifest item: enough to need both permissions and to be assessed
 * as detected, quiet enough to keep pagination and background traffic out of an observation.
 */
export const scriptedArchive = archive({
  identifier: "urn:elephant:network-smoke",
  title: "Network activity smoke",
  scripted: true,
});

/** A genuinely static book, not a policy override of the scripted one. */
export const staticArchive = archive({
  identifier: "urn:elephant:permissions-second",
  title: "Permissions second book",
  scripted: false,
});
