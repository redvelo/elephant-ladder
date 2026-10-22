//! Canonical publication path decoding.
//!
//! The raw path is decoded exactly once. Malformed UTF-8, separators,
//! backslashes, empty or dot segments, control characters, and double-decoding
//! traps are rejected before any provider lookup.

use axum::http::Uri;
use epub_stack::EpubPath;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

pub fn decode_publication_path(uri: &Uri) -> Result<EpubPath, &'static str> {
    if uri.query().is_some() {
        return Err("publication queries are not supported");
    }
    let raw = uri
        .path()
        .strip_prefix('/')
        .ok_or("publication path must be absolute")?;
    decode_publication_raw_path(raw)
}

pub fn image_document_source(path: &EpubPath) -> String {
    // Escape each canonical path segment as a URL, then as XML. No source markup is accepted.
    let url = format!(
        "/{}",
        path.as_str()
            .split('/')
            .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
            .collect::<Vec<_>>()
            .join("/")
    );
    let escaped = url
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    format!(
        "<!DOCTYPE html><html xmlns=\"http://www.w3.org/1999/xhtml\" data-epub-navigator-image-document=\"1\"><head><title></title><style>html,body{{margin:0;width:100%;height:100%;overflow:hidden}}img{{display:block;width:100%;height:100%;object-fit:contain}}</style></head><body><img src=\"{escaped}\" alt=\"\" /></body></html>"
    )
}

pub(crate) fn decode_publication_raw_path(raw: &str) -> Result<EpubPath, &'static str> {
    if raw.is_empty() || raw.ends_with('/') || raw.contains("//") || raw.contains('\\') {
        return Err("publication path is not canonical");
    }
    validate_percent_escapes(raw)?;
    let mut decoded = Vec::new();
    for segment in raw.split('/') {
        let segment = percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| "publication path is not UTF-8")?;
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains(['/', '\\'])
            || has_encoded_triplet(&segment)
        {
            return Err("publication path contains an unsafe segment");
        }
        decoded.push(segment.into_owned());
    }
    EpubPath::new(decoded.join("/")).map_err(|_| "publication path is not canonical")
}

pub(crate) fn validate_percent_escapes(value: &str) -> Result<(), &'static str> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err("publication path contains a malformed escape");
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

pub(crate) fn has_encoded_triplet(value: &str) -> bool {
    value.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && window[1].is_ascii_hexdigit() && window[2].is_ascii_hexdigit()
    })
}
