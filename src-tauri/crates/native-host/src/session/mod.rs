//! Reader-session lifetime: opening, preparing, and closing a publication.
//!
//! Open accepts only public publication identity, selects a live current
//! snapshot, and validates persisted facts, response policy, and the complete
//! annotation projection before a session exists. Every listener, session, and
//! capacity reservation is acquired before a replacement session is published,
//! so admission failure leaves no partial origins or capabilities behind.

use crate::http::auth::is_hex_id;
use crate::http::not_found;
use crate::reader::{NavigationBootstrap, NavigationTicket, ReadingBootstrapSeed};
use crate::search::ensure_search_index;
use crate::snapshot::{ForegroundPreparationState, prepare_publication};
use crate::{
    Measurement, OPEN_OPERATION_HEADER, PRODUCTION_READING_REVISION, ReaderState,
    ResolvedActiveContentPolicy, ResolvedPublicationPolicy, ResolvedRemoteResourcesPolicy,
    ResponseManifest, SnapshotEntry, TRACE_HEADER, Trusted, TrustedJson, annotation_exchange,
    annotation_links, dictionary, generate_session_token, load_snapshot_entry, random_id,
    reader::measurement_basis_digest, reader_lifecycle::ActiveSessionClear,
    reading_order_local_resources, request_library_document, resolve_publication_policy, storage,
};
use axum::{
    Json, Router, extract::Path, extract::State, http::HeaderMap, http::StatusCode,
    response::IntoResponse, response::Response, routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap, sync::Arc, sync::Mutex, sync::atomic::AtomicBool,
    sync::atomic::AtomicU32, sync::atomic::Ordering, time::Duration, time::Instant,
};
use tokio::sync::watch;

/// Reader-session routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/publications/{publication_id}/open",
            post(open_publication),
        )
        .route("/native/reader/close", post(close_reader))
}

pub struct ActiveReaderSession {
    pub reader_session_id: Arc<str>,
    pub pagination_handle: Arc<str>,
    pub publication_id: Arc<str>,
    pub browser_origin_id: Arc<str>,
    pub entry: Arc<SnapshotEntry>,
    pub trace_id: Arc<str>,
    pub authorities: ReaderSessionAuthorities,
    pub policy: ResolvedPublicationPolicy,
    pub content_script_nonce: Option<Arc<str>>,
    pub revoked: AtomicBool,
    pub last_accepted_position_sequence: AtomicU32,
    pub network_activity: Mutex<Option<u64>>,
    pub(crate) dictionary: Arc<dictionary::DictionarySet>,
}

#[derive(Clone)]
pub struct ReaderSessionAuthorities {
    pub foreground_authority: Arc<str>,
    pub foreground_origin: Arc<str>,
    pub nested_authority: Arc<str>,
    pub nested_origin: Arc<str>,
    pub measurement_authority: Arc<str>,
    pub measurement_origin: Arc<str>,
}

