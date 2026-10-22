use super::support::*;

#[test]
fn image_only_svg_resources_are_not_instrumented() {
    let epub = EpubZip::from_reader(Cursor::new(ACTIVE_PROBE_BYTES.to_vec()))
        .unwrap()
        .default_rendition()
        .unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(
        &epub_navigator::facts::PublicationFacts::of(&epub).unwrap(),
    )
    .unwrap();
    let responses = crate::snapshot::response_manifest(&epub.analyze(), &reading_order).unwrap();

    for path in [
        "EPUB/images/probe-image.svg",
        "EPUB/images/scripted-probe.svg",
    ] {
        let rule = responses.resources.get(path).unwrap();
        assert!(matches!(rule.foreground, Foreground::Unchanged));
    }
}

#[test]
fn an_undeclared_resource_the_publication_links_to_is_withheld_and_named() {
    let package =
        br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
        <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:identifier id="id">undeclared</dc:identifier>
            <dc:title>Undeclared</dc:title>
            <dc:language>en</dc:language>
            <meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
        </metadata>
        <manifest>
            <item id="nav" properties="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
            <item id="page" href="page.xhtml" media-type="application/xhtml+xml"/>
            <item id="listed" href="listed.png" media-type="image/png"/>
        </manifest>
        <spine><itemref idref="page"/></spine>
    </package>"#;
    let page = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Page</title></head>
        <body><img src="listed.png" alt=""/><img src="unlisted.png" alt=""/>
        <!-- <img src="commented.png" alt=""/> --></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <head><title>Nav</title></head><body><nav epub:type="toc"><ol><li><a href="page.xhtml">Page</a></li></ol></nav></body></html>"#;
    let bytes = synthetic_epub(
        package,
        &[
            ("EPUB/page.xhtml", page),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/listed.png", b"listed"),
            // In the container, referenced by the page, absent from the manifest.
            ("EPUB/unlisted.png", b"unlisted"),
            // In the container, referenced by nothing: withheld without comment.
            ("EPUB/orphan.png", b"orphan"),
            // In the container, named only inside a comment: not a reference.
            ("EPUB/commented.png", b"commented"),
        ],
    );
    let epub = EpubZip::from_reader(Cursor::new(bytes))
        .unwrap()
        .default_rendition()
        .unwrap();
    let reading_order = epub_navigator::kernel::project_reading_order(
        &epub_navigator::facts::PublicationFacts::of(&epub).unwrap(),
    )
    .unwrap();
    let responses = crate::snapshot::response_manifest(&epub.analyze(), &reading_order).unwrap();

    assert!(!responses.resources["EPUB/listed.png"].undeclared);
    for path in ["EPUB/unlisted.png", "EPUB/orphan.png", "EPUB/commented.png"] {
        assert!(responses.resources[path].undeclared, "{path}");
        let refused =
            crate::admit_publication_resource(&format!("/{path}").parse().unwrap(), &responses);
        assert!(refused.is_err(), "{path} must not be served");
    }
    // Only the one a reader can actually notice is reported as a defect.
    assert_eq!(responses.undeclared_references, vec!["EPUB/unlisted.png"]);
}

/// A publication built from the three constructs this host used to refuse.
///
/// Each one is ordinary in publications that predate EPUB 3: an internal entity subset is how an
/// older XHTML document declares `&nbsp;`, an `xml-stylesheet` instruction is how it carries an
/// Adobe page template, and a large package document is what a reference work with thousands of
/// manifest entries has. None of them is a reason to withhold the publication, and the browser
/// adjudicates the first two for itself.
///
/// The host previously bounded structural documents at 1 MiB and refused both XML constructs, so
/// this publication could not be opened and its page answered `422`. Run against that revision,
/// every assertion below fails.
#[test]
fn a_publication_of_formerly_refused_constructs_opens_and_is_instrumented() {
    // Over the 1 MiB bound the host used to impose on a package document, under the portable
    // core's own 16 MiB default. Padded in metadata so every manifest entry still resolves.
    let padding = "pad ".repeat(300_000);
    let package = format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
        <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:identifier id="id">formerly-refused</dc:identifier>
            <dc:title>Formerly Refused</dc:title>
            <dc:language>en</dc:language>
            <dc:description>{padding}</dc:description>
            <meta property="dcterms:modified">2026-01-01T00:00:00Z</meta>
        </metadata>
        <manifest>
            <item id="nav" properties="nav" href="nav.xhtml" media-type="application/xhtml+xml"/>
            <item id="page" href="page.xhtml" media-type="application/xhtml+xml"/>
            <item id="template" href="page-template.xpgt" media-type="application/vnd.adobe-page-template+xml"/>
        </manifest>
        <spine><itemref idref="page"/></spine>
    </package>"#
    );
    assert!(
        package.len() > 1024 * 1024,
        "the package document must exceed the bound it used to be refused by: {}",
        package.len()
    );

    // An Adobe page template named by a processing instruction, and a named entity the document
    // can only resolve because its doctype declares one.
    let page = br#"<?xml version="1.0" encoding="utf-8"?>
<?xml-stylesheet type="application/vnd.adobe-page-template+xml" href="page-template.xpgt"?>
<!DOCTYPE html [<!ENTITY nbsp "&#160;">]>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Page</title></head>
<body><p>One&nbsp;two.</p></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
        <head><title>Nav</title></head><body><nav epub:type="toc"><ol><li><a href="page.xhtml">Page</a></li></ol></nav></body></html>"#;
    let bytes = synthetic_epub(
        package.as_bytes(),
        &[
            ("EPUB/page.xhtml", page),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/page-template.xpgt", b"<template/>"),
        ],
    );

    // Item 1: the publication opens under the portable core's defaults.
    let epub = EpubZip::from_reader(Cursor::new(bytes))
        .unwrap()
        .default_rendition()
        .expect("a publication with a large package document opens");
    let reading_order = epub_navigator::kernel::project_reading_order(
        &epub_navigator::facts::PublicationFacts::of(&epub).unwrap(),
    )
    .unwrap();
    let responses = crate::snapshot::response_manifest(&epub.analyze(), &reading_order).unwrap();
    assert!(matches!(
        responses.resources["EPUB/page.xhtml"].foreground,
        Foreground::Xhtml
    ));

    // Item 3: its page is instrumented rather than refused, and the authored bytes survive.
    let instrumented = crate::mount::instrument_xhtml_response(
        page,
        "http://asset.test",
        false,
        Some("nonce"),
        None,
    )
    .expect("a page with an entity subset and a page-template instruction is instrumented");
    let text = String::from_utf8(instrumented).unwrap();
    assert!(
        text.contains("/public/content-agent/"),
        "the agent is injected"
    );
    assert!(
        text.contains(r#"<!DOCTYPE html [<!ENTITY nbsp "&#160;">]>"#),
        "the subset survives"
    );
    assert!(
        text.contains("page-template.xpgt"),
        "the page template survives"
    );
    assert!(
        text.contains("One&nbsp;two."),
        "the authored entity survives"
    );
    // The agent precedes the body it has to observe.
    assert!(
        text.find("/public/content-agent/").unwrap() < text.find("One&nbsp;two.").unwrap(),
        "the agent is inserted ahead of the authored content"
    );
}
