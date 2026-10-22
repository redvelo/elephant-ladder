//! The application-owned library: import, listing, deletion, and covers.
//!
//! Import streams a selected file into an unpredictable staging path while
//! counting bytes and hashing, then installs a verified content-addressed blob
//! and commits the library item, snapshot, and derived products in one durable
//! transaction. Deletion is resumable and bounded. Website-data reset rotates a
//! publication's browser origin without touching its EPUB bytes or reader state.

mod acquire;
mod cover;

pub(crate) use acquire::ImportReservationGuard;
use acquire::{
    ReceivedAcquisition, cleanup_import_reservation, exact_content_length,
    receive_reserved_acquisition,
};
pub(crate) use cover::{
    StagedCover, apply_reader_cover_cors, apply_reader_cover_cors_headers,
    apply_reader_page_image_cors, selected_cover_path, stage_selected_cover,
};

use crate::http::not_found;
use crate::media_overlay::backing as media_overlay_backing;
use crate::opened_publications;
use crate::publication_details;
use crate::search::ensure_search_index;
use crate::snapshot::remaining_sidecar_budget;
use crate::{
    ACQUISITION_TIMEOUT, NativeActionError, PRODUCTION_READING_REVISION, ReaderState, Trusted,
    TrustedRead, WebsiteDataPurgeReport, derive_publication, duration_milliseconds,
    has_exact_header, log_performance, request_library_document, storage,
};
use axum::routing::{delete, get, post};
use axum::{
    Json, Router, body::Body, extract::DefaultBodyLimit, extract::Path, extract::State,
    http::HeaderMap, http::Method, http::StatusCode, http::header::ACCEPT_RANGES,
    http::header::CONTENT_LENGTH, http::header::CONTENT_TYPE, response::IntoResponse,
    response::Response,
};
use epub_stack::package::metadata::DcElement;
use serde::Serialize;
use std::path::Path as FsPath;
use std::time::Instant;
use tokio_util::io::ReaderStream;