pub(crate) struct PreparationJob {
    pub(crate) open_operation: u64,
    pub(crate) completion: watch::Sender<PreparationJobState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparationJobState {
    Running,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreparationParticipation {
    Started,
    Joined,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenPublicationResponse {
    status: &'static str,
    navigation_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloseReaderRequest {
    reader_session_id: String,
}

/// What the request itself says, once its headers and path have been trusted.
struct OpenRequest {
    library_document_id: String,
    trace_id: String,
    open_operation: u64,
}

/// Why a request never reached the lifecycle. Kept as the reason rather than a
/// built response, so admission does not decide how a rejection is rendered.
enum OpenRejection {
    Conflict(&'static str),
    BadRequest(&'static str),
    NotFound,
}

/// Validates the request before any lock is taken or any storage is read.
fn admit_open_request(
    state: &ReaderState,
    publication_id: &str,
    headers: &HeaderMap,
) -> Result<OpenRequest, OpenRejection> {
    let library_document_id =
        request_library_document(headers, state).map_err(OpenRejection::Conflict)?;
    if publication_id.len() != 32 || !publication_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(OpenRejection::NotFound);
    }
    let trace_id = request_trace_id(headers).map_err(OpenRejection::BadRequest)?;
    let open_operation = request_open_operation(headers).map_err(OpenRejection::BadRequest)?;
    Ok(OpenRequest {
        library_document_id,
        trace_id,
        open_operation,
    })
}

/// The archive hash this publication is currently bound to, which annotation
/// identity is derived from.
async fn publication_blob_hash(
    state: &ReaderState,
    publication_id: &str,
) -> Result<String, &'static str> {
    let storage = state.storage.clone();
    let publication_id = publication_id.to_owned();
    match tokio::task::spawn_blocking(move || {
        storage.locked()?.load_publication_blob(&publication_id)
    })
    .await
    {
        Ok(Ok(Some(blob))) => Ok(blob.hash),
        _ => {
            tracing::error!(event = "publication_identity_load_failed");
            Err("publication open failed")
        }
    }
}

/// Everything the new session is identified and reached by. These are minted
/// together and are never reused from a previous session.
struct SessionCredentials {
    reader_session_id: String,
    pagination_handle: String,
    navigation_nonce: String,
    authorities: ReaderSessionAuthorities,
}

fn mint_session_credentials(
    state: &ReaderState,
    browser_origin_id: &str,
) -> Result<SessionCredentials, ()> {
    Ok(SessionCredentials {
        reader_session_id: generate_session_token().map_err(|_| ())?,
        pagination_handle: generate_session_token().map_err(|_| ())?,
        navigation_nonce: generate_session_token().map_err(|_| ())?,
        authorities: allocate_session_authorities(
            browser_origin_id,
            state.imported_publication_port,
            state.nested_publication_port,
            state.measurement_port,
        )
        .map_err(|_| ())?,
    })
}

/// Chooses the snapshot this open will serve, on a blocking worker.
///
/// `warm_snapshot_id` lets storage report a reusable snapshot the registry is
/// already holding, so a reopen of the same publication skips derivation.
async fn select_publication(
    state: &ReaderState,
    publication_id: &str,
    warm_snapshot_id: Option<String>,
) -> Result<storage::PublicationSelection, StatusCode> {
    let storage = state.storage.clone();
    let publication_id = publication_id.to_owned();
    match tokio::task::spawn_blocking(move || {
        storage.locked()?.select_publication(
            &publication_id,
            PRODUCTION_READING_REVISION,
            warm_snapshot_id.as_deref(),
        )
    })
    .await
    {
        Ok(Ok(selection)) => Ok(selection),
        Ok(Err(_error)) => {
            tracing::error!(event = "publication_open_failed", error_kind = "storage");
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(_error) => {
            tracing::error!(event = "publication_open_failed", error_kind = "worker");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Reads this publication's stored preferences and policy together, under one
/// storage lock, so the reader is never told a policy that disagrees with the
/// preferences it was given alongside it.
async fn load_stored_reader_state(
    state: &ReaderState,
    publication_id: &str,
) -> Result<(storage::StoredPreferences, storage::StoredPublicationPolicy), StatusCode> {
    let storage = state.storage.clone();
    let publication_id = publication_id.to_owned();
    match tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        Ok::<_, String>((
            storage.load_publication_preferences(&publication_id)?,
            storage.load_publication_policy(&publication_id)?,
        ))
    })
    .await
    {
        Ok(Ok(stored)) => Ok(stored),
        Ok(Err(_error)) => {
            tracing::error!(
                event = "publication_preferences_load_failed",
                error_kind = "storage"
            );
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(_error) => {
            tracing::error!(
                event = "publication_preferences_load_failed",
                error_kind = "worker"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Opens the selected snapshot's archive on a blocking worker and retains it as
/// the registry's warm entry, so the next open of the same publication reuses it.
async fn load_selected_snapshot(
    state: &ReaderState,
    selected: Box<storage::SelectedPublication>,
    timings: &mut OpenStageTimings,
) -> Result<Arc<SnapshotEntry>, StatusCode> {
    match tokio::task::spawn_blocking(move || load_snapshot_entry(selected)).await {
        Ok(Ok(loaded)) => {
            timings.zip_open_ms = Some(loaded.zip_open_ms);
            timings.rendition_open_ms = Some(loaded.rendition_open_ms);
            let entry = Arc::new(loaded.entry);
            state
                .snapshots
                .lock()
                .expect("snapshot registry lock poisoned")
                .warm = Some(entry.clone());
            Ok(entry)
        }
        Ok(Err(_error)) => {
            tracing::error!(
                event = "publication_snapshot_load_failed",
                error_kind = "storage"
            );
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(_error) => {
            tracing::error!(
                event = "publication_snapshot_load_failed",
                error_kind = "worker"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Whether this open still owns the reader, checked immediately before its
/// authorities are published.
///
/// Two things can have overtaken it: a later open operation, or a library reload
/// that replaced the document this request was bound to. Either one means the
/// caller is acting on a view the host has already moved past.
fn open_is_still_current(
    state: &ReaderState,
    open_operation: u64,
    library_document_id: &str,
) -> bool {
    open_operation == state.latest_open_operation.load(Ordering::Acquire)
        && state
            .library_document_id
            .read()
            .expect("library document lock poisoned")
            .as_ref()
            .is_some_and(|current| current.as_ref() == library_document_id)
}

/// Tells the reader which permissions it must decide before this publication can
/// be opened, with the stored values and revisions it would be deciding against.
fn policy_decision_required(
    policy_state: &storage::StoredPublicationPolicy,
    relevance: storage::PublicationPolicyRelevance,
    required: &crate::policy::PolicyDecisionRequirement,
) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "status": "policy-decision-required",
            "required": {
                "activeContent": required.active_content,
                "remoteResources": required.remote_resources,
            },
            "policy": policy_state.policy,
            "effective": policy_state.effective,
            "relevance": relevance,
            "revisions": policy_state.revisions,
        })),
    )
        .into_response()
}

async fn open_publication(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    _: Trusted,
) -> Response {
    let request = match admit_open_request(&state, &publication_id, &headers) {
        Ok(request) => request,
        Err(OpenRejection::NotFound) => return not_found().await,
        Err(OpenRejection::Conflict(message)) => {
            return (StatusCode::CONFLICT, message).into_response();
        }
        Err(OpenRejection::BadRequest(message)) => {
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    };
    let OpenRequest {
        library_document_id,
        trace_id,
        open_operation,
    } = request;
    {
        let _sequence = state
            .open_sequence_gate
            .lock()
            .expect("open sequence lock poisoned");
        let latest = state.latest_open_operation.load(Ordering::Acquire);
        if open_operation > latest {
            state
                .latest_open_operation
                .store(open_operation, Ordering::Release);
        }
    }
    let started = Instant::now();
    let mut timings = OpenStageTimings::default();
    let gate_started = Instant::now();
    let mut operation = state.lifecycle.lock().await;
    timings.session_gate_wait_ms = duration_milliseconds(gate_started.elapsed());
    if open_operation < state.latest_open_operation.load(Ordering::Acquire) {
        log_open_summary(&trace_id, started, "superseded", false, &timings);
        return typed_open_status(StatusCode::CONFLICT, "superseded", None);
    }
    let mut preparation_completed = false;
    let (entry, browser_origin_id, reading_position, position_revision, annotations, warm_hit) = loop {
        if operation.import_active() {
            log_open_summary(&trace_id, started, "import-active", false, &timings);
            return typed_open_status(StatusCode::CONFLICT, "import-active", None);
        }
        let warm_snapshot_id = state
            .snapshots
            .lock()
            .expect("snapshot registry lock poisoned")
            .warm
            .as_ref()
            .map(|entry| entry.snapshot_id.to_string());
        let selection_started = Instant::now();
        let selection = select_publication(&state, &publication_id, warm_snapshot_id).await;
        timings.snapshot_selection_ms += duration_milliseconds(selection_started.elapsed());
        let selection = match selection {
            Ok(selection) => selection,
            Err(status) => {
                log_open_summary(&trace_id, started, "failed", false, &timings);
                return (status, "publication open failed").into_response();
            }
        };
        match selection {
            storage::PublicationSelection::NotFound => {
                log_open_summary(&trace_id, started, "not-found", false, &timings);
                return not_found().await;
            }
            storage::PublicationSelection::SourceUnavailable => {
                log_open_summary(&trace_id, started, "source-unavailable", false, &timings);
                return typed_open_status(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "source-unavailable",
                    Some("repair-required"),
                );
            }
            storage::PublicationSelection::Revoked => {
                log_open_summary(&trace_id, started, "revoked", false, &timings);
                return typed_open_status(StatusCode::CONFLICT, "revoked", None);
            }
            storage::PublicationSelection::NeedsDerivation(blob) => {
                if preparation_completed {
                    log_open_summary(&trace_id, started, "preparation-failed", false, &timings);
                    return typed_open_status(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "preparation-failed",
                        None,
                    );
                }
                let (job, participation) = start_or_join_preparation(
                    &state,
                    publication_id.clone(),
                    blob,
                    trace_id.clone(),
                    open_operation,
                );
                timings.preparation_state = participation.as_str();
                let completion = job.completion.subscribe();
                drop(operation);
                let wait_started = Instant::now();
                let outcome =
                    tokio::time::timeout(state.preparation_wait, wait_for_preparation(completion))
                        .await;
                timings.preparation_wait_ms = duration_milliseconds(wait_started.elapsed());
                match outcome {
                    Err(_) => {
                        log_open_summary(&trace_id, started, "preparing", false, &timings);
                        return preparing_response();
                    }
                    Ok(PreparationJobState::Failed) => {
                        log_open_summary(&trace_id, started, "preparation-failed", false, &timings);
                        return typed_open_status(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "preparation-failed",
                            None,
                        );
                    }
                    Ok(PreparationJobState::Ready) => {
                        preparation_completed = true;
                        let gate_started = Instant::now();
                        operation = state.lifecycle.lock().await;
                        timings.session_gate_wait_ms +=
                            duration_milliseconds(gate_started.elapsed());
                        if open_operation < state.latest_open_operation.load(Ordering::Acquire) {
                            log_open_summary(&trace_id, started, "superseded", false, &timings);
                            return typed_open_status(StatusCode::CONFLICT, "superseded", None);
                        }
                    }
                    Ok(PreparationJobState::Running) => unreachable!(),
                }
                continue;
            }
            storage::PublicationSelection::Warm {
                snapshot_id,
                browser_origin_id,
                reading_position,
                position_revision,
                annotations,
            } => {
                let entry = state
                    .snapshots
                    .lock()
                    .expect("snapshot registry lock poisoned")
                    .warm
                    .clone()
                    .filter(|entry| entry.snapshot_id.as_ref() == snapshot_id);
                let Some(entry) = entry else {
                    log_open_summary(&trace_id, started, "failed", false, &timings);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "publication open failed")
                        .into_response();
                };
                break (
                    entry,
                    browser_origin_id,
                    reading_position,
                    position_revision,
                    annotations,
                    true,
                );
            }
            storage::PublicationSelection::Ready(selected) => {
                let snapshot_id = selected.snapshot_id.clone();
                ensure_search_index(&state, snapshot_id.clone(), selected.blob_path.clone());
                let browser_origin_id = selected.browser_origin_id.clone();
                let reading_position = selected.reading_position.clone();
                let position_revision = selected.position_revision;
                let annotations = selected.annotations.clone();
                let entry = match load_selected_snapshot(&state, selected, &mut timings).await {
                    Ok(entry) => entry,
                    Err(status) => {
                        log_open_summary(&trace_id, started, "failed", false, &timings);
                        return (status, "publication open failed").into_response();
                    }
                };
                debug_assert_eq!(entry.snapshot_id.as_ref(), snapshot_id);
                break (
                    entry,
                    browser_origin_id,
                    reading_position,
                    position_revision,
                    annotations,
                    false,
                );
            }
        }
    };
    if annotations
        .validate_resource_hints(&reading_order_local_resources(&entry.reading_order))
        .is_err()
    {
        log_open_summary(&trace_id, started, "failed", warm_hit, &timings);
        return (StatusCode::UNPROCESSABLE_ENTITY, "publication open failed").into_response();
    }
    let (preference_state, policy_state) =
        match load_stored_reader_state(&state, &publication_id).await {
            Ok(stored) => stored,
            Err(status) => return (status, "publication open failed").into_response(),
        };
    let policy = match resolve_publication_policy(&policy_state, entry.policy_relevance) {
        Ok(policy) => policy,
        Err(required) => {
            log_open_summary(
                &trace_id,
                started,
                "policy-decision-required",
                warm_hit,
                &timings,
            );
            return policy_decision_required(&policy_state, entry.policy_relevance, &required);
        }
    };
    let blob_hash = match publication_blob_hash(&state, &publication_id).await {
        Ok(hash) => hash,
        Err(message) => return (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
    };
    let reading_position = annotation_links::take_initial_position(&state, &publication_id, &entry)
        .or(reading_position);
    let annotation_identifiers = annotation_exchange::publication_identifiers(
        entry
            .epub
            .package()
            .unique_identifier()
            .map(|identifier| identifier.as_str()),
        &blob_hash,
    );
    let Ok(credentials) = mint_session_credentials(&state, &browser_origin_id) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "reader session failed").into_response();
    };
    let SessionCredentials {
        reader_session_id,
        pagination_handle,
        navigation_nonce,
        authorities,
    } = credentials;
    let content_script_nonce =
        matches!(policy.active_content, ResolvedActiveContentPolicy::Disabled)
            .then(|| Arc::from(reader_session_id.as_str()));
    let _sequence = state
        .open_sequence_gate
        .lock()
        .expect("open sequence lock poisoned");
    if !open_is_still_current(&state, open_operation, &library_document_id) {
        log_open_summary(&trace_id, started, "superseded", warm_hit, &timings);
        return typed_open_status(StatusCode::CONFLICT, "superseded", None);
    }
    let publication = serde_json::json!({
        "facts": entry.facts.as_ref(),
        "mount": {
            "baseUrl": format!("{}/", authorities.foreground_origin),
            "remoteResources": match policy.remote_resources {
                ResolvedRemoteResourcesPolicy::Allowed => "allowed",
                ResolvedRemoteResourcesPolicy::Blocked => "blocked",
            },
            "contentOrigin": "isolated-publication-origin"
        }
    });
    let pagination = live_pagination_descriptor(
        &entry.responses,
        &authorities.measurement_origin,
        &pagination_handle,
    );
    let active = Arc::new(ActiveReaderSession {
        reader_session_id: Arc::from(reader_session_id.as_str()),
        pagination_handle: Arc::from(pagination_handle.as_str()),
        publication_id: Arc::from(publication_id.as_str()),
        browser_origin_id: Arc::from(browser_origin_id.as_str()),
        entry: entry.clone(),
        trace_id: Arc::from(trace_id.as_str()),
        authorities: authorities.clone(),
        policy,
        content_script_nonce,
        revoked: AtomicBool::new(false),
        last_accepted_position_sequence: AtomicU32::new(0),
        network_activity: Mutex::new(None),
        dictionary: state.dictionary.lease(),
    });
    let _ = state.native_actions.set_narration_playback(None);
    operation.replace(active);
    operation.navigation_ticket = Some(NavigationTicket {
        nonce: navigation_nonce.clone(),
        bootstrap: NavigationBootstrap::Reading(Box::new(ReadingBootstrapSeed {
            reader_session_id: reader_session_id.clone(),
            publication_id: publication_id.clone(),
            publication_origin: authorities.foreground_origin.to_string(),
            measurement_origin: authorities.measurement_origin.to_string(),
            publication: publication.clone(),
            initial_reading_position: reading_position.clone(),
            position_revision,
            annotations,
            annotation_identifiers,
            preferences: preference_state.preferences,
            preference_revisions: preference_state.revisions,
            policy,
            pagination: pagination.clone(),
            media_overlay: entry.media_overlay_capability.clone(),
            dictionary: state.dictionary.lease().capability(),
        })),
    });
    let navigation_path = format!("/window/{navigation_nonce}/start");
    drop(_sequence);
    drop(operation);
    let response = Json(OpenPublicationResponse {
        status: "navigate",
        navigation_path,
    })
    .into_response();
    log_open_summary(&trace_id, started, "ready", warm_hit, &timings);
    response
}

pub(crate) fn live_pagination_descriptor(
    responses: &ResponseManifest,
    measurement_origin: &str,
    pagination_handle: &str,
) -> Value {
    let measurement_paths = responses
        .resources
        .iter()
        .filter(|(_, rule)| matches!(rule.measurement, Measurement::InstrumentedXhtml))
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let foreground_preparation_paths = responses
        .resources
        .iter()
        .filter(|(_, rule)| {
            matches!(
                rule.foreground_preparation,
                ForegroundPreparationState::Eligible
            )
        })
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    if measurement_paths.is_empty() && foreground_preparation_paths.is_empty() {
        serde_json::json!({
            "state": "unavailable",
            "reason": "capabilities-unavailable"
        })
    } else {
        let mut descriptor = serde_json::Map::from_iter([(
            "state".to_owned(),
            Value::String("available".to_owned()),
        )]);
        if !measurement_paths.is_empty() {
            descriptor.insert(
                "responses".to_owned(),
                serde_json::json!({
                    "mount": {
                        "baseUrl": format!("{measurement_origin}/"),
                        "remoteResources": "blocked",
                        "contentOrigin": "isolated-publication-origin"
                    },
                    "eligibleResourcePaths": measurement_paths
                }),
            );
        }
        if !foreground_preparation_paths.is_empty() {
            descriptor.insert(
                "foregroundPreparation".to_owned(),
                serde_json::json!({
                    "eligibleResourcePaths": foreground_preparation_paths
                }),
            );
        }
        if cfg!(target_os = "android") {
            descriptor.insert(
                "pacing".to_owned(),
                Value::String("conservative".to_owned()),
            );
        }
        descriptor.insert(
            "cache".to_owned(),
            serde_json::json!({
                "baseUrl": "/native/pagination-cache/",
                "handle": pagination_handle,
                "measurementArtifactDigest": measurement_basis_digest(),
            }),
        );
        Value::Object(descriptor)
    }
}

pub(crate) fn allocate_session_authorities(
    browser_origin_id: &str,
    foreground_port: u16,
    nested_port: u16,
    measurement_port: u16,
) -> Result<ReaderSessionAuthorities, String> {
    if browser_origin_id.len() != 32
        || !browser_origin_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("publication browser origin is invalid".to_owned());
    }
    let foreground_authority = format!("p-{browser_origin_id}.localhost:{foreground_port}");
    let nested_authority = format!("n-{}.localhost:{nested_port}", random_id(16)?);
    let measurement_authority = format!("m-{}.localhost:{measurement_port}", random_id(16)?);
    Ok(ReaderSessionAuthorities {
        foreground_origin: Arc::from(format!("http://{foreground_authority}")),
        foreground_authority: Arc::from(foreground_authority),
        nested_origin: Arc::from(format!("http://{nested_authority}")),
        nested_authority: Arc::from(nested_authority),
        measurement_origin: Arc::from(format!("http://{measurement_authority}")),
        measurement_authority: Arc::from(measurement_authority),
    })
}

pub(crate) fn request_trace_id(headers: &HeaderMap) -> Result<String, &'static str> {
    let mut values = headers.get_all(TRACE_HEADER).iter();
    let supplied = values.next();
    if values.next().is_some() {
        return Err("invalid trace id");
    }
    match supplied {
        Some(value) => {
            let value = value.to_str().map_err(|_| "invalid trace id")?;
            if value.len() != 32
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err("invalid trace id");
            }
            Ok(value.to_owned())
        }
        None => random_id(16).map_err(|_| "trace id generation failed"),
    }
}

pub(crate) fn request_open_operation(headers: &HeaderMap) -> Result<u64, &'static str> {
    let mut values = headers.get_all(OPEN_OPERATION_HEADER).iter();
    let supplied = values.next();
    if values.next().is_some() {
        return Err("invalid open operation");
    }
    let value = supplied.ok_or("missing open operation")?;
    let value = value.to_str().map_err(|_| "invalid open operation")?;
    let operation = value.parse::<u64>().map_err(|_| "invalid open operation")?;
    (operation > 0)
        .then_some(operation)
        .ok_or("invalid open operation")
}

pub(crate) struct OpenStageTimings {
    pub(crate) session_gate_wait_ms: u64,
    pub(crate) snapshot_selection_ms: u64,
    pub(crate) preparation_wait_ms: u64,
    pub(crate) preparation_state: &'static str,
    pub(crate) zip_open_ms: Option<u64>,
    pub(crate) rendition_open_ms: Option<u64>,
}

fn log_open_summary(
    trace_id: &str,
    started: Instant,
    status: &str,
    warm_hit: bool,
    timings: &OpenStageTimings,
) {
    log_performance(serde_json::json!({
        "event": "publication_open",
        "traceId": trace_id,
        "outcome": status,
        "totalMs": duration_milliseconds(started.elapsed()),
        "registry": if warm_hit { "hit" } else { "miss" },
        "sessionGateWaitMs": timings.session_gate_wait_ms,
        "snapshotSelectionMs": timings.snapshot_selection_ms,
        "preparationWaitMs": timings.preparation_wait_ms,
        "preparationState": timings.preparation_state,
        "zipOpenMs": timings.zip_open_ms,
        "renditionOpenMs": timings.rendition_open_ms,
    }));
}

pub(crate) fn duration_milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn log_performance(event: Value) {
    if let Ok(line) = serde_json::to_string(&event) {
        eprintln!("{line}");
    }
}

fn start_or_join_preparation(
    state: &ReaderState,
    publication_id: String,
    blob: storage::InstalledBlob,
    trace_id: String,
    open_operation: u64,
) -> (Arc<PreparationJob>, PreparationParticipation) {
    let (job, participation) =
        preparation_job(&state.preparations, &publication_id, open_operation);
    if participation == PreparationParticipation::Joined {
        return (job, participation);
    }
    let storage = state.storage.clone();
    let registry = state.preparations.clone();
    let admission = state.preparation_admission.clone();
    let worker_job = job.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let result = match admission.acquire_owned().await {
            Ok(permit) => tokio::task::spawn_blocking(move || {
                let _permit = permit;
                prepare_publication(&storage, blob)
            })
            .await
            .map_err(|error| format!("preparation worker failed: {error}"))
            .and_then(|result| result),
            Err(_) => Err("preparation admission is unavailable".to_owned()),
        };
        let (status, metrics) = match result {
            Ok(metrics) => ("ready", Some(metrics)),
            Err(_error) => {
                tracing::error!(event = "publication_preparation_failed");
                ("failed", None)
            }
        };
        let completion = if metrics.is_some() {
            PreparationJobState::Ready
        } else {
            PreparationJobState::Failed
        };
        let mut preparations = registry.lock().expect("preparation registry lock poisoned");
        let current = preparations
            .get(&publication_id)
            .is_some_and(|current| Arc::ptr_eq(current, &worker_job));
        if current {
            worker_job.completion.send_replace(completion);
            if completion == PreparationJobState::Ready {
                preparations.remove(&publication_id);
            }
        }
        drop(preparations);
        log_performance(serde_json::json!({
            "event": "publication_derivation",
            "traceId": trace_id,
            "outcome": status,
            "totalMs": duration_milliseconds(started.elapsed()),
            "zipOpenMs": metrics.as_ref().map(|metrics| metrics.zip_open_ms),
            "renditionOpenMs": metrics.as_ref().map(|metrics| metrics.rendition_open_ms),
        "factsMs": metrics.as_ref().map(|metrics| metrics.facts_ms),
        "coverMs": metrics.as_ref().map(|metrics| metrics.cover_ms),
        "analysisMs": metrics.as_ref().map(|metrics| metrics.analysis_ms),
        "responseManifestMs": metrics.as_ref().map(|metrics| metrics.response_manifest_ms),
            "validationMs": metrics.as_ref().map(|metrics| metrics.validation_ms),
            "commitMs": metrics.as_ref().map(|metrics| metrics.commit_ms),
            "sidecarPreparationMs": metrics.as_ref().map(|metrics| metrics.sidecar_preparation_ms),
            "sidecarTranscodedCount": metrics.as_ref().map(|metrics| metrics.sidecar_transcoded_count),
        }));
    });
    (job, participation)
}

pub(crate) fn preparation_job(
    registry: &Arc<Mutex<HashMap<String, Arc<PreparationJob>>>>,
    publication_id: &str,
    open_operation: u64,
) -> (Arc<PreparationJob>, PreparationParticipation) {
    let mut preparations = registry.lock().expect("preparation registry lock poisoned");
    if let Some(job) = preparations.get(publication_id) {
        let failed = *job.completion.borrow() == PreparationJobState::Failed;
        if !failed || open_operation <= job.open_operation {
            return (job.clone(), PreparationParticipation::Joined);
        }
    }
    let (completion, _) = watch::channel(PreparationJobState::Running);
    let job = Arc::new(PreparationJob {
        open_operation,
        completion,
    });
    preparations.insert(publication_id.to_owned(), job.clone());
    (job, PreparationParticipation::Started)
}

pub(crate) async fn wait_for_preparation(
    mut completion: watch::Receiver<PreparationJobState>,
) -> PreparationJobState {
    loop {
        let state = *completion.borrow_and_update();
        if state != PreparationJobState::Running {
            return state;
        }
        if completion.changed().await.is_err() {
            return *completion.borrow();
        }
    }
}

fn preparing_response() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "preparing",
        })),
    )
        .into_response()
}

