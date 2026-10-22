//! The reader bundle this process serves.
//!
//! A product build serves the bundle compiled into it. A development build may
//! read the same paths off disk instead, and a contributor may replace the whole
//! bundle once, before any request is served.

use std::sync::OnceLock;

use include_dir::{Dir, include_dir};

static PRODUCT_READER_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../../dist");

/// The bundle this process serves. A contributor may replace it once, before any
/// request is served; a product build never does.
pub(crate) static READER_ASSETS: OnceLock<&'static Dir<'static>> = OnceLock::new();

/// The reader bundle in force, which is the product's unless one was contributed.
pub(crate) fn reader_assets() -> &'static Dir<'static> {
    READER_ASSETS
        .get()
        .copied()
        .unwrap_or(&PRODUCT_READER_ASSETS)
}

pub(crate) static READER_FONT_BUNDLE_ID: OnceLock<Option<String>> = OnceLock::new();

/// Development builds may serve the reader bundle from disk so a frontend rebuild does not
/// need a Rust rebuild. Packaged builds always answer from the bytes compiled into the
/// binary, so no runtime path is ever trusted.
#[cfg(debug_assertions)]
fn disk_reader_asset(path: &str) -> Option<Vec<u8>> {
    let root = std::path::Path::new(&std::env::var_os("ELEPHANT_READER_ASSETS")?)
        .canonicalize()
        .ok()?;
    let candidate = root.join(path).canonicalize().ok()?;
    if !candidate.starts_with(&root) {
        return None;
    }
    std::fs::read(candidate).ok()
}

#[cfg(not(debug_assertions))]
fn disk_reader_asset(_path: &str) -> Option<Vec<u8>> {
    None
}

pub(crate) fn reader_asset_bytes(path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    if let Some(bytes) = disk_reader_asset(path) {
        return Some(std::borrow::Cow::Owned(bytes));
    }
    reader_assets()
        .get_file(path)
        .map(|file| std::borrow::Cow::Borrowed(file.contents()))
}