/// Library routes on the trusted reader origin.
///
/// Import disables the framework body limit because the request is a streamed
/// EPUB whose ceiling is enforced by the acquisition byte counter instead.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/publications/{publication_id}/details",
            get(publication_details::read),
        )
        .route(
            "/native/publications/import",
            post(import_publication).layer(DefaultBodyLimit::disable()),
        )
        .route("/native/publications", get(list_publications))
        .route("/native/storage/usage", get(crate::storage_usage::read))
        .route(
            "/native/opened-publications/pending",
            get(opened_publications::pending),
        )
        .route(
            "/native/opened-publications/{open_id}/bytes",
            get(opened_publications::bytes),
        )
        .route(
            "/native/publications/{publication_id}",
            delete(delete_publication),
        )
        .route(
            "/native/publications/{publication_id}/cover",
            get(publication_cover),
        )
        .route(
            "/native/publications/{publication_id}/website-data/reset",
            post(reset_publication_website_data),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicationSummaryResponse {
    publication_id: String,
    title: String,
    /// What direction the publication says its title reads, when it says. `creators` and
    /// `creator_directions` line up by index, so the names stay plain for sorting and
    /// search while the reader still renders each one the way it was authored.
    title_direction: Option<String>,
    creators: Vec<String>,
    creator_directions: Vec<Option<String>>,
    language: Option<String>,
    reading_direction: Option<String>,
    imported_at: String,
    last_read_at: Option<String>,
    has_reading_position: bool,
    has_cover: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportPublicationResponse {
    disposition: &'static str,
    publication: PublicationSummaryResponse,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebsiteDataResetResponse {
    status: &'static str,
    physical_purge: &'static str,
    purge_report: WebsiteDataPurgeReport,
}

async fn import_publication(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: Trusted,
    body: Body,
) -> Response {
    if !has_exact_header(&headers, CONTENT_TYPE, "application/epub+zip") {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid content type").into_response();
    }
    let declared_size = match exact_content_length(&headers) {
        Ok(size) => size,
        Err(response) => return response.into_response(),
    };
    let permit = match state.acquisition_slot.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (StatusCode::CONFLICT, "publication import already active").into_response();
        }
    };
    let import_lease = {
        let mut operation = state.lifecycle.lock().await;
        match operation.begin_import() {
            Ok(lease) => lease,
            Err(refusal) => return (StatusCode::CONFLICT, refusal.as_str()).into_response(),
        }
    };
    let storage = state.storage.clone();
    let reservation_lease = import_lease.clone();
    let reserved = tokio::task::spawn_blocking(move || {
        let _lease = reservation_lease;
        let outcome = storage.locked()?.reserve_import(declared_size)?;
        Ok::<_, String>(match outcome {
            storage::ReserveImportOutcome::Reserved(reservation) => {
                let guard = ImportReservationGuard {
                    storage,
                    reservation_id: Some(reservation.id.clone()),
                    candidate_blob_hash: None,
                };
                Some((reservation, guard))
            }
            storage::ReserveImportOutcome::QuotaExceeded => None,
        })
    })
    .await;
    let (reservation, mut reservation_guard) = match reserved {
        Ok(Ok(Some(reserved))) => reserved,
        Ok(Ok(None)) => {
            return (
                StatusCode::INSUFFICIENT_STORAGE,
                "archive storage quota exceeded",
            )
                .into_response();
        }
        Ok(Err(_error)) => {
            tracing::error!(
                event = "publication_import_failed",
                phase = "reservation",
                error_kind = "storage"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication import reservation failed",
            )
                .into_response();
        }
        Err(_error) => {
            tracing::error!(
                event = "publication_import_failed",
                phase = "reservation",
                error_kind = "worker"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication import reservation failed",
            )
                .into_response();
        }
    };
    let received = match tokio::time::timeout(
        ACQUISITION_TIMEOUT,
        receive_reserved_acquisition(&reservation.staging_path, body, declared_size),
    )
    .await
    {
        Ok(Ok(received)) => received,
        Ok(Err(error)) => {
            cleanup_import_reservation(reservation_guard).await;
            return error.into_response();
        }
        Err(_) => {
            cleanup_import_reservation(reservation_guard).await;
            return (StatusCode::REQUEST_TIMEOUT, "publication import timed out").into_response();
        }
    };
    let ReceivedAcquisition { metrics, guard } = received;
    log_performance(serde_json::json!({
        "event": "publication_import_received",
        "sizeBytes": metrics.size_bytes,
        "receiveMs": metrics.receive_milliseconds,
        "syncMs": metrics.sync_milliseconds,
    }));
    let path = guard.path().to_path_buf();
    let hash = metrics.blake3;
    reservation_guard.candidate_blob_hash = Some(hash.clone());
    let reservation_id = reservation.id;
    let storage = state.storage.clone();
    let worker_lease = import_lease.clone();
    let imported = tokio::task::spawn_blocking(move || {
        let _lease = worker_lease;
        let outcome =
            import_staged_publication(storage, &reservation_id, &path, &hash, declared_size);
        drop(guard);
        reservation_guard.cleanup();
        outcome
    })
    .await;
    drop(permit);
    match imported {
        Ok(Ok(outcome)) => {
            if let Some(snapshot_id) = outcome.snapshot_id.clone() {
                ensure_search_index(&state, snapshot_id, outcome.blob_path.clone());
            }
            let status = if outcome.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(ImportPublicationResponse {
                    disposition: if outcome.created {
                        "created"
                    } else {
                        "existing"
                    },
                    publication: publication_summary_response(outcome.publication),
                }),
            )
                .into_response()
        }
        Ok(Err(error)) => {
            tracing::warn!(
                event = "publication_import_failed",
                phase = "derivation",
                error_kind = "invalid-publication"
            );
            (StatusCode::UNPROCESSABLE_ENTITY, error).into_response()
        }
        Err(_error) => {
            tracing::error!(
                event = "publication_import_failed",
                phase = "derivation",
                error_kind = "worker"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication import failed",
            )
                .into_response()
        }
    }
}

