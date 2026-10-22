//! Navigator diagnostics and client-error intake.
//!
//! Accepted records use closed event types, contiguous sequence numbers, and
//! bounded values, as required by the production plan under "Diagnostics And
//! Privacy". Nothing here retains publication metadata, authored content,
//! target data, credentials, or reader-session identifiers.

use crate::network_activity;
use crate::{
    ActiveReaderSession, DiagnosticsLevel, MAX_DIAGNOSTIC_BATCH_RECORDS, MAX_DIAGNOSTIC_COUNT,
    MAX_DIAGNOSTIC_DURATION_MS, MAX_DIAGNOSTIC_REQUEST_BYTES, ReaderState, Trusted, TrustedWrite,
    duration_milliseconds, has_exact_header, log_performance,
};
use axum::routing::get;
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::State, http::HeaderMap, http::StatusCode,
    http::header::CONTENT_TYPE, response::IntoResponse, response::Response, routing::post,
};
use serde::{Deserialize, Serialize};

use std::{
    sync::Arc, sync::Mutex, sync::Weak, sync::atomic::AtomicU64, sync::atomic::Ordering,
    time::Instant,
};

/// Diagnostics intake on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route(
            "/native/reader/network-activity",
            get(network_activity::read),
        )
        .route(
            "/native/diagnostics/v1",
            post(native_diagnostics).layer(DefaultBodyLimit::max(MAX_DIAGNOSTIC_REQUEST_BYTES)),
        )
        .route("/native/client-error", post(native_client_error))
}

pub(crate) struct NativeDiagnostics {
    pub(crate) started: Instant,
    pub(crate) process_sequence: AtomicU64,
    pub(crate) receipt: Mutex<DiagnosticReceipt>,
}

