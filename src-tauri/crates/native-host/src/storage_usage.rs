//! What the application's storage is spending, from the ledgers that already record it.
//!
//! The archive knows the bytes of every book it committed, and a dictionary records its size
//! when it installs. Both answers are exact and cost one query each, so neither is measured by
//! walking the device: a walk of app storage is dominated by the browser cache, which is
//! thousands of small files on the sort of eMMC an e-reader has.

use axum::Json;
use axum::extract::State;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::ReaderState;
use crate::http::TrustedRead;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageUsage {
    books_bytes: u64,
    dictionaries_bytes: u64,
}

pub(super) async fn read(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    let storage = state.storage.clone();
    let measured = tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        Ok::<_, String>(StorageUsage {
            books_bytes: storage.committed_archive_bytes()?,
            dictionaries_bytes: storage
                .dictionary_installations()?
                .iter()
                .map(|installation| installation.installed_bytes)
                .sum(),
        })
    })
    .await;
    let usage = match measured {
        Ok(Ok(usage)) => usage,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(event = "storage_usage_failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage usage is unavailable",
            )
                .into_response();
        }
    };
    let mut response = Json(usage).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
