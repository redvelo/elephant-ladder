//! Native device operations exposed to the trusted reader.
//!
//! Covers the platform front-light dialog and the two-step external navigation
//! flow described in the production plan under "Native Authorization":
//! `prepare` normalizes and reports a destination without any external effect,
//! and `open` revalidates it before platform dispatch.

mod actions;
pub use actions::{
    DiagnosticsLevel, DisplayCapabilities, DisplayTechnology, HostInfo, NarrationCommand,
    NarrationPlayback, NarrationPlaybackState, NativeActionError, NativeActions,
    NativeActionsAvailability, OpenedPublication, OpenedPublicationIntent,
    UnsupportedNativeActions, WebsiteDataPurgeCategoryStatus, WebsiteDataPurgeReport,
};

use crate::http::HostError;
use crate::{
    MAX_EXTERNAL_MAIL_ADDRESS_BYTES, MAX_EXTERNAL_URL_BYTES, MAX_EXTERNAL_URL_REQUEST_BYTES,
    READER_HOST, ReaderState, TrustedJson, TrustedRead, TrustedWrite, has_exact_header,
};
use axum::routing::{get, post};
use serde::Deserialize;
use std::sync::atomic::Ordering;

use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode,
    response::IntoResponse, response::Response,
};

/// Device and external-navigation routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/display/front-light-dialog",
            post(show_front_light_dialog),
        )
        .route(
            "/native/window/fullscreen",
            get(window_fullscreen)
                .post(set_window_fullscreen)
                .layer(DefaultBodyLimit::max(MAX_WINDOW_FULLSCREEN_REQUEST_BYTES)),
        )
        .route(
            "/native/prepare-external-url",
            post(external_url::<true>).layer(DefaultBodyLimit::max(MAX_EXTERNAL_URL_REQUEST_BYTES)),
        )
        .route(
            "/native/open-external-url",
            post(external_url::<false>)
                .layer(DefaultBodyLimit::max(MAX_EXTERNAL_URL_REQUEST_BYTES)),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenExternalUrlRequest {
    url: String,
}

async fn show_front_light_dialog(State(state): State<ReaderState>, _: TrustedWrite) -> Response {
    native_action_empty(state.native_actions.show_front_light_dialog())
}

const MAX_WINDOW_FULLSCREEN_REQUEST_BYTES: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowFullscreenRequest {
    fullscreen: bool,
}

async fn window_fullscreen(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    let actions = state.native_actions.clone();
    let Ok(result) = tokio::task::spawn_blocking(move || actions.window_fullscreen()).await else {
        return HostError::Unavailable("native-action-failed").into_response();
    };
    native_action(result, |fullscreen| {
        Json(serde_json::json!({ "fullscreen": fullscreen })).into_response()
    })
}

async fn set_window_fullscreen(
    State(state): State<ReaderState>,
    TrustedJson(request): TrustedJson<WindowFullscreenRequest>,
) -> Response {
    let actions = state.native_actions.clone();
    let Ok(result) =
        tokio::task::spawn_blocking(move || actions.set_window_fullscreen(request.fullscreen))
            .await
    else {
        return HostError::Unavailable("native-action-failed").into_response();
    };
    native_action_empty(result)
}

async fn external_url<const PREPARE: bool>(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedWrite,
    body: Result<Json<OpenExternalUrlRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // Keep replacement and close excluded until validation and native dispatch finish.
    let _operation = state.lifecycle.lock().await;
    let Some(_) = state.lifecycle.load_active().filter(|active| {
        !active.revoked.load(Ordering::Acquire)
            && has_exact_header(
                &headers,
                "x-elephant-reader-session",
                &active.reader_session_id,
            )
    }) else {
        return (StatusCode::CONFLICT, "reader session unavailable").into_response();
    };
    let Ok(Json(request)) = body else {
        return (StatusCode::BAD_REQUEST, "invalid external URL request").into_response();
    };
    let Some(url) = validate_external_url(&request.url) else {
        return (StatusCode::BAD_REQUEST, "invalid external URL").into_response();
    };
    if PREPARE {
        // A mail link has no origin to name, so the reader is shown the one recipient instead.
        return match external_mail_address(&url) {
            Some(address) => {
                Json(serde_json::json!({ "url": url.as_str(), "address": address })).into_response()
            }
            None => Json(serde_json::json!({
                "url": url.as_str(),
                "origin": url.origin().ascii_serialization(),
            }))
            .into_response(),
        };
    }
    match state.native_actions.open_external_url(url.as_str()) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(NativeActionError::Unsupported) => {
            (StatusCode::NOT_IMPLEMENTED, "native action unsupported").into_response()
        }
        Err(NativeActionError::Failed(_)) => {
            tracing::warn!(event = "native_external_url_action_failed");
            (StatusCode::SERVICE_UNAVAILABLE, "native action failed").into_response()
        }
    }
}

pub(crate) fn validate_external_url(value: &str) -> Option<url::Url> {
    if value.len() > MAX_EXTERNAL_URL_BYTES {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if url.as_str().len() > MAX_EXTERNAL_URL_BYTES {
        return None;
    }
    match url.scheme() {
        "http" | "https" => {
            let host = url.host_str().unwrap_or_default();
            if !url.username().is_empty()
                || url.password().is_some()
                || host.is_empty()
                || app_owned_host(host)
            {
                return None;
            }
            Some(url)
        }
        "mailto" => external_mail_address(&url).map(|_| url),
        _ => None,
    }
}

/// The single recipient a mail link names, decoded for the consent prompt.
///
/// A mail link has no host or origin to check, so what it is held to instead is naming exactly
/// one ordinary recipient: the prompt says who the message goes to, and it can only say that
/// honestly if the link names one.
pub(crate) fn external_mail_address(url: &url::Url) -> Option<String> {
    if url.scheme() != "mailto" {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .ok()?
        .into_owned();
    let (local, domain) = decoded.split_once('@')?;
    if decoded.len() > MAX_EXTERNAL_MAIL_ADDRESS_BYTES
        || local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || decoded.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == ','
        })
    {
        return None;
    }
    Some(decoded)
}

pub(crate) fn app_owned_host(host: &str) -> bool {
    host == READER_HOST
        || host.ends_with(".elephant-ladder.localhost")
        || host.strip_suffix(".localhost").is_some_and(|label| {
            ["p-", "n-", "m-"]
                .iter()
                .any(|prefix| label.strip_prefix(prefix).is_some_and(|id| !id.is_empty()))
        })
}

pub(crate) fn native_action_empty(result: Result<(), NativeActionError>) -> Response {
    native_action(result, |()| StatusCode::NO_CONTENT.into_response())
}

fn native_action<T>(
    result: Result<T, NativeActionError>,
    respond: impl FnOnce(T) -> Response,
) -> Response {
    match result {
        Ok(value) => respond(value),
        Err(NativeActionError::Unsupported) => {
            HostError::Unsupported("native-action-unsupported").into_response()
        }
        Err(NativeActionError::Failed(_)) => {
            tracing::warn!(event = "native_action_failed");
            HostError::Unavailable("native-action-failed").into_response()
        }
    }
}
