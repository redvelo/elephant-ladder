//! Durable reader state: position, preferences, and annotations.
//!
//! Navigator derives positions from rendered browser state; the host validates
//! the credential, reader session, monotonic save sequence, and durable base
//! revision, then writes under revision compare-and-swap. A failed, stale, or
//! cancelled capture never overwrites the last durable value, and each accepted
//! write refreshes the pending recovery bootstrap.

use crate::http::{Conflict, HostError};
use crate::reader::{NavigationBootstrap, NavigationTicket, ReadingBootstrapSeed};
use crate::{
    MAX_ANNOTATION_MUTATION_REQUEST_BYTES, MAX_PREFERENCES_REQUEST_BYTES,
    MAX_READING_POSITION_REQUEST_BYTES, MAX_READING_POSITION_TARGET_BYTES, MAX_SEARCH_STRING_BYTES,
    ReaderState, TrustedJson, TrustedRead, storage, valid_reader_session_id,
};
use crate::{annotation_links, annotation_transfer, app_settings, journal, reading_activity};
use axum::routing::{get, post, put};
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::State, http::StatusCode,
    response::IntoResponse, response::Response,
};
use epub_navigator::{kernel::OccurrenceDocumentTarget, kernel::ReadingOrderOccurrence};
use epub_stack::EpubPath;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::Ordering;

