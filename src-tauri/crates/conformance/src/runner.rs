//! What a fixture run contributes to the host: the runner page, the route it
//! fetches its fixture from, and the route it reports through.
//!
//! The fixture itself is a file the host is offered the way the platform offers a
//! book, so the runner page imports and opens it over the product's own routes
//! and the publication is served by the product's own planes under the product's
//! own policy. Nothing here serves publication bytes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{DefaultBodyLimit, Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use elephant_ladder_native_host::auth::{authorize, authorize_safe_read};
use elephant_ladder_native_host::{
    HostExtensions, MAX_CONFORMANCE_REQUEST_BYTES, OpenedPublication, OpenedPublicationIntent,
    ReaderState, offer_opened_publication,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::RunnerFixtureDescriptor;
use crate::artifact::{
    fixture_evidence_terminal_event, fixture_terminal_event, persist_fixture_evidence,
    persist_fixture_terminal_event, persist_publication_observations,
};
use crate::catalog::{
    EVIDENCE_RESULT_CONTRACT, RUNNER_FIXTURE_NOTICE, RUNNER_READER_ASSETS, RunnerFixtureSelection,
    RunnerFixtureSource,
};
use crate::report::{ConformanceSubmission, EvidenceSubmission};
use crate::validate::{validate_evidence_submission, validate_publication_observations};

/// An evidence submission carries bounded observations for up to 64 documents plus
/// the publication facts, so it is allowed more room than a check summary.
pub const MAX_EVIDENCE_REQUEST_BYTES: usize = 8 * 1024 * 1024;

pub struct RunnerState {
    pub descriptor: RunnerFixtureDescriptor,
    pub fixture_path: PathBuf,
    pub observation: Option<serde_json::Value>,
    pub notice: String,
    pub app_data_dir: PathBuf,
    offered: AtomicBool,
}

/// Writes the selected fixture where the host can be handed it, and returns the
/// contribution a fixture-runner binary makes to the host it binds.
pub fn contribute(
    selection: RunnerFixtureSelection,
    app_data: &Path,
) -> Result<HostExtensions, String> {
    let (fixture_path, notice) = match &selection.source {
        RunnerFixtureSource::Embedded(bytes) => (
            write_fixture(&selection.descriptor, bytes, app_data)?,
            RUNNER_FIXTURE_NOTICE.to_owned(),
        ),
        RunnerFixtureSource::File(path) => (
            path.clone(),
            format!("Publication supplied from {}", path.display()),
        ),
    };
    let runner = Arc::new(RunnerState {
        descriptor: selection.descriptor,
        fixture_path,
        observation: selection.observation,
        notice,
        app_data_dir: app_data.to_path_buf(),
        offered: AtomicBool::new(false),
    });
    Ok(HostExtensions {
        reader_routes: Some(reader_routes(runner)),
        reader_assets: Some(&RUNNER_READER_ASSETS),
    })
}

/// The embedded bytes become a file under the run's own data directory, because
/// the host hands over files: that is the one path a publication arrives by.
pub fn write_fixture(
    descriptor: &RunnerFixtureDescriptor,
    bytes: &[u8],
    app_data: &Path,
) -> Result<PathBuf, String> {
    let actual_sha256 = format!("{:x}", Sha256::digest(bytes));
    if actual_sha256 != descriptor.expected_sha256 {
        return Err(format!(
            "embedded runner fixture {} does not match its expected SHA-256",
            descriptor.id
        ));
    }
    let directory = app_data.join("fixture");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create the fixture directory: {error}"))?;
    let path = directory.join(format!("{}.epub", descriptor.id));
    std::fs::write(&path, bytes)
        .map_err(|error| format!("could not write the fixture file: {error}"))?;
    Ok(path)
}

/// The reader-origin routes a fixture run adds. A product binary has no route
/// that names a fixture or accepts what a run reports.
pub fn reader_routes(runner: Arc<RunnerState>) -> axum::Router<ReaderState> {
    axum::Router::new()
        .route("/native/conformance/fixture", get(fixture))
        .route(
            "/native/conformance",
            post(native_conformance).layer(DefaultBodyLimit::max(MAX_CONFORMANCE_REQUEST_BYTES)),
        )
        .route(
            "/native/conformance/evidence",
            post(native_evidence).layer(DefaultBodyLimit::max(MAX_EVIDENCE_REQUEST_BYTES)),
        )
        .layer(Extension(runner))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FixtureResponse<'a> {
    descriptor: &'a RunnerFixtureDescriptor,
    notice: &'a str,
    observation: Option<&'a serde_json::Value>,
}

/// Tells the runner page which fixture this process runs, and hands the host the
/// fixture file the first time it is asked, so the page then finds it where a book
/// handed over by the platform would be.
async fn fixture(
    State(state): State<ReaderState>,
    Extension(runner): Extension<Arc<RunnerState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(reason) = authorize_safe_read(&headers, &state.reader_origin, &state.token) {
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    if !runner.offered.swap(true, Ordering::AcqRel) {
        let offered = offer_opened_publication(
            &state,
            OpenedPublication {
                file_name: format!("{}.epub", runner.descriptor.id),
                path: runner.fixture_path.clone(),
                owned: false,
                intent: OpenedPublicationIntent::Read,
            },
        );
        if let Err(reason) = offered {
            tracing::error!(event = "fixture_offer_failed", reason);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "fixture could not be offered",
            )
                .into_response();
        }
    }
    let mut response = Json(FixtureResponse {
        descriptor: &runner.descriptor,
        notice: &runner.notice,
        observation: runner.observation.as_ref(),
    })
    .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

async fn native_conformance(
    State(state): State<ReaderState>,
    Extension(runner): Extension<Arc<RunnerState>>,
    headers: HeaderMap,
    body: Result<Json<ConformanceSubmission>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(reason) = authorize(&headers, &state.reader_origin, &state.token, true) {
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    let Json(submission) = match body {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response(),
    };
    let descriptor = &runner.descriptor;
    let publication_origin = state
        .lifecycle
        .active_view()
        .load()
        .filter(|session| !session.revoked.load(Ordering::Acquire))
        .map(|session| session.authorities.foreground_origin.to_string());
    if let Some(reason) = submission.incomplete_reason.as_deref() {
        if reason.is_empty()
            || reason.len() > 1000
            || submission.passed != 0
            || submission.total != descriptor.expected_total
            || submission.failures != ["runner-execution"]
            || submission.capture.is_some()
            || submission
                .publication_observations
                .as_ref()
                .is_some_and(|observations| !observations.is_empty())
        {
            return (StatusCode::BAD_REQUEST, "invalid incomplete fixture result").into_response();
        }
        if let Err(error) = persist_fixture_terminal_event(
            &runner.app_data_dir,
            fixture_terminal_event(descriptor, &submission, publication_origin.as_deref(), None),
        ) {
            tracing::error!(event = "fixture_terminal_event_write_failed", error = %error);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist terminal fixture event",
            )
                .into_response();
        }
        tracing::warn!(event = "fixture_run_incomplete", reason);
        log_finished(
            descriptor,
            &submission,
            "incomplete",
            publication_origin.as_deref(),
            None,
        );
        return StatusCode::NO_CONTENT.into_response();
    }
    if submission.total == 0
        || submission.total > 64
        || submission.total != descriptor.expected_total
        || submission.passed > submission.total
        || usize::from(submission.passed) + submission.failures.len()
            != usize::from(submission.total)
        || submission
            .failures
            .iter()
            .any(|failure| failure.is_empty() || failure.len() > 160)
        || submission.capture.as_ref().is_some_and(|capture| {
            capture.viewport_width == 0
                || capture.viewport_height == 0
                || capture.width == 0
                || capture.height == 0
                || capture.viewport_width > 8192
                || capture.viewport_height > 8192
                || u32::from(capture.x) + u32::from(capture.width)
                    > u32::from(capture.viewport_width)
                || u32::from(capture.y) + u32::from(capture.height)
                    > u32::from(capture.viewport_height)
        })
    {
        return (StatusCode::BAD_REQUEST, "invalid conformance result").into_response();
    }
    let Some(observations) = submission.publication_observations.as_deref() else {
        return (StatusCode::BAD_REQUEST, "missing publication observations").into_response();
    };
    let Some(publication_origin) = publication_origin else {
        return (StatusCode::CONFLICT, "no publication is open").into_response();
    };
    if let Err(reason) = validate_publication_observations(descriptor, observations) {
        tracing::warn!(event = "fixture_publication_observations_rejected", reason);
        return (StatusCode::BAD_REQUEST, reason).into_response();
    }
    let persisted = {
        let descriptor = descriptor.clone();
        let observations = observations.to_vec();
        let app_data_dir = runner.app_data_dir.clone();
        let publication_origin = publication_origin.clone();
        tokio::task::spawn_blocking(move || {
            persist_publication_observations(
                &app_data_dir,
                &descriptor,
                &publication_origin,
                observations,
            )
        })
        .await
    };
    let artifact = match persisted {
        Ok(Ok(artifact)) => artifact,
        Ok(Err(error)) => {
            tracing::error!(
                event = "fixture_publication_observations_write_failed",
                error = %error
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist publication observations",
            )
                .into_response();
        }
        Err(_) => {
            tracing::error!(
                event = "fixture_publication_observations_write_failed",
                error_kind = "worker"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist publication observations",
            )
                .into_response();
        }
    };
    if let Err(error) = persist_fixture_terminal_event(
        &runner.app_data_dir,
        fixture_terminal_event(
            descriptor,
            &submission,
            Some(&publication_origin),
            Some(&artifact),
        ),
    ) {
        tracing::error!(event = "fixture_terminal_event_write_failed", error = %error);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not persist terminal fixture event",
        )
            .into_response();
    }
    log_finished(
        descriptor,
        &submission,
        "complete",
        Some(&publication_origin),
        Some(&artifact),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Records what an evidence run observed. The route binds the evidence to the
/// publication the host has open and persists it; it judges nothing.
async fn native_evidence(
    State(state): State<ReaderState>,
    Extension(runner): Extension<Arc<RunnerState>>,
    headers: HeaderMap,
    body: Result<Json<EvidenceSubmission>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(reason) = authorize(&headers, &state.reader_origin, &state.token, true) {
        return (StatusCode::UNAUTHORIZED, reason).into_response();
    }
    let descriptor = &runner.descriptor;
    if descriptor.result_contract != EVIDENCE_RESULT_CONTRACT {
        return (
            StatusCode::CONFLICT,
            "this fixture reports checks, not evidence",
        )
            .into_response();
    }
    let Json(submission) = match body {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response(),
    };
    if let Err(reason) = validate_evidence_submission(&submission) {
        tracing::warn!(event = "fixture_evidence_rejected", reason);
        return (StatusCode::BAD_REQUEST, reason).into_response();
    }
    let publication_origin = state
        .lifecycle
        .active_view()
        .load()
        .filter(|session| !session.revoked.load(Ordering::Acquire))
        .map(|session| session.authorities.foreground_origin.to_string());
    if let Some(reason) = submission.incomplete_reason.as_deref() {
        if let Err(error) = persist_fixture_terminal_event(
            &runner.app_data_dir,
            fixture_evidence_terminal_event(
                descriptor,
                publication_origin.as_deref(),
                None,
                Some(reason),
            ),
        ) {
            tracing::error!(event = "fixture_terminal_event_write_failed", error = %error);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist terminal fixture event",
            )
                .into_response();
        }
        tracing::warn!(event = "fixture_run_incomplete", reason);
        tracing::info!(
            event = "fixture_run_finished",
            run_status = "incomplete",
            incomplete_reason = reason,
            publication_origin,
            fixture_id = descriptor.id,
            result_contract = descriptor.result_contract,
            fixture_sha256 = descriptor.expected_sha256,
            fixture_blake3 = descriptor.blake3,
            policy_profile = descriptor.policy_profile,
        );
        return StatusCode::NO_CONTENT.into_response();
    }
    let Some(publication_origin) = publication_origin else {
        return (StatusCode::CONFLICT, "no publication is open").into_response();
    };
    let persisted = {
        let descriptor = descriptor.clone();
        let app_data_dir = runner.app_data_dir.clone();
        let publication_origin = publication_origin.clone();
        tokio::task::spawn_blocking(move || {
            persist_fixture_evidence(&app_data_dir, &descriptor, &publication_origin, submission)
        })
        .await
    };
    let artifact = match persisted {
        Ok(Ok(artifact)) => artifact,
        Ok(Err(error)) => {
            tracing::error!(event = "fixture_evidence_write_failed", error = %error);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist fixture evidence",
            )
                .into_response();
        }
        Err(_) => {
            tracing::error!(
                event = "fixture_evidence_write_failed",
                error_kind = "worker"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not persist fixture evidence",
            )
                .into_response();
        }
    };
    if let Err(error) = persist_fixture_terminal_event(
        &runner.app_data_dir,
        fixture_evidence_terminal_event(
            descriptor,
            Some(&publication_origin),
            Some(&artifact),
            None,
        ),
    ) {
        tracing::error!(event = "fixture_terminal_event_write_failed", error = %error);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not persist terminal fixture event",
        )
            .into_response();
    }
    tracing::info!(
        event = "fixture_run_finished",
        run_status = "complete",
        publication_origin,
        fixture_id = descriptor.id,
        result_contract = descriptor.result_contract,
        fixture_sha256 = descriptor.expected_sha256,
        fixture_blake3 = descriptor.blake3,
        policy_profile = descriptor.policy_profile,
        evidence_file = "fixture-evidence.json",
        evidence_sha256 = artifact.sha256,
        evidence_bytes = artifact.bytes,
        evidence_documents = artifact.document_count,
    );
    StatusCode::NO_CONTENT.into_response()
}