#[derive(Default)]
pub(crate) struct DiagnosticReceipt {
    session: Option<Weak<ActiveReaderSession>>,
    next_sequence: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DiagnosticBatch {
    pub(crate) version: u8,
    pub(crate) first_sequence: u64,
    pub(crate) records: Vec<DiagnosticRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum DiagnosticRecord {
    FontEvidence {
        target: FontEvidenceTarget,
        family: FontEvidenceFamily,
        frame: Option<u64>,
        configured: bool,
        declared: bool,
        loaded: bool,
        check: bool,
    },
    ReaderStatus {
        status: ReaderDiagnosticStatus,
    },
    PaginationRun {
        action: RunAction,
        run: u64,
        outcome: RunOutcome,
        duration_ms: f64,
        measured_occurrences: u64,
        total_occurrences: u64,
        measured_page_count: u64,
        total_page_count: Option<u64>,
        measured_targets: u64,
        total_targets: u64,
        page_count_attempts: u64,
        timed_out_occurrences: u64,
        measurement_ms: f64,
    },
    CacheRestore {
        action: CacheRestoreAction,
        run: u64,
        claimed_occurrences: u64,
        remaining_occurrences: u64,
        duration_ms: f64,
    },
    CacheWrite {
        action: CacheWriteAction,
        operation: u64,
        run: u64,
        occurrence_count: u64,
        operation_count: u64,
        completed_operation_count: u64,
        duration_ms: f64,
    },
    CacheDrain {
        run: u64,
        outcome: CacheDrainOutcome,
        duration_ms: f64,
    },
    FrameLifecycle {
        role: FrameRole,
        occurrence: u64,
        frame: u64,
        outcome: FrameOutcome,
        reason: FrameReason,
        create_to_terminal_ms: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        create_to_load_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        load_to_agent_ready_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_ready_to_commit_ms: Option<f64>,
    },
    Layout {
        action: LayoutAction,
        role: LayoutRole,
        document: u64,
        layout: u64,
        operation: u64,
        phase: LayoutPhaseKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
    },
    LayoutPhase {
        role: FrameRole,
        frame: u64,
        operation: Option<u64>,
        phase: LayoutPhase,
        duration_ms: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        scan_count: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rectangle_count: Option<u64>,
    },
    ControllerTask {
        action: ControllerTaskAction,
        task: ControllerTaskKind,
        occurrence: u64,
        run: u64,
        task_generation: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        document: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        layout: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<ControllerTaskReason>,
    },
    CacheAggregate {
        action: CacheAction,
        record_type: CacheRecordType,
        outcome: CacheOutcome,
        count: u64,
        bytes: u64,
        duration_ms: f64,
    },
    OpenMilestone {
        milestone: OpenMilestone,
    },
    ReadingPosition {
        outcome: ReadingPositionOutcome,
        reason: Option<ReadingPositionCaptureReason>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientErrorSubmission {
    message: String,
}

async fn native_diagnostics(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: Trusted,
    body: Result<Json<DiagnosticBatch>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !has_exact_header(&headers, CONTENT_TYPE, "application/json") {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "invalid content type").into_response();
    }
    let _operation = state.lifecycle.lock().await;
    let session = match state
        .lifecycle
        .load_active()
        .filter(|session| !session.revoked.load(Ordering::Acquire))
    {
        Some(session) => session,
        None => return (StatusCode::UNAUTHORIZED, "reader session is unavailable").into_response(),
    };
    let Ok(Json(batch)) = body else {
        return (StatusCode::BAD_REQUEST, "invalid diagnostic batch").into_response();
    };
    if batch.version != 1
        || batch.records.is_empty()
        || batch.records.len() > MAX_DIAGNOSTIC_BATCH_RECORDS
        || batch.records.iter().any(|record| {
            !record.is_valid()
                || (matches!(state.host_info.diagnostics_level, DiagnosticsLevel::Summary)
                    && record.is_trace_only())
        })
    {
        return (StatusCode::BAD_REQUEST, "invalid diagnostic batch").into_response();
    }

    let mut receipt = state
        .diagnostics
        .receipt
        .lock()
        .expect("diagnostic receipt lock poisoned");
    let same_session = receipt
        .session
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|current| Arc::ptr_eq(&current, &session));
    if !same_session {
        receipt.session = Some(Arc::downgrade(&session));
        receipt.next_sequence = 1;
    }
    if batch.first_sequence != receipt.next_sequence {
        return (StatusCode::CONFLICT, "invalid diagnostic sequence").into_response();
    }
    let Some(next_sequence) = batch
        .first_sequence
        .checked_add(u64::try_from(batch.records.len()).unwrap_or(u64::MAX))
    else {
        return (StatusCode::BAD_REQUEST, "invalid diagnostic sequence").into_response();
    };

    for (offset, record) in batch.records.into_iter().enumerate() {
        let process_sequence = state
            .diagnostics
            .process_sequence
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let session_sequence = batch.first_sequence + u64::try_from(offset).unwrap_or(u64::MAX);
        let evidence_record = if matches!(&record, DiagnosticRecord::OpenMilestone { .. }) {
            serde_json::json!({
                "event": "openMilestone",
                "milestone": "first-visible",
                "correlated": !session.trace_id.is_empty(),
            })
        } else {
            serde_json::to_value(record).expect("diagnostic record must serialize")
        };
        log_performance(serde_json::json!({
            "schema": "elephant-ladder.native-diagnostics",
            "version": 1,
            "processSequence": process_sequence,
            "sessionSequence": session_sequence,
            "receiptMonotonicMs": duration_milliseconds(state.diagnostics.started.elapsed()),
            "record": evidence_record,
        }));
    }
    receipt.next_sequence = next_sequence;
    StatusCode::NO_CONTENT.into_response()
}

impl DiagnosticRecord {
    fn is_trace_only(&self) -> bool {
        matches!(
            self,
            Self::FrameLifecycle { .. }
                | Self::Layout { .. }
                | Self::LayoutPhase { .. }
                | Self::ControllerTask { .. }
                | Self::CacheAggregate { .. }
        )
    }

    pub(crate) fn is_valid(&self) -> bool {
        let count = |value: u64| value <= MAX_DIAGNOSTIC_COUNT;
        let duration =
            |value: f64| value.is_finite() && (0.0..=MAX_DIAGNOSTIC_DURATION_MS).contains(&value);
        let optional_duration = |value: &Option<f64>| value.is_none_or(duration);
        let optional_count = |value: &Option<u64>| value.is_none_or(count);
        match self {
            Self::ReaderStatus { .. }
            | Self::OpenMilestone { .. }
            | Self::ReadingPosition { .. } => true,
            Self::FontEvidence {
                target,
                family,
                frame,
                ..
            } => match (target, family, frame) {
                (FontEvidenceTarget::Interface, FontEvidenceFamily::InterVariable, None) => true,
                (
                    FontEvidenceTarget::Publication,
                    FontEvidenceFamily::NavigatorReaderSerif,
                    Some(frame),
                ) => *frame > 0 && count(*frame),
                _ => false,
            },
            Self::PaginationRun {
                run,
                duration_ms,
                measured_occurrences,
                total_occurrences,
                measured_page_count,
                total_page_count,
                measured_targets,
                total_targets,
                page_count_attempts,
                timed_out_occurrences,
                measurement_ms,
                ..
            } => {
                *run > 0
                    && count(*run)
                    && duration(*duration_ms)
                    && count(*total_occurrences)
                    && *measured_occurrences <= *total_occurrences
                    && count(*measured_page_count)
                    && total_page_count
                        .is_none_or(|total| count(total) && *measured_page_count <= total)
                    && count(*total_targets)
                    && *measured_targets <= *total_targets
                    && count(*page_count_attempts)
                    && *timed_out_occurrences <= *total_occurrences
                    && duration(*measurement_ms)
            }
            Self::CacheRestore {
                run,
                claimed_occurrences,
                remaining_occurrences,
                duration_ms,
                ..
            } => {
                *run > 0
                    && count(*run)
                    && count(*claimed_occurrences)
                    && count(*remaining_occurrences)
                    && duration(*duration_ms)
            }
            Self::CacheWrite {
                operation,
                run,
                occurrence_count,
                operation_count,
                completed_operation_count,
                duration_ms,
                ..
            } => {
                *operation > 0
                    && *run > 0
                    && count(*operation)
                    && count(*run)
                    && count(*occurrence_count)
                    && count(*operation_count)
                    && *completed_operation_count <= *operation_count
                    && duration(*duration_ms)
            }
            Self::CacheDrain {
                run, duration_ms, ..
            } => *run > 0 && count(*run) && duration(*duration_ms),
            Self::FrameLifecycle {
                occurrence,
                frame,
                create_to_terminal_ms,
                create_to_load_ms,
                load_to_agent_ready_ms,
                agent_ready_to_commit_ms,
                ..
            } => {
                count(*occurrence)
                    && *frame > 0
                    && count(*frame)
                    && duration(*create_to_terminal_ms)
                    && optional_duration(create_to_load_ms)
                    && optional_duration(load_to_agent_ready_ms)
                    && optional_duration(agent_ready_to_commit_ms)
            }
            Self::Layout {
                document,
                layout,
                operation,
                duration_ms,
                ..
            } => {
                count(*document)
                    && count(*layout)
                    && count(*operation)
                    && optional_duration(duration_ms)
            }
            Self::LayoutPhase {
                frame,
                operation,
                duration_ms,
                scan_count,
                rectangle_count,
                ..
            } => {
                count(*frame)
                    && optional_count(operation)
                    && duration(*duration_ms)
                    && optional_count(scan_count)
                    && optional_count(rectangle_count)
            }
            Self::ControllerTask {
                occurrence,
                run,
                task_generation,
                document,
                layout,
                ..
            } => {
                count(*occurrence)
                    && *run > 0
                    && count(*run)
                    && count(*task_generation)
                    && optional_count(document)
                    && optional_count(layout)
            }
            Self::CacheAggregate {
                count: event_count,
                bytes,
                duration_ms,
                ..
            } => *event_count > 0 && count(*event_count) && count(*bytes) && duration(*duration_ms),
        }
    }
}

async fn native_client_error(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<ClientErrorSubmission>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(submission) = match body {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response(),
    };
    if submission.message.is_empty() || submission.message.len() > 1000 {
        return (StatusCode::BAD_REQUEST, "invalid client error").into_response();
    }
    if state.contributed {
        tracing::warn!(
            event = "reader_client_error_reported",
            message = submission.message
        );
    } else {
        tracing::warn!(event = "reader_client_error_reported");
    }
    StatusCode::NO_CONTENT.into_response()
}

macro_rules! diagnostic_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub(crate) enum $name { $($variant),+ }
    };
}