async fn list_publications(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    let storage = state.storage.clone();
    match tokio::task::spawn_blocking(move || {
        let mut storage = storage.locked()?;
        storage.maintain_deletions()?;
        storage.list_publications()
    })
    .await
    {
        Ok(Ok(publications)) => Json(
            publications
                .into_iter()
                .map(publication_summary_response)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Ok(Err(_error)) => {
            tracing::error!(event = "publication_list_failed", error_kind = "storage");
            (StatusCode::INTERNAL_SERVER_ERROR, "publication list failed").into_response()
        }
        Err(_error) => {
            tracing::error!(event = "publication_list_failed", error_kind = "worker");
            (StatusCode::INTERNAL_SERVER_ERROR, "publication list failed").into_response()
        }
    }
}

async fn delete_publication(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    _: Trusted,
) -> Response {
    if publication_id.len() != 32
        || !publication_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return not_found().await;
    }
    let _publication_work = match state.acquisition_slot.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (StatusCode::CONFLICT, "publication import is active").into_response();
        }
    };
    let operation = state.lifecycle.lock_owned().await;
    if let Err(message) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, message).into_response();
    }
    let storage = state.storage.clone();
    let search_index = state.search_index.clone();
    let snapshots = state.snapshots.clone();
    let deletion_publication_id = publication_id.clone();
    let deleted = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        // The worker can outlive a cancelled HTTP request, so it must retain lifecycle ownership.
        let mut operation = operation;
        let mut storage = storage.locked()?;
        let snapshot_ids = match storage.begin_publication_deletion(&deletion_publication_id)? {
            storage::BeginPublicationDeletionOutcome::NotFound => return Ok(false),
            storage::BeginPublicationDeletionOutcome::Started { snapshot_ids } => snapshot_ids,
        };
        operation.clear_if(|session| session.publication_id.as_ref() == deletion_publication_id);
        {
            let mut registry = snapshots
                .lock()
                .map_err(|_| "snapshot registry lock poisoned".to_owned())?;
            if registry.warm.as_ref().is_some_and(|entry| {
                snapshot_ids
                    .iter()
                    .any(|snapshot_id| snapshot_id == entry.snapshot_id.as_ref())
            }) {
                registry.warm = None;
            }
        }
        if let Err(error) = storage.complete_publication_deletion(&deletion_publication_id) {
            let _ = storage.record_deletion_failure(&deletion_publication_id, &error);
            return Err(error);
        }
        for snapshot_id in &snapshot_ids {
            let _ = search_index.remove(snapshot_id);
        }
        Ok(true)
    })
    .await;
    match deleted {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => not_found().await,
        Ok(Err(_)) => {
            tracing::error!(
                event = "publication_deletion_failed",
                error_kind = "storage"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication deletion failed",
            )
                .into_response()
        }
        Err(_error) => {
            tracing::error!(event = "publication_deletion_failed", error_kind = "worker");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication deletion failed",
            )
                .into_response()
        }
    }
}

async fn reset_publication_website_data(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    _: Trusted,
) -> Response {
    if publication_id.len() != 32
        || !publication_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return not_found().await;
    }
    let _publication_work = match state.acquisition_slot.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (StatusCode::CONFLICT, "publication import is active").into_response();
        }
    };
    let operation = state.lifecycle.lock_owned().await;
    if let Err(message) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, message).into_response();
    }
    if operation.reader_active() {
        return (StatusCode::CONFLICT, "publication is open").into_response();
    }
    let storage = state.storage.clone();
    let reset_publication_id = publication_id;
    let reset = tokio::task::spawn_blocking(move || {
        // The worker retains lifecycle ownership so Open cannot race origin replacement.
        let _operation = operation;
        storage
            .locked()?
            .reset_publication_website_data(&reset_publication_id)
    })
    .await;
    match reset {
        Ok(Ok(storage::ResetPublicationWebsiteDataOutcome::Reset { retired_origin_id })) => {
            let retired_origin = format!(
                "http://p-{retired_origin_id}.localhost:{}",
                state.imported_publication_port
            );
            let purge_result = state
                .native_actions
                .purge_origin_website_data(&retired_origin);
            let purge_report = WebsiteDataPurgeReport::for_native_result(&purge_result);
            let report_json = serde_json::to_string(&purge_report)
                .expect("website-data purge report serialization cannot fail");
            let purge_error = match &purge_result {
                Err(NativeActionError::Failed(_)) => Some("platform purge attempt failed"),
                _ => None,
            };
            if state
                .storage
                .lock()
                .map_err(|_| ())
                .and_then(|mut storage| {
                    storage
                        .record_browser_origin_purge_attempt(
                            &retired_origin_id,
                            &report_json,
                            purge_error,
                        )
                        .map_err(|_| ())
                })
                .is_err()
            {
                tracing::error!(
                    event = "publication_website_data_purge_report_failed",
                    error_kind = "storage"
                );
            }
            let physical_purge = purge_report.physical_status();
            tracing::info!(event = "publication_website_data_reset", physical_purge);
            Json(WebsiteDataResetResponse {
                status: "reset",
                physical_purge,
                purge_report,
            })
            .into_response()
        }
        Ok(Ok(storage::ResetPublicationWebsiteDataOutcome::NotFound)) => not_found().await,
        Ok(Err(_)) => {
            tracing::error!(
                event = "publication_website_data_reset_failed",
                error_kind = "storage"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication website-data reset failed",
            )
                .into_response()
        }
        Err(_) => {
            tracing::error!(
                event = "publication_website_data_reset_failed",
                error_kind = "worker"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication website-data reset failed",
            )
                .into_response()
        }
    }
}

