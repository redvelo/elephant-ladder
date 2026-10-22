use crate::http::{TrustedRead, TrustedWrite};
use std::time::{Duration, Instant};

use crate::NarrationCommand;
use crate::NarrationPlayback;
use crate::NarrationPlaybackState;
use crate::NativeActionError;
use crate::Ordering;
use crate::ReaderState;
use crate::has_exact_header;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::{Deserialize, Serialize};

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(250);
const COMMAND_WAIT: Duration = Duration::from_secs(20);
const MAX_LABEL_BYTES: usize = 512;
const MAX_MEDIA_MILLISECONDS: u64 = 24 * 60 * 60 * 1000;
const RATE_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NarrationReport {
    state: ReportedState,
    title: String,
    section: Option<String>,
    position_ms: Option<u64>,
    duration_ms: Option<u64>,
    rate: f32,
}

fn valid_label(value: &str) -> bool {
    value.len() <= MAX_LABEL_BYTES && !value.chars().any(char::is_control)
}

impl NarrationReport {
    fn is_valid(&self) -> bool {
        valid_label(&self.title)
            && self.section.as_deref().is_none_or(valid_label)
            && self
                .duration_ms
                .is_none_or(|duration| duration <= MAX_MEDIA_MILLISECONDS)
            && self.position_ms.is_none_or(|position| {
                position <= self.duration_ms.unwrap_or(MAX_MEDIA_MILLISECONDS)
            })
            && RATE_RANGE.contains(&self.rate)
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ReportedState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Serialize)]
struct CommandBatch {
    commands: Vec<NarrationCommand>,
}

fn session_is_active(state: &ReaderState, headers: &HeaderMap) -> bool {
    state.lifecycle.load_active().is_some_and(|active| {
        !active.revoked.load(Ordering::Acquire)
            && has_exact_header(
                headers,
                "x-elephant-reader-session",
                &active.reader_session_id,
            )
    })
}

pub(crate) async fn report(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedWrite,
    body: Result<Json<NarrationReport>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    if !session_is_active(&state, &headers) {
        return (StatusCode::CONFLICT, "reader session unavailable").into_response();
    }
    let Ok(Json(report)) = body else {
        return (StatusCode::BAD_REQUEST, "invalid narration report").into_response();
    };
    if !report.is_valid() {
        return (StatusCode::BAD_REQUEST, "invalid narration report").into_response();
    }
    let playback = match report.state {
        ReportedState::Playing => Some(NarrationPlaybackState::Playing),
        ReportedState::Paused => Some(NarrationPlaybackState::Paused),
        ReportedState::Stopped => None,
    }
    .map(|playback_state| NarrationPlayback {
        state: playback_state,
        title: &report.title,
        section: report.section.as_deref(),
        position_ms: report.position_ms,
        duration_ms: report.duration_ms,
        rate: report.rate,
    });
    match state.native_actions.set_narration_playback(playback) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(NativeActionError::Unsupported) => {
            (StatusCode::NOT_IMPLEMENTED, "native action unsupported").into_response()
        }
        Err(NativeActionError::Failed(_)) => {
            tracing::warn!(event = "native_narration_playback_failed");
            (StatusCode::SERVICE_UNAVAILABLE, "native action failed").into_response()
        }
    }
}

// A pending fetch completes promptly in a hidden page, unlike throttled timers.
pub(crate) async fn commands(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    let mut response = wait_for_commands(&state, &headers).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn wait_for_commands(state: &ReaderState, headers: &HeaderMap) -> Response {
    let deadline = Instant::now() + COMMAND_WAIT;
    loop {
        if !session_is_active(state, headers) {
            return (
                StatusCode::CONFLICT,
                "reader session is unavailable or stale",
            )
                .into_response();
        }
        let actions = state.native_actions.clone();
        match tokio::task::spawn_blocking(move || actions.take_narration_commands()).await {
            Ok(Ok(commands)) if !commands.is_empty() || Instant::now() >= deadline => {
                return Json(CommandBatch { commands }).into_response();
            }
            Ok(Ok(_)) => {}
            Ok(Err(NativeActionError::Unsupported)) => {
                return (StatusCode::NOT_IMPLEMENTED, "native action unsupported").into_response();
            }
            Ok(Err(NativeActionError::Failed(_))) | Err(_) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "native action failed").into_response();
            }
        }
        tokio::time::sleep(COMMAND_POLL_INTERVAL).await;
    }
}