/// Durable reader-state routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/annotation-links/pending",
            get(annotation_links::pending),
        )
        .route(
            "/native/publications/{publication_id}/journal",
            get(journal::read)
                .post(journal::mutate)
                .layer(axum::extract::DefaultBodyLimit::max(32 * 1024)),
        )
        .route(
            "/native/reading-activity",
            get(reading_activity::read).put(reading_activity::save),
        )
        .route(
            "/native/app-settings",
            get(app_settings::read)
                .put(app_settings::save)
                .layer(axum::extract::DefaultBodyLimit::max(8 * 1024)),
        )
        .route(
            "/native/app-settings/reset",
            post(app_settings::reset).layer(axum::extract::DefaultBodyLimit::max(1024)),
        )
        .route(
            "/native/publications/{publication_id}/preferences",
            axum::routing::delete(app_settings::clear_publication_preferences),
        )
        .route(
            "/native/publications/{publication_id}/annotations/export",
            get(annotation_transfer::export),
        )
        .route(
            "/native/publications/{publication_id}/annotations/save",
            post(annotation_transfer::save),
        )
        .route(
            "/native/publications/{publication_id}/annotations/import",
            post(annotation_transfer::import).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route(
            "/native/reading-position",
            put(save_reading_position)
                .layer(DefaultBodyLimit::max(MAX_READING_POSITION_REQUEST_BYTES)),
        )
        .route(
            "/native/preferences",
            get(read_preferences)
                .put(save_preferences)
                .layer(DefaultBodyLimit::max(MAX_PREFERENCES_REQUEST_BYTES)),
        )
        .route(
            "/native/annotations/mutations",
            post(mutate_annotations)
                .layer(DefaultBodyLimit::max(MAX_ANNOTATION_MUTATION_REQUEST_BYTES)),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnnotationMutationRequest {
    reader_session_id: String,
    mutation: storage::AnnotationMutation,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationMutationAccepted<'a> {
    status: &'static str,
    request_id: String,
    record_id: String,
    projection: storage::PublicAnnotationProjection<'a>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReadingPositionOccurrenceHint {
    pub(crate) occurrence: u32,
    pub(crate) resource: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveReadingPositionRequest {
    #[serde(default)]
    summary: Option<storage::ReadingSummary>,
    reader_session_id: String,
    base_revision: u32,
    sequence: u32,
    position: ReadingPosition,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveReadingPositionResponse {
    status: &'static str,
    reader_session_id: String,
    sequence: u32,
    position_revision: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavePreferencesRequest {
    reader_session_id: String,
    base_revisions: storage::PreferenceRevisions,
    preferences: storage::ReaderPreferences,
}

async fn read_preferences(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    let _operation = state.lifecycle.lock().await;
    let in_library = state
        .library_document_id
        .read()
        .expect("library document lock poisoned")
        .is_some();
    let publication_id = if in_library {
        None
    } else {
        let session = state.lifecycle.load_active();
        let Some(session) = session.filter(|session| !session.revoked.load(Ordering::Acquire))
        else {
            return (StatusCode::CONFLICT, "reader session replaced").into_response();
        };
        Some(session.publication_id.to_string())
    };
    let storage = state.storage.clone();
    match tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        match publication_id {
            Some(publication_id) => storage.load_publication_preferences(&publication_id),
            None => storage.load_global_preferences(),
        }
    })
    .await
    {
        Ok(Ok(preferences)) => Json(preferences).into_response(),
        Ok(Err(_error)) => {
            tracing::error!(
                event = "reader_preferences_load_failed",
                error_kind = "storage"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "preference read failed").into_response()
        }
        Err(_error) => {
            tracing::error!(
                event = "reader_preferences_load_failed",
                error_kind = "worker"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "preference read failed").into_response()
        }
    }
}

async fn save_preferences(
    State(state): State<ReaderState>,
    TrustedJson(request): TrustedJson<SavePreferencesRequest>,
) -> Response {
    if !valid_reader_session_id(&request.reader_session_id)
        || request.preferences.validate().is_err()
    {
        return (StatusCode::BAD_REQUEST, "invalid reader preferences").into_response();
    }
    let mut operation = state.lifecycle.lock().await;
    let session = state.lifecycle.load_active();
    let Some(session) = session.filter(|session| {
        !session.revoked.load(Ordering::Acquire)
            && session.reader_session_id.as_ref() == request.reader_session_id
    }) else {
        return preference_conflict("reader-session-replaced", None);
    };
    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let preferences = request.preferences;
    let base_revisions = request.base_revisions;
    let outcome = tokio::task::spawn_blocking(move || {
        storage.locked()?.compare_and_swap_preferences(
            &publication_id,
            base_revisions.global,
            base_revisions.publication,
            &preferences,
        )
    })
    .await;
    match outcome {
        Ok(Ok(storage::SavePreferencesOutcome::Saved(preferences))) => {
            update_recovery(
                &mut operation.navigation_ticket,
                &request.reader_session_id,
                |reading| {
                    reading.preferences = preferences.preferences.clone();
                    reading.preference_revisions = preferences.revisions.clone();
                },
            );
            Json(serde_json::json!({
                "status": "saved",
                "preferences": preferences.preferences,
                "revisions": preferences.revisions,
            }))
            .into_response()
        }
        Ok(Ok(storage::SavePreferencesOutcome::RevisionMismatch(preferences))) => {
            preference_conflict("revision-mismatch", Some(preferences))
        }
        Ok(Ok(storage::SavePreferencesOutcome::RevisionExhausted(preferences))) => {
            preference_conflict("revision-exhausted", Some(preferences))
        }
        Ok(Err(_error)) => {
            tracing::error!(
                event = "reader_preferences_save_failed",
                error_kind = "storage"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "preference save failed").into_response()
        }
        Err(_error) => {
            tracing::error!(
                event = "reader_preferences_save_failed",
                error_kind = "worker"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "preference save failed").into_response()
        }
    }
}

fn preference_conflict(
    reason: &'static str,
    preferences: Option<storage::StoredPreferences>,
) -> Response {
    let authoritative = preferences.map_or(serde_json::Value::Null, |preferences| {
        serde_json::json!({
            "preferences": preferences.preferences,
            "revisions": preferences.revisions,
        })
    });
    HostError::conflict(reason, authoritative).into_response()
}

async fn save_reading_position(
    State(state): State<ReaderState>,
    TrustedJson(request): TrustedJson<SaveReadingPositionRequest>,
) -> Response {
    if !valid_reader_session_id(&request.reader_session_id)
        || request.sequence == 0
        || validate_reading_position(&request.position).is_err()
        || request
            .summary
            .as_ref()
            .is_some_and(|summary| !summary.valid())
    {
        return (StatusCode::BAD_REQUEST, "invalid reading position").into_response();
    }
    let mut operation = state.lifecycle.lock().await;
    let session = state.lifecycle.load_active();
    let Some(session) = session else {
        return reading_position_conflict("reader-session-replaced", None);
    };
    if session.reader_session_id.as_ref() != request.reader_session_id {
        return reading_position_conflict("reader-session-replaced", None);
    }
    if request.sequence
        <= session
            .last_accepted_position_sequence
            .load(Ordering::Acquire)
    {
        return reading_position_conflict("sequence-not-increasing", None);
    }
    let position_json = match serde_json::to_string(&request.position) {
        Ok(position) => position,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid reading position").into_response(),
    };
    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let base_revision = request.base_revision;
    let outcome = tokio::task::spawn_blocking(move || {
        storage.locked()?.save_reading_position_summary(
            &publication_id,
            base_revision,
            &position_json,
            request.summary.as_ref(),
        )
    })
    .await;
    match outcome {
        Ok(Ok(storage::SaveReadingPositionOutcome::Saved { revision })) => {
            session
                .last_accepted_position_sequence
                .store(request.sequence, Ordering::Release);
            update_recovery(
                &mut operation.navigation_ticket,
                &request.reader_session_id,
                |reading| {
                    reading.initial_reading_position = Some(request.position);
                    reading.position_revision = revision;
                },
            );
            Json(SaveReadingPositionResponse {
                status: "saved",
                reader_session_id: request.reader_session_id,
                sequence: request.sequence,
                position_revision: revision,
            })
            .into_response()
        }
        Ok(Ok(storage::SaveReadingPositionOutcome::RevisionMismatch { current_revision })) => {
            reading_position_conflict("revision-mismatch", Some(current_revision))
        }
        Ok(Ok(storage::SaveReadingPositionOutcome::RevisionExhausted { current_revision })) => {
            reading_position_conflict("revision-exhausted", Some(current_revision))
        }
        Ok(Err(_error)) => {
            tracing::error!(
                event = "reading_position_save_failed",
                error_kind = "storage"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "reading position save failed",
            )
                .into_response()
        }
        Err(_error) => {
            tracing::error!(
                event = "reading_position_save_failed",
                error_kind = "worker"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "reading position save failed",
            )
                .into_response()
        }
    }
}

fn reading_position_conflict(reason: &'static str, revision: Option<u32>) -> Response {
    let authoritative = revision.map_or(
        serde_json::Value::Null,
        |revision| serde_json::json!({ "positionRevision": revision }),
    );
    HostError::conflict(reason, authoritative).into_response()
}

async fn mutate_annotations(
    State(state): State<ReaderState>,
    TrustedJson(request): TrustedJson<AnnotationMutationRequest>,
) -> Response {
    if !valid_reader_session_id(&request.reader_session_id) {
        return (StatusCode::BAD_REQUEST, "invalid annotation mutation").into_response();
    }
    let request_id = request.mutation.request_id().to_owned();
    let mut operation = state.lifecycle.lock().await;
    let session = state.lifecycle.load_active();
    let Some(session) = session else {
        return annotation_conflict("reader-session-replaced", request_id, None);
    };
    if session.reader_session_id.as_ref() != request.reader_session_id {
        return annotation_conflict("reader-session-replaced", request_id, None);
    }
    let reading_order_resources = reading_order_local_resources(&session.entry.reading_order);
    let storage = state.storage.clone();
    let publication_id = session.publication_id.to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        storage
            .lock()
            .map_err(|_| storage::SaveAnnotationError::Storage)?
            .compare_and_swap_annotations(
                &publication_id,
                request.mutation,
                &reading_order_resources,
            )
    })
    .await;
    match outcome {
        Ok(Ok(storage::SaveAnnotationOutcome::Saved {
            request_id,
            record_id,
            projection,
        })) => {
            update_recovery(
                &mut operation.navigation_ticket,
                &request.reader_session_id,
                |reading| reading.annotations = projection.clone(),
            );
            Json(AnnotationMutationAccepted {
                status: "accepted",
                request_id,
                record_id,
                projection: projection.public(),
            })
            .into_response()
        }
        Ok(Ok(storage::SaveAnnotationOutcome::RevisionMismatch {
            request_id,
            projection,
        })) => annotation_conflict("revision-mismatch", request_id, Some(projection)),
        Ok(Ok(storage::SaveAnnotationOutcome::RevisionExhausted {
            request_id,
            projection,
        })) => annotation_conflict("revision-exhausted", request_id, Some(projection)),
        Ok(Err(storage::SaveAnnotationError::Invalid)) => {
            tracing::warn!(
                event = "annotation_mutation_rejected",
                error_kind = "invalid"
            );
            (StatusCode::BAD_REQUEST, "invalid annotation mutation").into_response()
        }
        Ok(Err(storage::SaveAnnotationError::Storage)) => {
            tracing::error!(event = "annotation_mutation_failed", error_kind = "storage");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "annotation mutation failed",
            )
                .into_response()
        }
        Err(_error) => {
            tracing::error!(event = "annotation_mutation_failed", error_kind = "worker");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "annotation mutation failed",
            )
                .into_response()
        }
    }
}