async fn publication_cover(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    method: Method,
    _: TrustedRead,
) -> Response {
    if publication_id.len() != 32
        || !publication_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return not_found().await;
    }
    let storage = state.storage.clone();
    let cover = match tokio::task::spawn_blocking(move || {
        storage.locked()?.load_publication_cover(&publication_id)
    })
    .await
    {
        Ok(Ok(Some(cover))) => cover,
        Ok(Ok(None)) => return not_found().await,
        Ok(Err(_error)) => {
            tracing::warn!(event = "publication_cover_failed", error_kind = "storage");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "publication cover is unavailable",
            )
                .into_response();
        }
        Err(_error) => {
            tracing::error!(event = "publication_cover_failed", error_kind = "worker");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "publication cover load failed",
            )
                .into_response();
        }
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        let file = match tokio::fs::File::open(&cover.path).await {
            Ok(file) => file,
            Err(_error) => {
                tracing::warn!(event = "publication_cover_failed", error_kind = "file-open");
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "publication cover is unavailable",
                )
                    .into_response();
            }
        };
        Body::from_stream(ReaderStream::new(file))
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, cover.mime)
        .header(CONTENT_LENGTH, cover.size)
        .header(ACCEPT_RANGES, "none")
        .body(body)
        .expect("cover response must be valid")
}

pub(crate) fn valid_publication_id(publication_id: &str) -> bool {
    publication_id.len() == 32
        && publication_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn import_staged_publication(
    storage: storage::StorageHandle,
    reservation_id: &str,
    path: &FsPath,
    hash: &str,
    size: u64,
) -> Result<storage::ImportOutcome, String> {
    let staging = path
        .parent()
        .ok_or_else(|| "imported EPUB staging path has no parent directory".to_owned())?;
    let (sidecar_budget, reusable_sidecars) = {
        let storage = storage.locked()?;
        (
            remaining_sidecar_budget(&storage)?,
            storage.reusable_media_transcodes(media_overlay_backing::transcode_derivation())?,
        )
    };
    let derivation_started = Instant::now();
    let derived = derive_publication(path, staging, sidecar_budget, &reusable_sidecars)?;
    log_performance(serde_json::json!({
        "event": "publication_import_derivation",
        "totalMs": duration_milliseconds(derivation_started.elapsed()),
        "zipOpenMs": derived.metrics.zip_open_ms,
        "renditionOpenMs": derived.metrics.rendition_open_ms,
        "factsMs": derived.metrics.facts_ms,
        "coverMs": derived.metrics.cover_ms,
        "analysisMs": derived.metrics.analysis_ms,
        "responseManifestMs": derived.metrics.response_manifest_ms,
        "validationMs": derived.metrics.validation_ms,
        "commitMs": derived.metrics.commit_ms,
        "sidecarPreparationMs": derived.metrics.sidecar_preparation_ms,
        "sidecarTranscodedCount": derived.metrics.sidecar_transcoded_count,
    }));

    let mut storage = storage.locked()?;
    let blob = storage.install_staged_blob(reservation_id, hash, size)?;
    let installed_cover = derived
        .cover
        .as_ref()
        .map(|cover| {
            storage.install_staged_cover(cover.guard.path(), &cover.hash, cover.size, &cover.mime)
        })
        .transpose()?;
    storage.commit_import_snapshot_with_policy_relevance(
        storage::InstalledImport {
            archive: &blob,
            cover: installed_cover.as_ref(),
        },
        PRODUCTION_READING_REVISION,
        &derived.title,
        storage::PolicyAwareReadingProducts {
            facts: &derived.facts,
            package: &derived.package,
            responses: &derived.responses,
            media_overlay: &derived.media_overlay,
            media_overlay_timelines: &derived.media_overlay_timelines,
            prepared_sidecars: derived.prepared_sidecars,
            policy_relevance: derived.policy_relevance,
        },
    )
}

pub(crate) fn publication_title(package: &epub_stack::package::Package) -> String {
    package
        .metadata()
        .elements(DcElement::Title)
        .first()
        .and_then(|title| title.content())
        .map(|title| title.as_str())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| title.chars().take(512).collect())
        .unwrap_or_else(|| "Untitled publication".to_owned())
}

fn publication_summary_response(
    publication: storage::PublicationSummary,
) -> PublicationSummaryResponse {
    PublicationSummaryResponse {
        publication_id: publication.publication_id,
        title: publication.title,
        title_direction: publication.title_direction,
        creators: publication.creators,
        creator_directions: publication.creator_directions,
        language: publication.language,
        reading_direction: publication.reading_direction,
        imported_at: publication.imported_at,
        has_reading_position: publication.last_read_at.is_some(),
        has_cover: publication.has_cover,
        last_read_at: publication.last_read_at,
    }
}
