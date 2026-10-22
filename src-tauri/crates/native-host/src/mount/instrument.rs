//! Ephemeral instrumentation of executable XHTML and reading-order SVG.
//!
//! The authored EPUB is never modified: the content agent is inserted ahead of
//! publisher scripts in the response only. Transformation is fail-closed, so an
//! unsafe or ambiguous document is refused rather than served uninstrumented.

use super::{PublicationResponseProfile, ResponseRepresentation};
use crate::reader::{content_agent_url, reader_font_base_url};
use crate::{
    ActiveReaderSession, Foreground, MAX_TRANSFORM_SOURCE_BYTES, MAX_TRANSFORMED_DOCUMENT_BYTES,
    ResponseRule, TransformCacheKey, TransformedOutcome, duration_milliseconds, instrumentation,
    random_id,
};
use axum::body::Bytes;
use epub_stack::EpubPath;
use instrumentation::instrument_xhtml as transform_xhtml;
use instrumentation::{SvgInstrumentationOptions, XhtmlInstrumentationOptions, instrument_svg};
use std::{io::Read, sync::Arc, sync::atomic::Ordering, time::Instant};
use tokio::sync::Semaphore;

pub(crate) async fn transformed_measurement_document(
    session: Arc<ActiveReaderSession>,
    path: EpubPath,
    asset_origin: Arc<str>,
    admission: Arc<Semaphore>,
) -> Result<(Bytes, String), String> {
    let permit = admission
        .acquire_owned()
        .await
        .map_err(|_| "measurement transformation is unavailable".to_owned())?;
    let nonce = random_id(16)?;
    let transform_nonce = nonce.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if session.revoked.load(Ordering::Acquire) {
            return Err("reader session was revoked".to_owned());
        }
        let bytes = session
            .entry
            .epub
            .read_with(&path, |reader| {
                let mut bytes = Vec::new();
                reader
                    .take(MAX_TRANSFORM_SOURCE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_TRANSFORM_SOURCE_BYTES {
            return Err("measurement XHTML input exceeds its bound".to_owned());
        }
        let transformed =
            instrument_xhtml_response(&bytes, &asset_origin, false, Some(&transform_nonce), None)
                .map_err(|error| error.to_string())?;
        if transformed.len() > MAX_TRANSFORMED_DOCUMENT_BYTES {
            return Err("measurement XHTML output exceeds its bound".to_owned());
        }
        if session.revoked.load(Ordering::Acquire) {
            return Err("reader session was revoked".to_owned());
        }
        Ok::<_, String>(Bytes::from(transformed))
    })
    .await
    .map_err(|error| format!("measurement XHTML worker failed: {error}"))??;
    Ok((loaded, nonce))
}

pub(crate) async fn transformed_document(
    session: Arc<ActiveReaderSession>,
    path: EpubPath,
    asset_origin: Arc<str>,
    admission: Arc<Semaphore>,
    foreground: Foreground,
    profile: PublicationResponseProfile,
    nested_publication_base_url: Option<Arc<str>>,
) -> Result<TransformedOutcome, String> {
    let entry = session.entry.clone();
    let content_script_nonce = session.content_script_nonce.clone();
    let cache_key = TransformCacheKey {
        path: path.as_str().to_owned(),
        representation: ResponseRepresentation::Source,
        profile,
        nested_publication_base_url: nested_publication_base_url.clone(),
        content_script_nonce: content_script_nonce.clone(),
    };
    if let Some(bytes) = entry
        .transforms
        .lock()
        .expect("transform cache lock poisoned")
        .get(&cache_key)
    {
        return Ok(TransformedOutcome {
            bytes,
            hit: true,
            cached: true,
            evictions: 0,
            source_bytes: None,
            resource_read_ms: None,
            transform_ms: None,
        });
    }
    // The archive read takes no transform permit: instrumentation output is what is bounded,
    // and holding the single permit across the read serialized every preloaded page document.
    let read_entry = entry.clone();
    let read_session = session.clone();
    let (bytes, resource_read_ms) = tokio::task::spawn_blocking(move || {
        let read_started = Instant::now();
        if read_session.revoked.load(Ordering::Acquire) {
            return Err("reader session was revoked".to_owned());
        }
        let bytes = read_entry
            .epub
            .read_with(&path, |reader| {
                let mut bytes = Vec::new();
                reader
                    .take(MAX_TRANSFORM_SOURCE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            })
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_TRANSFORM_SOURCE_BYTES {
            return Err("XHTML input exceeds its bound".to_owned());
        }
        Ok::<_, String>((bytes, duration_milliseconds(read_started.elapsed())))
    })
    .await
    .map_err(|error| format!("document worker failed: {error}"))??;
    let permit = admission
        .acquire_owned()
        .await
        .map_err(|_| "document transformation is unavailable".to_owned())?;
    if let Some(bytes) = entry
        .transforms
        .lock()
        .expect("transform cache lock poisoned")
        .get(&cache_key)
    {
        return Ok(TransformedOutcome {
            bytes,
            hit: true,
            cached: true,
            evictions: 0,
            source_bytes: None,
            resource_read_ms: None,
            transform_ms: None,
        });
    }
    let loaded = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if session.revoked.load(Ordering::Acquire) {
            return Err("reader session was revoked".to_owned());
        }
        let source_bytes = bytes.len();
        let transform_started = Instant::now();
        let transformed = match foreground {
            Foreground::Xhtml => instrument_xhtml_response(
                &bytes,
                &asset_origin,
                false,
                content_script_nonce.as_deref(),
                nested_publication_base_url.as_deref(),
            )
            .map_err(|error| error.to_string())?,
            Foreground::Svg => instrument_svg_response(
                &bytes,
                &asset_origin,
                false,
                content_script_nonce.as_deref(),
                nested_publication_base_url.as_deref(),
            )
            .map_err(|error| error.to_string())?,
            Foreground::Unchanged | Foreground::FixedImage => {
                return Err("resource does not have a document transformation".to_owned());
            }
        };
        if transformed.len() > MAX_TRANSFORMED_DOCUMENT_BYTES {
            return Err("document output exceeds its bound".to_owned());
        }
        let transformed = Bytes::from(transformed);
        if session.revoked.load(Ordering::Acquire) {
            return Err("reader session was revoked".to_owned());
        }
        let inserted = entry
            .transforms
            .lock()
            .expect("transform cache lock poisoned")
            .insert(cache_key, transformed);
        Ok::<_, String>((
            inserted,
            source_bytes,
            resource_read_ms,
            duration_milliseconds(transform_started.elapsed()),
        ))
    })
    .await
    .map_err(|error| format!("document worker failed: {error}"))??;
    Ok(TransformedOutcome {
        bytes: loaded.0.bytes,
        hit: false,
        cached: loaded.0.cached,
        evictions: loaded.0.evictions,
        source_bytes: Some(loaded.1),
        resource_read_ms: Some(loaded.2),
        transform_ms: Some(loaded.3),
    })
}