fn typed_open_status(
    status: StatusCode,
    value: &'static str,
    reason: Option<&'static str>,
) -> Response {
    (
        status,
        Json(serde_json::json!({ "status": value, "reason": reason })),
    )
        .into_response()
}

async fn close_reader(
    State(state): State<ReaderState>,
    TrustedJson(request): TrustedJson<CloseReaderRequest>,
) -> Response {
    if !valid_reader_session_id(&request.reader_session_id) {
        return (StatusCode::BAD_REQUEST, "invalid reader session").into_response();
    }
    let mut operation = state.lifecycle.lock().await;
    let navigation_nonce = match generate_session_token() {
        Ok(nonce) => nonce,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "reader close failed").into_response();
        }
    };
    let library_document_id = match generate_session_token() {
        Ok(id) => id,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "reader close failed").into_response();
        }
    };
    let can_close = match operation
        .clear_if(|session| session.reader_session_id.as_ref() == request.reader_session_id)
    {
        ActiveSessionClear::Empty => true,
        ActiveSessionClear::Cleared => {
            tracing::info!(event = "reader_session_closed");
            let _ = state.native_actions.set_narration_playback(None);
            true
        }
        ActiveSessionClear::Retained => false,
    };
    if can_close {
        navigation_response(
            &mut operation.navigation_ticket,
            navigation_nonce,
            library_document_id,
        )
    } else {
        (StatusCode::CONFLICT, "reader session was replaced").into_response()
    }
}

fn navigation_response(
    pending: &mut Option<NavigationTicket>,
    nonce: String,
    library_document_id: String,
) -> Response {
    if let Some(NavigationTicket {
        nonce,
        bootstrap: NavigationBootstrap::Library { .. },
    }) = pending.as_ref()
    {
        return Json(serde_json::json!({
            "status": "navigate",
            "navigationPath": format!("/window/{nonce}/start")
        }))
        .into_response();
    }
    *pending = Some(NavigationTicket {
        nonce: nonce.clone(),
        bootstrap: NavigationBootstrap::Library {
            document_id: library_document_id,
        },
    });
    Json(serde_json::json!({
        "status": "navigate",
        "navigationPath": format!("/window/{nonce}/start")
    }))
    .into_response()
}

pub(crate) fn valid_reader_session_id(value: &str) -> bool {
    is_hex_id(value)
}

impl Default for OpenStageTimings {
    fn default() -> Self {
        Self {
            session_gate_wait_ms: 0,
            snapshot_selection_ms: 0,
            preparation_wait_ms: 0,
            preparation_state: "none",
            zip_open_ms: None,
            rendition_open_ms: None,
        }
    }
}
