//! Pagination-cache storage exposed through a session-bound capability.
//!
//! Navigator owns basis identity, record formats, and stale-result rejection;
//! the host only stores opaque records under private snapshot identity. Every
//! operation revalidates the bearer, reader session, snapshot, origin, and
//! handle, and writes repeat that authorization immediately before commit so a
//! revoked operation cannot publish late data.

use crate::http::auth::is_hex_id;
use crate::{
    ActiveReaderSession, MAX_PAGINATION_BULK_ENCODED_BYTES,
    MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES, MAX_PAGINATION_BULK_RAW_BYTES,
    MAX_PAGINATION_BULK_RECORDS, MAX_PAGINATION_BULK_REQUEST_BYTES, MAX_PAGINATION_RECORD_BYTES,
    PAGINATION_HANDLE_HEADER, ReaderState, TrustedRead, authorize, constant_time_equal,
    has_exact_header, log_performance, storage,
};
use axum::routing::{post, put};
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::Path, extract::State, http::HeaderMap,
    http::StatusCode, http::header::CONTENT_TYPE, response::IntoResponse, response::Response,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet, sync::Arc, sync::OnceLock, sync::atomic::AtomicU64,
    sync::atomic::Ordering,
};

/// Pagination-cache routes on the trusted reader origin.
///
/// A bulk read carries only record identifiers and a bulk mutation carries their
/// encoded values, so the two methods have very different body limits. Building
/// one method router per limit and merging them keeps each limit next to the
/// methods it governs, rather than relying on the order in which chained
/// `MethodRouter::layer` calls happen to nest.
pub(crate) fn routes() -> Router<ReaderState> {
    let bulk_read = post(read_pagination_cache_bulk)
        .layer(DefaultBodyLimit::max(MAX_PAGINATION_BULK_REQUEST_BYTES));
    let bulk_mutate = put(mutate_pagination_cache_bulk)
        .delete(remove_pagination_cache_basis)
        .layer(DefaultBodyLimit::max(
            MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES,
        ));
    Router::new().route(
        "/native/pagination-cache/{basis}",
        bulk_read.merge(bulk_mutate),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaginationCacheBulkReadRequest {
    records: Vec<String>,
}

#[derive(Serialize)]
struct PaginationCacheBulkReadResponse {
    records: Vec<Option<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaginationCacheBulkMutationRequest {
    operations: Vec<PaginationCacheBulkMutation>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum PaginationCacheBulkMutation {
    Write { record: String, value: String },
    Remove { record: String },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PaginationCacheOutcome {
    AuthorizationRejected,
    HandleRejected,
    ContentTypeRejected,
    BulkRead,
    BulkMutationApplied,
    Unavailable,
    QuotaRejected,
    ReadRejected,
    BulkMutationRejected,
    WorkerFailed,
}

#[derive(Default)]
struct PaginationCacheCounters {
    authorization_rejected: AtomicU64,
    handle_rejected: AtomicU64,
    content_type_rejected: AtomicU64,
    bulk_read: AtomicU64,
    bulk_mutation_applied: AtomicU64,
    unavailable: AtomicU64,
    quota_rejected: AtomicU64,
    read_rejected: AtomicU64,
    bulk_mutation_rejected: AtomicU64,
    worker_failed: AtomicU64,
}

fn record_pagination_cache_event(outcome: PaginationCacheOutcome) {
    static COUNTERS: OnceLock<PaginationCacheCounters> = OnceLock::new();
    let counters = COUNTERS.get_or_init(PaginationCacheCounters::default);
    let counter = match outcome {
        PaginationCacheOutcome::AuthorizationRejected => &counters.authorization_rejected,
        PaginationCacheOutcome::HandleRejected => &counters.handle_rejected,
        PaginationCacheOutcome::ContentTypeRejected => &counters.content_type_rejected,
        PaginationCacheOutcome::BulkRead => &counters.bulk_read,
        PaginationCacheOutcome::BulkMutationApplied => &counters.bulk_mutation_applied,
        PaginationCacheOutcome::Unavailable => &counters.unavailable,
        PaginationCacheOutcome::QuotaRejected => &counters.quota_rejected,
        PaginationCacheOutcome::ReadRejected => &counters.read_rejected,
        PaginationCacheOutcome::BulkMutationRejected => &counters.bulk_mutation_rejected,
        PaginationCacheOutcome::WorkerFailed => &counters.worker_failed,
    };
    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
    if count <= 3 || count.is_power_of_two() {
        log_performance(serde_json::json!({
            "schema": "elephant-ladder.native-pagination-cache",
            "version": 1,
            "event": "paginationCacheSummary",
            "outcome": outcome,
            "count": count,
        }));
    }
}

async fn read_pagination_cache_bulk(
    State(state): State<ReaderState>,
    Path(basis): Path<String>,
    headers: HeaderMap,
    _: TrustedRead,
    body: Result<Json<PaginationCacheBulkReadRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    let session = match authorize_pagination_session(&headers, &state) {
        Ok(session) => session,
        Err(status) => {
            record_pagination_cache_event(PaginationCacheOutcome::HandleRejected);
            return status.into_response();
        }
    };
    let request = match body {
        Ok(Json(request)) => request,
        // Only an exceeded body limit changes the status; every other malformed
        // body stays a plain bad request.
        Err(rejection) => {
            return (
                oversized_or_invalid(&rejection),
                "invalid pagination cache read",
            )
                .into_response();
        }
    };
    let mut unique = HashSet::with_capacity(request.records.len());
    if request.records.is_empty()
        || request.records.len() > MAX_PAGINATION_BULK_RECORDS
        || request
            .records
            .iter()
            .any(|record| !valid_sha256_hash(record) || !unique.insert(record.as_str()))
        || !valid_sha256_hash(&basis)
    {
        return (StatusCode::BAD_REQUEST, "invalid pagination cache read").into_response();
    }

    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let snapshot_id = session.entry.snapshot_id.to_string();
    match tokio::task::spawn_blocking(move || {
        storage.locked()?.read_pagination_records(
            &publication_id,
            &snapshot_id,
            &basis,
            &request.records,
        )
    })
    .await
    {
        Ok(Ok(storage::PaginationCacheRecordsReadOutcome::Records(records))) => {
            let mut raw_bytes = 0_usize;
            let mut encoded_bytes = 0_usize;
            let mut encoded = Vec::with_capacity(records.len());
            for value in records {
                let value = match value {
                    Some(value) => {
                        raw_bytes = match raw_bytes.checked_add(value.len()) {
                            Some(total) if total <= MAX_PAGINATION_BULK_RAW_BYTES => total,
                            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                        };
                        let value = BASE64_STANDARD.encode(value);
                        encoded_bytes = match encoded_bytes.checked_add(value.len()) {
                            Some(total) if total <= MAX_PAGINATION_BULK_ENCODED_BYTES => total,
                            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                        };
                        Some(value)
                    }
                    None => None,
                };
                encoded.push(value);
            }
            record_pagination_cache_event(PaginationCacheOutcome::BulkRead);
            Json(PaginationCacheBulkReadResponse { records: encoded }).into_response()
        }
        Ok(Ok(storage::PaginationCacheRecordsReadOutcome::Unavailable)) => {
            record_pagination_cache_event(PaginationCacheOutcome::Unavailable);
            StatusCode::CONFLICT.into_response()
        }
        Ok(Err(_)) => {
            record_pagination_cache_event(PaginationCacheOutcome::ReadRejected);
            (StatusCode::BAD_REQUEST, "invalid pagination cache read").into_response()
        }
        Err(_) => {
            record_pagination_cache_event(PaginationCacheOutcome::WorkerFailed);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn valid_sha256_hash(value: &str) -> bool {
    is_hex_id(value)
}

/// The trusted-reader boundary for the pagination plane.
///
/// Identical to [`Trusted`] except that a rejection is counted, so the pagination
/// counters still observe requests that never reach a handler.
struct PaginationAuthorized;

impl axum::extract::FromRequestParts<ReaderState> for PaginationAuthorized {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &ReaderState,
    ) -> Result<Self, Self::Rejection> {
        authorize(&parts.headers, &state.reader_origin, &state.token, false).map_err(|reason| {
            record_pagination_cache_event(PaginationCacheOutcome::AuthorizationRejected);
            (StatusCode::UNAUTHORIZED, reason).into_response()
        })?;
        Ok(Self)
    }
}

async fn mutate_pagination_cache_bulk(
    State(state): State<ReaderState>,
    Path(basis): Path<String>,
    headers: HeaderMap,
    _: PaginationAuthorized,
    body: Result<Json<PaginationCacheBulkMutationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !has_exact_header(&headers, CONTENT_TYPE, "application/json") {
        record_pagination_cache_event(PaginationCacheOutcome::ContentTypeRejected);
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid content type").into_response();
    }
    let _operation = state.lifecycle.lock().await;
    let session = match authorize_pagination_session(&headers, &state) {
        Ok(session) => session,
        Err(status) => {
            record_pagination_cache_event(PaginationCacheOutcome::HandleRejected);
            return status.into_response();
        }
    };
    let request = match body {
        Ok(Json(request)) => request,
        Err(rejection) => {
            return (
                oversized_or_invalid(&rejection),
                "invalid pagination cache mutation",
            )
                .into_response();
        }
    };
    if request.operations.is_empty()
        || request.operations.len() > MAX_PAGINATION_BULK_RECORDS
        || !valid_sha256_hash(&basis)
    {
        return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation").into_response();
    }

    let mut unique = HashSet::with_capacity(request.operations.len());
    let mut raw_bytes = 0_usize;
    let mut encoded_bytes = 0_usize;
    let mut mutations = Vec::with_capacity(request.operations.len());
    for operation in request.operations {
        let (record, mutation) = match operation {
            PaginationCacheBulkMutation::Write { record, value } => {
                encoded_bytes = match encoded_bytes.checked_add(value.len()) {
                    Some(total) if total <= MAX_PAGINATION_BULK_ENCODED_BYTES => total,
                    _ => {
                        return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation")
                            .into_response();
                    }
                };
                let Ok(value) = BASE64_STANDARD.decode(value) else {
                    return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation")
                        .into_response();
                };
                if value.len() > MAX_PAGINATION_RECORD_BYTES {
                    return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation")
                        .into_response();
                }
                raw_bytes = match raw_bytes.checked_add(value.len()) {
                    Some(total) if total <= MAX_PAGINATION_BULK_RAW_BYTES => total,
                    _ => {
                        return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation")
                            .into_response();
                    }
                };
                let mutation = storage::PaginationCacheMutation::Write {
                    record_hash: record.clone(),
                    value,
                };
                (record, mutation)
            }
            PaginationCacheBulkMutation::Remove { record } => {
                let mutation = storage::PaginationCacheMutation::Remove {
                    record_hash: record.clone(),
                };
                (record, mutation)
            }
        };
        if !valid_sha256_hash(&record) || !unique.insert(record) {
            return (StatusCode::BAD_REQUEST, "invalid pagination cache mutation").into_response();
        }
        mutations.push(mutation);
    }

    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let snapshot_id = session.entry.snapshot_id.to_string();
    match tokio::task::spawn_blocking(move || {
        storage.locked()?.mutate_pagination_records(
            &publication_id,
            &snapshot_id,
            &basis,
            &mutations,
        )
    })
    .await
    {
        Ok(Ok(storage::PaginationCacheMutationOutcome::Applied)) => {
            record_pagination_cache_event(PaginationCacheOutcome::BulkMutationApplied);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(storage::PaginationCacheMutationOutcome::Unavailable)) => {
            record_pagination_cache_event(PaginationCacheOutcome::Unavailable);
            StatusCode::CONFLICT.into_response()
        }
        Ok(Ok(storage::PaginationCacheMutationOutcome::SnapshotLimitExceeded)) => {
            record_pagination_cache_event(PaginationCacheOutcome::QuotaRejected);
            StatusCode::INSUFFICIENT_STORAGE.into_response()
        }
        Ok(Err(_)) => {
            record_pagination_cache_event(PaginationCacheOutcome::BulkMutationRejected);
            (StatusCode::BAD_REQUEST, "invalid pagination cache mutation").into_response()
        }
        Err(_) => {
            record_pagination_cache_event(PaginationCacheOutcome::WorkerFailed);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn remove_pagination_cache_basis(
    State(state): State<ReaderState>,
    Path(basis): Path<String>,
    headers: HeaderMap,
    _: PaginationAuthorized,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    let session = match authorize_pagination_session(&headers, &state) {
        Ok(session) => session,
        Err(status) => {
            record_pagination_cache_event(PaginationCacheOutcome::HandleRejected);
            return status.into_response();
        }
    };
    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let snapshot_id = session.entry.snapshot_id.to_string();
    match tokio::task::spawn_blocking(move || {
        storage
            .locked()?
            .remove_pagination_basis(&publication_id, &snapshot_id, &basis)
    })
    .await
    {
        Ok(Ok(storage::PaginationCacheMutationOutcome::Applied)) => {
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(storage::PaginationCacheMutationOutcome::Unavailable)) => {
            StatusCode::CONFLICT.into_response()
        }
        Ok(Ok(storage::PaginationCacheMutationOutcome::SnapshotLimitExceeded)) => {
            StatusCode::INSUFFICIENT_STORAGE.into_response()
        }
        Ok(Err(_)) => (StatusCode::BAD_REQUEST, "invalid pagination cache key").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn authorize_pagination_session(
    headers: &HeaderMap,
    state: &ReaderState,
) -> Result<Arc<ActiveReaderSession>, StatusCode> {
    let session = state.lifecycle.load_active().ok_or(StatusCode::CONFLICT)?;
    let mut handles = headers.get_all(PAGINATION_HANDLE_HEADER).iter();
    let supplied = handles.next().and_then(|value| value.to_str().ok());
    if handles.next().is_some()
        || session.revoked.load(Ordering::Acquire)
        || supplied.is_none_or(|supplied| {
            !constant_time_equal(supplied.as_bytes(), session.pagination_handle.as_bytes())
        })
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(session)
}

/// Distinguishes a body that exceeded its route's declared limit from one that
/// was merely malformed. Everything but the limit keeps its existing status.
fn oversized_or_invalid(rejection: &axum::extract::rejection::JsonRejection) -> StatusCode {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        StatusCode::PAYLOAD_TOO_LARGE
    } else {
        StatusCode::BAD_REQUEST
    }
}
