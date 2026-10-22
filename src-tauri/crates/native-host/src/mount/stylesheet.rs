//! Stylesheets, with EPUB's prefixed declarations restated for the engine.
//!
//! EPUB retains `-epub-` prefixed properties that browsers do not implement, and a
//! stylesheet cannot be mapped in the document the way an inline declaration can: its
//! relative URLs resolve against the sheet, so it has to stay at its own address. A
//! manifest-declared stylesheet is therefore read, restated by the portable rewrite,
//! and served buffered.
//!
//! A sheet past its bound, one that is not UTF-8, or one that cannot be read is left
//! to the streaming path exactly as authored.

use crate::{ActiveReaderSession, MAX_STYLESHEET_BYTES};
use axum::body::Bytes;
use epub_stack::EpubPath;
use epub_stack::css::rewrite_prefixed_stylesheet;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Whether the manifest declares this resource a stylesheet, rather than the response
/// MIME resembling one.
pub(crate) fn declares_stylesheet(mime: &str) -> bool {
    mime == "text/css"
}

pub(crate) async fn stylesheet_representation(
    session: Arc<ActiveReaderSession>,
    path: EpubPath,
    size_bytes: u64,
) -> Option<Bytes> {
    if size_bytes > MAX_STYLESHEET_BYTES {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        if session.revoked.load(Ordering::Acquire) {
            return None;
        }
        let bytes = session
            .entry
            .epub
            .read_with(&path, |reader| {
                let mut bytes = Vec::new();
                reader
                    .take(MAX_STYLESHEET_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .ok()?
            .ok()?;
        if bytes.len() as u64 > MAX_STYLESHEET_BYTES {
            return None;
        }
        let rewritten = rewrite_prefixed_stylesheet(std::str::from_utf8(&bytes).ok()?);
        Some(match rewritten {
            Some(rewritten) => Bytes::from(rewritten),
            None => Bytes::from(bytes),
        })
    })
    .await
    .ok()
    .flatten()
}
