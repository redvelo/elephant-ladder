//! Reading an incoming publication off the socket under an explicit byte budget.
//!
//! The declared length is reserved against the archive quota before any byte is
//! written, and a guard cancels the reservation if the transfer does not finish, so
//! an abandoned import cannot hold space. Nothing is read past the reservation.

use std::path::Path as FsPath;
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header::CONTENT_LENGTH};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::staging::StagedFile;
use crate::{MAX_ACQUISITION_BYTES, io, storage};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AcquisitionMetrics {
    pub(super) size_bytes: String,
    pub(super) blake3: String,
    pub(super) chunk_count: u64,
    pub(super) maximum_chunk_bytes: usize,
    pub(super) receive_milliseconds: u128,
    pub(super) sync_milliseconds: u128,
}

pub(super) fn exact_content_length(headers: &HeaderMap) -> Result<u64, (StatusCode, &'static str)> {
    let mut values = headers.get_all(CONTENT_LENGTH).iter();
    let size = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|size| *size > 0)
        .ok_or((StatusCode::BAD_REQUEST, "invalid content length"))?;
    if values.next().is_some() {
        return Err((StatusCode::BAD_REQUEST, "invalid content length"));
    }
    if size > MAX_ACQUISITION_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "acquisition probe too large"));
    }
    Ok(size)
}

pub(crate) struct ImportReservationGuard {
    pub(crate) storage: storage::StorageHandle,
    pub(crate) reservation_id: Option<String>,
    pub(crate) candidate_blob_hash: Option<String>,
}

pub(crate) fn cancel_import_reservation(
    storage: &storage::StorageHandle,
    reservation_id: &str,
    candidate_blob_hash: Option<&str>,
) {
    let Ok(mut storage) = storage.lock() else {
        return;
    };
    if storage
        .cancel_import_reservation(reservation_id, candidate_blob_hash)
        .is_err()
    {
        tracing::error!(
            event = "import_reservation_cleanup_failed",
            error_kind = "storage"
        );
    }
}

pub(super) async fn cleanup_import_reservation(guard: ImportReservationGuard) {
    if tokio::task::spawn_blocking(move || guard.cleanup())
        .await
        .is_err()
    {
        tracing::error!(
            event = "import_reservation_cleanup_failed",
            error_kind = "worker"
        );
    }
}

pub(super) struct ReceivedAcquisition {
    pub(super) metrics: AcquisitionMetrics,
    pub(super) guard: StagedFile,
}

pub(crate) enum AcquisitionError {
    BadRequest(&'static str),
    TooLarge,
    Storage(io::Error),
}

pub(super) async fn receive_acquisition(
    mut file: tokio::fs::File,
    guard: StagedFile,
    mut body: Body,
    declared_size: u64,
) -> Result<ReceivedAcquisition, AcquisitionError> {
    let receive_started = Instant::now();
    let mut hasher = blake3::Hasher::new();
    let mut size = 0_u64;
    let mut chunk_count = 0_u64;
    let mut maximum_chunk_bytes = 0_usize;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| AcquisitionError::BadRequest("invalid request body"))?;
        let data = frame
            .data_ref()
            .ok_or(AcquisitionError::BadRequest("invalid request body"))?;
        let frame_size = u64::try_from(data.len()).map_err(|_| AcquisitionError::TooLarge)?;
        size = size
            .checked_add(frame_size)
            .ok_or(AcquisitionError::TooLarge)?;
        if size > MAX_ACQUISITION_BYTES {
            return Err(AcquisitionError::TooLarge);
        }
        if size > declared_size {
            return Err(AcquisitionError::BadRequest("content length mismatch"));
        }
        file.write_all(data)
            .await
            .map_err(AcquisitionError::Storage)?;
        hasher.update(data);
        chunk_count += 1;
        maximum_chunk_bytes = maximum_chunk_bytes.max(data.len());
    }
    let receive_milliseconds = receive_started.elapsed().as_millis();
    if size != declared_size {
        return Err(AcquisitionError::BadRequest("content length mismatch"));
    }

    let sync_started = Instant::now();
    file.sync_all().await.map_err(AcquisitionError::Storage)?;
    let sync_milliseconds = sync_started.elapsed().as_millis();
    drop(file);
    Ok(ReceivedAcquisition {
        metrics: AcquisitionMetrics {
            size_bytes: size.to_string(),
            blake3: hasher.finalize().to_hex().to_string(),
            chunk_count,
            maximum_chunk_bytes,
            receive_milliseconds,
            sync_milliseconds,
        },
        guard,
    })
}

pub(super) async fn receive_reserved_acquisition(
    staging_path: &FsPath,
    body: Body,
    declared_size: u64,
) -> Result<ReceivedAcquisition, AcquisitionError> {
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging_path)
        .await
        .map_err(AcquisitionError::Storage)?;
    let guard = StagedFile::new(staging_path.to_path_buf());
    let staging = staging_path
        .parent()
        .ok_or_else(|| AcquisitionError::Storage(io::Error::other("staging path has no parent")))?
        .to_path_buf();
    tokio::task::spawn_blocking(move || storage::fsync_directory(&staging))
        .await
        .map_err(|error| AcquisitionError::Storage(io::Error::other(error)))?
        .map_err(AcquisitionError::Storage)?;
    receive_acquisition(file, guard, body, declared_size).await
}

impl ImportReservationGuard {
    pub(super) fn cleanup(mut self) {
        let Some(reservation_id) = self.reservation_id.take() else {
            return;
        };
        cancel_import_reservation(
            &self.storage,
            &reservation_id,
            self.candidate_blob_hash.as_deref(),
        );
    }
}

impl Drop for ImportReservationGuard {
    fn drop(&mut self) {
        let Some(reservation_id) = self.reservation_id.take() else {
            return;
        };
        let storage = self.storage.clone();
        let candidate_blob_hash = self.candidate_blob_hash.take();
        if std::thread::Builder::new()
            .name("import-reservation-cleanup".to_owned())
            .spawn(move || {
                cancel_import_reservation(
                    &storage,
                    &reservation_id,
                    candidate_blob_hash.as_deref(),
                );
            })
            .is_err()
        {
            tracing::error!(
                event = "import_reservation_cleanup_failed",
                error_kind = "thread-start"
            );
        }
    }
}

impl AcquisitionError {
    pub(super) fn into_response(self) -> Response {
        match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message).into_response(),
            Self::TooLarge => {
                (StatusCode::PAYLOAD_TOO_LARGE, "acquisition probe too large").into_response()
            }
            Self::Storage(error) => {
                let status = if matches!(
                    error.kind(),
                    io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
                ) || matches!(error.raw_os_error(), Some(28 | 122))
                {
                    StatusCode::INSUFFICIENT_STORAGE
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                tracing::error!(
                    event = "publication_acquisition_failed",
                    error_kind = if status == StatusCode::INSUFFICIENT_STORAGE {
                        "storage-full"
                    } else {
                        "storage"
                    }
                );
                (status, "publication acquisition storage failure").into_response()
            }
        }
    }
}