/// Instruments a reading-order SVG document, mirroring the XHTML entry point.
///
/// `include_snapshot` adds the publication-snapshot reporter, which only the
/// conformance fixtures request.
pub fn instrument_svg_response(
    bytes: &[u8],
    asset_origin: &str,
    include_snapshot: bool,
    nonce: Option<&str>,
    nested_publication_base_url: Option<&str>,
) -> Result<Vec<u8>, instrumentation::InstrumentationError> {
    let agent_url = content_agent_url(asset_origin);
    let snapshot_url =
        include_snapshot.then(|| format!("{asset_origin}/public/publication-snapshot.js"));
    instrument_svg(
        bytes,
        SvgInstrumentationOptions {
            content_agent_url: &agent_url,
            nested_publication_base_url,
            nonce,
            auxiliary_script_url: snapshot_url.as_deref(),
            crossorigin: true,
        },
    )
}

pub fn instrument_xhtml_response(
    bytes: &[u8],
    asset_origin: &str,
    include_snapshot: bool,
    nonce: Option<&str>,
    nested_publication_base_url: Option<&str>,
) -> Result<Vec<u8>, instrumentation::InstrumentationError> {
    let agent_url = content_agent_url(asset_origin);
    let reader_font_url = format!("{}/reader-serif.css", reader_font_base_url(asset_origin));
    let snapshot_url =
        include_snapshot.then(|| format!("{asset_origin}/public/publication-snapshot.js"));
    transform_xhtml(
        bytes,
        XhtmlInstrumentationOptions {
            content_agent_url: &agent_url,
            nested_publication_base_url,
            reader_font_stylesheet_url: Some(&reader_font_url),
            nonce,
            auxiliary_script_url: snapshot_url.as_deref(),
            crossorigin: true,
        },
    )
}

pub fn nested_publication_base_url(
    profile: PublicationResponseProfile,
    nested_origin: &str,
) -> Option<&str> {
    matches!(profile, PublicationResponseProfile::SpineLevel).then_some(nested_origin)
}

pub fn publication_document_transformation(
    profile: PublicationResponseProfile,
    rule: &ResponseRule,
) -> Option<Foreground> {
    match (profile, rule.foreground) {
        (PublicationResponseProfile::SpineLevel, Foreground::Xhtml) => Some(Foreground::Xhtml),
        (PublicationResponseProfile::SpineLevel, Foreground::Svg) => Some(Foreground::Svg),
        (PublicationResponseProfile::ContainerConstrained, _)
            if rule.mime.split(';').next() == Some("application/xhtml+xml") =>
        {
            Some(Foreground::Xhtml)
        }
        _ => None,
    }
}
