use crate::ReaderState;
use crate::http::{TrustedJson, TrustedRead};
use crate::storage;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;

pub(super) async fn read(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    match state
        .storage
        .with("reading_activity_load", |storage| {
            storage.load_reading_activity()
        })
        .await
    {
        Ok(days) => Json(days).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "reading activity unavailable",
        )
            .into_response(),
    }
}

pub(super) async fn save(
    State(state): State<ReaderState>,
    TrustedJson(record): TrustedJson<storage::ActivityRecord>,
) -> Response {
    if !record.valid() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match state
        .storage
        .with("reading_activity_save", move |storage| {
            storage.save_reading_activity(&record)
        })
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not save reading activity",
        )
            .into_response(),
    }
}