diagnostic_enum!(ReaderDiagnosticStatus {
    Ready,
    Network,
    Loading,
    Paginating,
    Issue
});

diagnostic_enum!(FontEvidenceTarget {
    Interface,
    Publication
});

diagnostic_enum!(FontEvidenceFamily {
    InterVariable,
    NavigatorReaderSerif
});

diagnostic_enum!(RunAction { Start, Terminal });

diagnostic_enum!(RunOutcome {
    Running,
    Complete,
    Unavailable,
    Superseded,
    Cancelled
});

diagnostic_enum!(CacheRestoreAction {
    Start,
    Complete,
    Stalled,
    Cancelled
});

diagnostic_enum!(CacheWriteAction {
    Start,
    Complete,
    Cancelled,
    Failed
});

diagnostic_enum!(CacheDrainOutcome {
    Durable,
    Failed,
    Cancelled
});

diagnostic_enum!(FrameRole {
    Foreground,
    Auxiliary,
    Prepared
});

diagnostic_enum!(FrameOutcome {
    Committed,
    Prepared,
    Failed,
    Cancelled
});

diagnostic_enum!(FrameReason {
    Complete,
    LoadError,
    UnexpectedNavigation,
    EffectExecutionFailed,
    ReadinessTimeout,
    LayoutConflict,
    LayoutUnavailable,
    Superseded,
    Closed
});

