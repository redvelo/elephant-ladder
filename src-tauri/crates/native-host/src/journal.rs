use crate::ReaderState;
use crate::http::{TrustedJson, TrustedRead};
use crate::request_library_document;
use crate::storage;
use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;

fn response(
    result: Result<Result<storage::Journal, storage::JournalError>, tokio::task::JoinError>,
) -> Response {
    match result {
        Ok(Ok(journal)) => Json(journal).into_response(),
        Ok(Err(storage::JournalError::Missing)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(storage::JournalError::Invalid)) => {
            (StatusCode::BAD_REQUEST, "invalid reflection change").into_response()
        }
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "journal storage unavailable",
        )
            .into_response(),
    }
}

pub(super) async fn read(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    _: TrustedRead,
) -> Response {
    let storage = state.storage.clone();
    response(
        tokio::task::spawn_blocking(move || {
            storage
                .lock()
                .map_err(|_| storage::JournalError::Storage)?
                .load_journal(&publication_id)
        })
        .await,
    )
}

pub(super) async fn mutate(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    TrustedJson(mutation): TrustedJson<storage::JournalMutation>,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    if let Err(reason) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, reason).into_response();
    }
    let storage = state.storage.clone();
    response(
        tokio::task::spawn_blocking(move || {
            storage
                .lock()
                .map_err(|_| storage::JournalError::Storage)?
                .mutate_journal(&publication_id, mutation)
        })
        .await,
    )
}