/// Refreshes the pending recovery bootstrap so a browser reload restores the
/// newest durable reader state.
///
/// Applies only while the pending ticket still belongs to this reader session:
/// a replacement session must not inherit the previous one's state.
fn update_recovery(
    pending: &mut Option<NavigationTicket>,
    reader_session_id: &str,
    update: impl FnOnce(&mut ReadingBootstrapSeed),
) {
    let Some(NavigationTicket {
        bootstrap: NavigationBootstrap::Reading(reading),
        ..
    }) = pending.as_mut()
    else {
        return;
    };
    if reading.reader_session_id == reader_session_id {
        update(reading);
    }
}

fn annotation_conflict(
    reason: &'static str,
    request_id: String,
    projection: Option<storage::AnnotationProjection>,
) -> Response {
    let authoritative = projection.as_ref().map_or(
        serde_json::Value::Null,
        |projection| serde_json::json!({ "projection": projection.public() }),
    );
    HostError::Conflict(Conflict::new(reason, authoritative).for_request(request_id))
        .into_response()
}

pub(crate) fn reading_order_local_resources(
    reading_order: &[ReadingOrderOccurrence],
) -> Vec<Option<String>> {
    reading_order
        .iter()
        .map(|occurrence| match &occurrence.document {
            OccurrenceDocumentTarget::Loadable { resource, .. } => Some(resource.clone()),
            OccurrenceDocumentTarget::Unavailable { .. } | OccurrenceDocumentTarget::Unresolved => {
                None
            }
        })
        .collect()
}

pub(crate) fn validate_reading_position(position: &ReadingPosition) -> Result<(), &'static str> {
    if position
        .roll_progression
        .is_some_and(|progression| !progression.is_finite() || !(0.0..=1.0).contains(&progression))
    {
        return Err("reading position roll progression is invalid");
    }
    if position.target_json.is_empty()
        || position.target_json.len() > MAX_READING_POSITION_TARGET_BYTES
        || !serde_json::from_str::<Value>(&position.target_json)
            .is_ok_and(|target| target.is_object())
    {
        return Err("reading position target is invalid");
    }
    if let Some(hint) = &position.occurrence_hint {
        let canonical_resource =
            EpubPath::new(hint.resource.clone()).is_ok_and(|path| path.as_str() == hint.resource);
        if hint.resource.is_empty()
            || hint.resource.len() > MAX_SEARCH_STRING_BYTES
            || !canonical_resource
        {
            return Err("reading position occurrence hint is invalid");
        }
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingPosition {
    pub(crate) target_json: String,
    pub(crate) occurrence_hint: Option<ReadingPositionOccurrenceHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) roll_progression: Option<f64>,
}
