//! Shared response construction for the reader origin.
//!
//! Reader responses are private by default: same-origin resource policy, no
//! publication CORS, and `no-store` unless the route chose its own cache
//! lifetime.

use axum::{
    body::Body, http::HeaderMap, http::HeaderValue, http::StatusCode, http::header::CACHE_CONTROL,
    http::header::CONTENT_LENGTH, http::header::CONTENT_SECURITY_POLICY,
    http::header::CONTENT_TYPE, response::IntoResponse, response::Response,
};

/// The one 404 body the host serves, for callers that are not handlers.
pub fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

pub async fn not_found() -> Response {
    not_found_response()
}

pub(crate) fn response_with_bytes(
    status: StatusCode,
    mime: &'static str,
    bytes: &[u8],
    csp: Option<&str>,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes.to_vec()))
        .expect("byte response must be valid");
    session_headers(response.headers_mut(), csp);
    response
}

pub(crate) fn session_headers(headers: &mut HeaderMap, csp: Option<&str>) {
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if !headers.contains_key("cross-origin-resource-policy") {
        headers.insert(
            "cross-origin-resource-policy",
            HeaderValue::from_static("same-origin"),
        );
    }
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static(
            "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()",
        ),
    );
    if !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if let Some(csp) = csp {
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(csp).expect("CSP must be valid"),
        );
    }
}