diagnostic_enum!(LayoutAction {
    Start,
    Settle,
    Cancel,
    Discard
});

diagnostic_enum!(LayoutRole {
    Foreground,
    Auxiliary
});

diagnostic_enum!(LayoutPhaseKind { Initial, Update });

diagnostic_enum!(LayoutPhase {
    Capture,
    ContentEndScan,
    TextAssessment,
    Settlement,
    Observation,
    LiveObservation
});

diagnostic_enum!(ControllerTaskAction {
    Schedule,
    Start,
    Pause,
    Requeue,
    Complete,
    Discard
});

diagnostic_enum!(ControllerTaskKind { PageCount, Toc });

diagnostic_enum!(ControllerTaskReason {
    BasisChange,
    Close,
    ForegroundWork,
    Hidden,
    LayoutInvalidated,
    OwnerSuperseded,
    Timeout,
    DeliveryFailed,
    StaleGeneration,
    Unstable
});

diagnostic_enum!(CacheAction {
    Read,
    Write,
    Remove
});

diagnostic_enum!(CacheRecordType {
    Page,
    TocTarget,
    Unknown
});

diagnostic_enum!(CacheOutcome {
    Hit,
    Miss,
    Complete,
    Cancelled,
    Failed,
    Invalid
});

diagnostic_enum!(OpenMilestone { FirstVisible });

diagnostic_enum!(ReadingPositionOutcome {
    Unavailable,
    Unsupported,
    Ambiguous,
    Conflict,
    Stale,
    Failed,
    Cancelled,
    WriteFailed
});

diagnostic_enum!(ReadingPositionCaptureReason {
    RuntimeUnavailable,
    CurrentLocationUnavailable,
    NotAuthored,
    NoExactSelector,
    NoExactMovement,
    DecorationNotRendered
});