fn log_finished(
    descriptor: &RunnerFixtureDescriptor,
    submission: &ConformanceSubmission,
    run_status: &str,
    publication_origin: Option<&str>,
    artifact: Option<&crate::artifact::PersistedPublicationObservations>,
) {
    tracing::info!(
        event = "fixture_run_finished",
        run_status,
        publication_origin,
        fixture_id = descriptor.id,
        fixture_label = descriptor.label,
        result_contract = descriptor.result_contract,
        fixture_sha256 = descriptor.expected_sha256,
        fixture_blake3 = descriptor.blake3,
        policy_profile = descriptor.policy_profile,
        passed = submission.passed,
        total = submission.total,
        failure_count = submission.failures.len(),
        failures = ?submission.failures,
        viewport_width = submission.capture.as_ref().map(|capture| capture.viewport_width),
        viewport_height = submission.capture.as_ref().map(|capture| capture.viewport_height),
        capture_x = submission.capture.as_ref().map(|capture| capture.x),
        capture_y = submission.capture.as_ref().map(|capture| capture.y),
        capture_width = submission.capture.as_ref().map(|capture| capture.width),
        capture_height = submission.capture.as_ref().map(|capture| capture.height),
        publication_observations_file = artifact.map(|_| "fixture-publication-observations.json"),
        publication_observations_sha256 = artifact.map(|artifact| artifact.sha256.as_str()),
        publication_observations_bytes = artifact.map(|artifact| artifact.bytes),
        publication_observations_phase_count = artifact.map(|artifact| artifact.phase_count),
    );
}
