//! Reading derived narration over the trusted reader origin.

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{
        HeaderMap, StatusCode,
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::sync::atomic::Ordering;
use tokio_util::io::ReaderStream;

use super::playback;
use crate::ReaderState;
use crate::http::TrustedRead;
use crate::http::auth::constant_time_equal;
use crate::session::valid_reader_session_id;

/// Media-overlay and narration routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/reader-sessions/{reader_session_id}/media-overlays",
            get(read_media_overlay_description),
        )
        .route(
            "/native/reader-sessions/{reader_session_id}/media-overlays/{overlay_index}",
            get(read_media_overlay_timeline),
        )
        .route("/native/reader/narration", post(playback::report))
        .route("/native/reader/narration/commands", get(playback::commands))
}

async fn read_media_overlay_description(
    State(state): State<ReaderState>,
    Path(reader_session_id): Path<String>,
    _headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    media_overlay_response(state, reader_session_id, None).await
}

async fn read_media_overlay_timeline(
    State(state): State<ReaderState>,
    Path((reader_session_id, overlay_index)): Path<(String, String)>,
    _headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    let overlay_index = overlay_index
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| overlay_index.parse::<u32>().ok())
        .flatten();
    let Some(overlay_index) = overlay_index else {
        return (StatusCode::NOT_FOUND, "unknown media overlay").into_response();
    };
    media_overlay_response(state, reader_session_id, Some(overlay_index)).await
}

async fn media_overlay_response(
    state: ReaderState,
    reader_session_id: String,
    overlay_index: Option<u32>,
) -> Response {
    if !valid_reader_session_id(&reader_session_id) {
        return (StatusCode::BAD_REQUEST, "invalid reader session").into_response();
    }
    let active_session = |state: &ReaderState| {
        state.lifecycle.load_active().filter(|session| {
            !session.revoked.load(Ordering::Acquire)
                && constant_time_equal(
                    session.reader_session_id.as_bytes(),
                    reader_session_id.as_bytes(),
                )
        })
    };
    let Some(session) = active_session(&state) else {
        return (StatusCode::CONFLICT, "reader session replaced").into_response();
    };
    // A timeline is stored as its exact response bytes, so it streams straight off disk: no
    // parse, no clone, no database access, and memory that does not scale with its size.
    if let Some(index) = overlay_index {
        let Some(timeline) = session
            .entry
            .media_overlay_timeline_files
            .get(&index)
            .cloned()
        else {
            return (StatusCode::NOT_FOUND, "media overlay unavailable").into_response();
        };
        let Ok(file) = tokio::fs::File::open(&timeline.path).await else {
            return (StatusCode::SERVICE_UNAVAILABLE, "media overlay unavailable").into_response();
        };
        if active_session(&state).is_none() {
            return (StatusCode::CONFLICT, "reader session replaced").into_response();
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .header(CONTENT_LENGTH, timeline.length)
            .body(Body::from_stream(ReaderStream::new(file)))
            .expect("media overlay response must be valid");
    }
    let serialized = tokio::task::spawn_blocking(move || {
        serde_json::to_vec(session.entry.media_overlay.as_ref())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    })
    .await;
    let json = match serialized {
        Ok(Ok(json)) => json,
        Ok(Err(status)) => return (status, "media overlay unavailable").into_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "media overlay worker failed",
            )
                .into_response();
        }
    };
    if active_session(&state).is_none() {
        return (StatusCode::CONFLICT, "reader session replaced").into_response();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(json))
        .expect("media overlay response must be valid")
}
