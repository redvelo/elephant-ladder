use crate::ReaderState;
use crate::http::{Trusted, TrustedJson, TrustedRead};
use crate::library::valid_publication_id;
use crate::request_library_document;
use crate::storage;
use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveSettings {
    revision: u32,
    preferences: storage::ReaderPreferences,
    device: storage::DeviceSettings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ResetSettings {
    revision: u32,
}

/// Side buttons are page buttons on a device whose display is the reason it has them.
fn side_buttons_default(state: &ReaderState) -> bool {
    state.native_actions.display_capabilities().technology == crate::DisplayTechnology::Eink
}

/// The platform decides the keys itself, so it is told what the application settled on.
fn report_side_buttons(state: &ReaderState, settings: &storage::AppSettings) {
    let actions = state.native_actions.clone();
    let enabled = settings.device.side_buttons_turn_pages;
    tokio::task::spawn_blocking(move || {
        if actions.set_side_buttons_turn_pages(enabled).is_err() {
            tracing::debug!(event = "side_buttons_unreported");
        }
    });
}

pub(super) async fn read(State(state): State<ReaderState>, _: TrustedRead) -> Response {
    let storage = state.storage.clone();
    let default = side_buttons_default(&state);
    match tokio::task::spawn_blocking(move || storage.locked()?.load_app_settings(default)).await {
        Ok(Ok(settings)) => {
            report_side_buttons(&state, &settings);
            Json(settings).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "settings unavailable").into_response(),
    }
}

pub(super) async fn save(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    TrustedJson(request): TrustedJson<SaveSettings>,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    if let Err(reason) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, reason).into_response();
    }
    if request.preferences.validate().is_err() {
        return (StatusCode::BAD_REQUEST, "invalid settings").into_response();
    }
    let default = side_buttons_default(&state);
    match state
        .storage
        .with("app_settings_save", move |storage| {
            let saved = storage.save_app_settings(
                request.revision,
                &request.preferences,
                request.device,
            )?;
            storage
                .load_app_settings(default)
                .map(|settings| (saved, settings))
        })
        .await
    {
        Ok((saved, settings)) => {
            report_side_buttons(&state, &settings);
            (
                if saved {
                    StatusCode::OK
                } else {
                    StatusCode::CONFLICT
                },
                Json(settings),
            )
                .into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "could not save settings").into_response(),
    }
}

/// Returns the reading preferences to the application defaults under the same compare-and-swap
/// the ordinary save uses, so a reset cannot silently discard a concurrent change.
pub(super) async fn reset(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    TrustedJson(request): TrustedJson<ResetSettings>,
) -> Response {
    let _operation = state.lifecycle.lock().await;
    if let Err(reason) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, reason).into_response();
    }
    let default = side_buttons_default(&state);
    match state
        .storage
        .with("app_settings_reset", move |storage| {
            let reset = storage.reset_app_reading_preferences(request.revision)?;
            storage
                .load_app_settings(default)
                .map(|settings| (reset, settings))
        })
        .await
    {
        Ok((reset, settings)) => {
            report_side_buttons(&state, &settings);
            (
                if reset {
                    StatusCode::OK
                } else {
                    StatusCode::CONFLICT
                },
                Json(settings),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not reset settings",
        )
            .into_response(),
    }
}

/// Drops one publication's own layout so it follows the application defaults again. Refused
/// while a reader is active, because a live session holds the revisions this removes.
pub(super) async fn clear_publication_preferences(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    _: Trusted,
) -> Response {
    let operation = state.lifecycle.lock().await;
    if let Err(reason) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, reason).into_response();
    }
    if operation.reader_active() {
        return (StatusCode::CONFLICT, "publication is open").into_response();
    }
    if !valid_publication_id(&publication_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match state
        .storage
        .with("publication_preferences_clear", move |storage| {
            storage.clear_publication_preferences(&publication_id)
        })
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not clear publication layout",
        )
            .into_response(),
    }
}
