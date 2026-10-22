//! Authorization primitives for the trusted reader boundary.
//!
//! Every privileged route validates the exact trusted reader authority, exactly one
//! correctly formed bearer credential, and the request's method and content type.
//! Loopback binding, CORS, CSP, and sandboxing are defense in depth, never
//! authorization.

use std::fmt::Write as _;
use std::io;

use subtle::ConstantTimeEq as _;

use axum::http::{
    HeaderMap,
    header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN},
};

use crate::{TRUSTED_READER_REQUEST_HEADER, TRUSTED_READER_REQUEST_VALUE};

pub fn authorize(
    headers: &HeaderMap,
    reader_origin: &str,
    token: &str,
    require_json: bool,
) -> Result<(), &'static str> {
    if !has_exact_header(headers, ORIGIN, reader_origin) {
        return Err("invalid origin");
    }
    if require_json && !has_exact_header(headers, CONTENT_TYPE, "application/json") {
        return Err("invalid content type");
    }
    authorize_bearer(headers, token)
}

pub fn authorize_safe_read(
    headers: &HeaderMap,
    reader_origin: &str,
    token: &str,
) -> Result<(), &'static str> {
    if headers.get(ORIGIN).is_some() {
        if !has_exact_header(headers, ORIGIN, reader_origin) {
            return Err("invalid origin");
        }
    } else if headers.get("sec-fetch-site").is_some() {
        if !has_exact_header(headers, "sec-fetch-site", "same-origin") {
            return Err("invalid fetch site");
        }
    } else if !has_exact_header(
        headers,
        TRUSTED_READER_REQUEST_HEADER,
        TRUSTED_READER_REQUEST_VALUE,
    ) {
        return Err("missing safe-read context");
    }
    authorize_bearer(headers, token)
}

pub fn authorize_bearer(headers: &HeaderMap, token: &str) -> Result<(), &'static str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(presented) = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Err("missing authorization");
    };
    if values.next().is_some() || !constant_time_equal(presented.as_bytes(), token.as_bytes()) {
        return Err("invalid authorization");
    }
    Ok(())
}

pub fn has_exact_header(
    headers: &HeaderMap,
    name: impl axum::http::header::AsHeaderName,
    value: &str,
) -> bool {
    let mut values = headers.get_all(name).iter();
    matches!(values.next().and_then(|entry| entry.to_str().ok()), Some(found) if found == value)
        && values.next().is_none()
}

/// Compares two secrets without leaking their contents through timing.
///
/// This uses `subtle` rather than a hand-written accumulator loop: nothing in
/// plain Rust stops the optimiser from reintroducing an early exit, so the
/// constant-time property has to come from a crate that inserts optimisation
/// barriers. Lengths are compared in the clear, which only reveals the length
/// of a value the caller already controls.
pub fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

pub fn generate_session_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(token)
}

/// Whether a value could have come from [`random_id`] with 32 bytes: the shape
/// every 64-character host identifier and content hash shares.
pub(crate) fn is_hex_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn random_id(bytes: usize) -> Result<String, String> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value).map_err(|error| error.to_string())?;
    let mut encoded = String::with_capacity(bytes * 2);
    for byte in value {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}
