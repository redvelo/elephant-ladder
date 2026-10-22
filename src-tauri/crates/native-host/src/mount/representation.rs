//! Choosing a representation and turning it into a response.
//!
//! Publication responses are `private, no-cache` and carry a snapshot-scoped
//! validator, so a conditional request that matches returns 304 after the same
//! session gate as a 200 and costs no archive read.

use super::{RangeDecision, parse_range};
use crate::PRODUCTION_READING_REVISION;
use crate::http::session_headers;
use axum::{
    body::Body, body::Bytes, http::HeaderMap, http::HeaderValue, http::StatusCode,
    http::header::ACCEPT_RANGES, http::header::CACHE_CONTROL, http::header::CONTENT_LENGTH,
    http::header::CONTENT_RANGE, http::header::CONTENT_TYPE, http::header::ETAG,
    http::header::IF_NONE_MATCH, http::header::VARY, response::IntoResponse, response::Response,
};

/// One archive path can back both an instrumented source document and a synthesized
/// image-document wrapper, so the cache key names which representation it holds.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ResponseRepresentation {
    Source,
    ImageDocument,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PublicationResponseProfile {
    SpineLevel,
    ContainerConstrained,
}

pub(crate) fn opaque_container_resource_headers(headers: &mut HeaderMap) {
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
}

pub fn publication_representation(
    head: bool,
    mime: &str,
    bytes: impl Into<Bytes>,
    range_allowed: bool,
    range: Option<&HeaderValue>,
    csp: &str,
) -> Response {
    let bytes = bytes.into();
    let total = bytes.len();
    if !range_allowed {
        return publication_response(
            StatusCode::OK,
            mime,
            if head { Bytes::new() } else { bytes },
            total,
            "none",
            None,
            csp,
        );
    }
    let decision = if head {
        RangeDecision::Full
    } else {
        parse_range(range, total)
    };
    match decision {
        RangeDecision::Full => publication_response(
            StatusCode::OK,
            mime,
            if head { Bytes::new() } else { bytes },
            total,
            "bytes",
            None,
            csp,
        ),
        RangeDecision::Partial { start, end } => {
            let length = end - start + 1;
            let content_range = format!("bytes {start}-{end}/{total}");
            publication_response(
                StatusCode::PARTIAL_CONTENT,
                mime,
                if head {
                    Bytes::new()
                } else {
                    bytes.slice(start..end + 1)
                },
                length,
                "bytes",
                Some(&content_range),
                csp,
            )
        }
        RangeDecision::Unsatisfiable => {
            let mut response = (StatusCode::RANGE_NOT_SATISFIABLE, "").into_response();
            session_headers(response.headers_mut(), Some(csp));
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}"))
                    .expect("content range must be valid"),
            );
            response
        }
    }
}

pub(crate) fn publication_response(
    status: StatusCode,
    mime: &str,
    body: Bytes,
    content_length: usize,
    accept_ranges: &'static str,
    content_range: Option<&str>,
    csp: &str,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, content_length)
        .header(ACCEPT_RANGES, accept_ranges)
        .body(Body::from(body))
        .expect("publication response must be valid");
    session_headers(response.headers_mut(), Some(csp));
    publication_document_headers(response.headers_mut(), mime);
    if let Some(content_range) = content_range {
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(content_range).expect("content range must be valid"),
        );
    }
    response
}

/// Publication bytes are immutable per snapshot, so a revalidation needs no archive read.
/// A replaced snapshot changes the validator, and every read still passes the session gate.
pub(crate) fn publication_validator(
    snapshot_id: &str,
    path: &str,
    representation: &str,
    profile: PublicationResponseProfile,
    instrumentation: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [
        snapshot_id,
        path,
        representation,
        match profile {
            PublicationResponseProfile::SpineLevel => "spine-level",
            PublicationResponseProfile::ContainerConstrained => "container-constrained",
        },
        PRODUCTION_READING_REVISION,
        instrumentation,
    ] {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("\"{}\"", &hasher.finalize().to_hex()[..32])
}

pub(crate) fn publication_validator_matches(headers: &HeaderMap, validator: &str) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate.trim_start_matches("W/") == validator
            })
        })
}

pub(crate) fn apply_publication_validator(response: &mut Response, validator: &str) {
    if !response.status().is_success() {
        return;
    }
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(validator) {
        headers.insert(ETAG, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
}

/// Marks a response whose body depended on what the request said it could decode, so a
/// cache never serves one engine's representation to another.
pub(crate) fn apply_image_negotiation(response: &mut Response) {
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Accept"));
}

pub(crate) fn publication_not_modified(validator: &str, csp: &str) -> Response {
    let mut response = Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .body(Body::empty())
        .expect("not modified response must be valid");
    session_headers(response.headers_mut(), Some(csp));
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(validator) {
        headers.insert(ETAG, value);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    response
}

pub(crate) fn publication_document_headers(headers: &mut HeaderMap, mime: &str) {
    if matches!(
        mime.split(';').next(),
        Some("application/xhtml+xml" | "image/svg+xml" | "text/html")
    ) {
        headers.insert("origin-agent-cluster", HeaderValue::from_static("?1"));
    }
}

pub(crate) fn measurement_dependency_mime(mime: &str) -> bool {
    mime.starts_with("image/")
        || mime.starts_with("font/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime.starts_with("text/css")
}
