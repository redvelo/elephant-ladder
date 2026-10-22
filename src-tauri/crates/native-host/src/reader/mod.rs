//! The trusted reader origin: bootstrap, assets, and its response policy.
//!
//! Initial delivery uses an unguessable single-use navigation URL rather than a
//! credential-bearing query to a reusable page. The response consumes its nonce,
//! embeds escaped inert bootstrap JSON, and loads a fixed same-origin startup
//! module; it is no-store, redirect-free, frame-denied, and contains no inline
//! executable script. Consumed bootstrap and recovery paths return 404 on replay.

use crate::device::NativeActionsAvailability;
use crate::{
    DiagnosticReceipt, DiagnosticsLevel, DisplayCapabilities, NATIVE_MEASUREMENT_REVISION,
    ReaderState, ReadingPosition, ResolvedPublicationPolicy, constant_time_equal, content_type,
    dictionary, generate_session_token, has_exact_header, http::not_found,
    http::response_with_bytes, http::session_headers, media_overlay, storage,
};
use axum::Router;
use axum::routing::get;
use axum::{
    body::Body, extract::Path, extract::State, http::HeaderMap, http::HeaderValue, http::Request,
    http::StatusCode, http::header::CACHE_CONTROL, http::header::CONTENT_SECURITY_POLICY,
    http::header::HOST, http::header::ORIGIN, middleware::Next, response::IntoResponse,
    response::Response,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use std::{sync::Arc, sync::atomic::Ordering};

mod assets;
mod navigation;

pub(crate) use assets::{READER_ASSETS, READER_FONT_BUNDLE_ID, reader_asset_bytes, reader_assets};
pub use navigation::{ContentNavigationPolicy, NavigationRecovery};
pub(crate) static CONTENT_AGENT: &[u8] =
    include_bytes!("../../../../../../navigator/dist/content-agent.js");
pub(crate) static CONTENT_AGENT_SHA256: OnceLock<String> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct NavigationTicket {
    pub(crate) nonce: String,
    pub(crate) bootstrap: NavigationBootstrap,
}

#[derive(Clone)]
pub(crate) enum NavigationBootstrap {
    Library { document_id: String },
    Reading(Box<ReadingBootstrapSeed>),
}

/// Reader faces the single served stylesheet carries. A face the build cannot serve is not
/// offered, so this travels as a list rather than one flag per face.
pub(crate) const READER_TYPEFACES: [&str; 2] = ["reader-serif", "reader-sans"];

#[derive(Clone)]
pub(crate) struct ReadingBootstrapSeed {
    pub(crate) reader_session_id: String,
    pub(crate) publication_id: String,
    pub(crate) publication_origin: String,
    pub(crate) measurement_origin: String,
    pub(crate) publication: Value,
    pub(crate) initial_reading_position: Option<ReadingPosition>,
    pub(crate) position_revision: u32,
    pub(crate) annotations: storage::AnnotationProjection,
    pub(crate) annotation_identifiers: Vec<String>,
    pub(crate) preferences: storage::ReaderPreferences,
    pub(crate) preference_revisions: storage::PreferenceRevisions,
    pub(crate) policy: ResolvedPublicationPolicy,
    pub(crate) pagination: Value,
    pub(crate) media_overlay: media_overlay::MediaOverlayCapability,
    pub(crate) dictionary: Option<dictionary::DictionaryCapability>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryBootstrapResponse {
    pub(crate) display: DisplayCapabilities,
    pub(crate) mode: &'static str,
    pub(crate) library_document_id: String,
    pub(crate) reader_session_id: String,
    pub(crate) launch_id: String,
    pub(crate) reader_origin: String,
    pub(crate) asset_origin: String,
    pub(crate) session_token: String,
    pub(crate) tauri_ipc_enabled: bool,
    pub(crate) native_actions: NativeActionsAvailability,
    pub(crate) preferences: storage::ReaderPreferences,
    pub(crate) preference_revisions: storage::PreferenceRevisions,
    pub(crate) reader_typefaces: Vec<&'static str>,
    pub(crate) diagnostics_level: DiagnosticsLevel,
    pub(crate) dictionaries: DictionariesSummary,
    pub(crate) recovery_navigation_path: String,
    /// What this build calls itself, so the library can say which one is running.
    pub(crate) application: ApplicationIdentity,
}

/// The running build, reported by the host rather than the page: they ship together, and the
/// host is the half that knows what it is.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplicationIdentity {
    pub(crate) name: String,
    pub(crate) version: &'static str,
}

/// What the library needs to show a dictionaries entry point without reading the catalog.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DictionariesSummary {
    pub(crate) installed: usize,
    pub(crate) operation_active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadingBootstrapResponse<'a> {
    pub(crate) display: DisplayCapabilities,
    pub(crate) mode: &'static str,
    pub(crate) library_document_id: Option<String>,
    pub(crate) reader_session_id: String,
    pub(crate) launch_id: String,
    pub(crate) reader_origin: String,
    pub(crate) publication_origin: String,
    pub(crate) asset_origin: String,
    pub(crate) measurement_origin: String,
    pub(crate) session_token: String,
    pub(crate) tauri_ipc_enabled: bool,
    pub(crate) native_actions: NativeActionsAvailability,
    pub(crate) publication_id: String,
    pub(crate) publication: Value,
    pub(crate) initial_reading_position: Option<ReadingPosition>,
    pub(crate) position_revision: u32,
    pub(crate) annotations: storage::PublicAnnotationProjection<'a>,
    pub(crate) annotation_identifiers: Vec<String>,
    pub(crate) preferences: storage::ReaderPreferences,
    pub(crate) preference_revisions: storage::PreferenceRevisions,
    pub(crate) policy: ResolvedPublicationPolicy,
    pub(crate) reader_typefaces: Vec<&'static str>,
    pub(crate) pagination: Value,
    pub(crate) media_overlay: media_overlay::MediaOverlayCapability,
    pub(crate) dictionary: Option<dictionary::DictionaryCapability>,
    pub(crate) diagnostics_level: DiagnosticsLevel,
    pub(crate) recovery_navigation_path: String,
}

pub(crate) async fn reader_policy(
    State(state): State<ReaderState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if !has_exact_header(request.headers(), HOST, &state.reader_authority) {
        return (StatusCode::BAD_REQUEST, "invalid authority").into_response();
    }
    let mut response = next.run(request).await;
    if response.headers().contains_key(CONTENT_SECURITY_POLICY) {
        session_headers(response.headers_mut(), None);
    } else {
        session_headers(response.headers_mut(), Some(&reader_csp(&[], &[], &[])));
    }
    response
}

pub fn reader_csp(
    frame_origins: &[&str],
    image_origins: &[&str],
    media_origins: &[&str],
) -> String {
    let frame_src = if frame_origins.is_empty() {
        "'none'".to_owned()
    } else {
        frame_origins.join(" ")
    };
    let image_src = if image_origins.is_empty() {
        "'self' data: blob:".to_owned()
    } else {
        format!("'self' data: blob: {}", image_origins.join(" "))
    };
    let media_src = if media_origins.is_empty() {
        "'self' blob:".to_owned()
    } else {
        format!("'self' blob: {}", media_origins.join(" "))
    };
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; font-src 'self'; img-src {image_src}; media-src {media_src}; connect-src 'self'; frame-src {frame_src}; worker-src 'self'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
}

/// The reader page itself: bootstrap, its own assets, and the content agent.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route("/window/{nonce}/start", get(reader_start))
        .route("/public/content-agent/{asset}", get(content_agent))
        .route("/public/reader-fonts/{*path}", get(reader_font_asset))
}

pub(crate) async fn reader_asset(Path(path): Path<String>) -> Response {
    if !valid_asset_path(&path) {
        return not_found().await;
    }
    tracing::debug!(event = "reader_asset_requested");
    static_reader_response(&path)
}

pub(crate) fn static_reader_response(path: &str) -> Response {
    let Some(asset) = reader_asset_bytes(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mut response = response_with_bytes(StatusCode::OK, content_type(path), &asset, None);
    if path.starts_with("assets/") {
        response.headers_mut().insert(
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    }
    response
}

pub(crate) fn content_agent_sha256() -> &'static str {
    CONTENT_AGENT_SHA256
        .get_or_init(|| format!("{:x}", Sha256::digest(CONTENT_AGENT)))
        .as_str()
}

pub fn content_agent_path() -> String {
    format!("/public/content-agent/{}.js", content_agent_sha256())
}

pub fn content_agent_url(reader_origin: &str) -> String {
    format!("{reader_origin}{}", content_agent_path())
}

/// What a publication frame is measured with, as one digest.
///
/// Only the content agent and the reader-font bundle load inside a publication document, and
/// both are already content addressed in the URLs they are served under, so a changed agent or
/// font set moves this with nothing bumped by hand. The application bundle, its stylesheet and
/// its interface fonts never enter a publication frame and are deliberately absent: naming them
/// would discard every measured page on the device whenever the interface changed.
///
/// Parts are length prefixed so no value containing a separator can forge a boundary.
pub(crate) fn measurement_basis_digest() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            let mut digest = Sha256::new();
            for part in [
                content_agent_sha256(),
                reader_font_bundle_id()
                    .expect("reader build must contain one versioned font bundle"),
                NATIVE_MEASUREMENT_REVISION,
            ] {
                digest.update((part.len() as u64).to_be_bytes());
                digest.update(part.as_bytes());
            }
            format!("{:x}", digest.finalize())
        })
        .as_str()
}

pub(crate) async fn reader_start(
    State(state): State<ReaderState>,
    Path(nonce): Path<String>,
) -> Response {
    let needs_global_preferences = {
        let operation = state.lifecycle.lock().await;
        let Some(ticket) = operation
            .navigation_ticket
            .as_ref()
            .filter(|ticket| constant_time_equal(nonce.as_bytes(), ticket.nonce.as_bytes()))
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        matches!(&ticket.bootstrap, NavigationBootstrap::Library { .. })
    };
    let global_preferences = if needs_global_preferences {
        let storage = state.storage.clone();
        match tokio::task::spawn_blocking(move || storage.locked()?.load_global_preferences()).await
        {
            Ok(Ok(preferences)) => Some(preferences),
            Ok(Err(_error)) => {
                tracing::error!(
                    event = "reader_bootstrap_failed",
                    phase = "preferences",
                    error_kind = "storage"
                );
                return (StatusCode::INTERNAL_SERVER_ERROR, "bootstrap failed").into_response();
            }
            Err(_error) => {
                tracing::error!(
                    event = "reader_bootstrap_failed",
                    phase = "preferences",
                    error_kind = "worker"
                );
                return (StatusCode::INTERNAL_SERVER_ERROR, "bootstrap failed").into_response();
            }
        }
    } else {
        None
    };
    let dictionaries = if needs_global_preferences {
        let storage = state.storage.clone();
        let installed =
            tokio::task::spawn_blocking(move || storage.locked()?.dictionary_installations()).await;
        match installed {
            Ok(Ok(installations)) => Some(DictionariesSummary {
                installed: installations.len(),
                operation_active: state.dictionary_operations.active(),
            }),
            _ => Some(DictionariesSummary {
                installed: 0,
                operation_active: false,
            }),
        }
    } else {
        None
    };
    let mut operation = state.lifecycle.lock().await;
    let (ticket, recovery_navigation_path) = {
        if operation
            .navigation_ticket
            .as_ref()
            .is_none_or(|ticket| !constant_time_equal(nonce.as_bytes(), ticket.nonce.as_bytes()))
        {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        let ticket = operation
            .navigation_ticket
            .as_ref()
            .expect("validated navigation ticket must exist");
        match &ticket.bootstrap {
            NavigationBootstrap::Library { .. } => {
                if state.lifecycle.load_active().is_some() {
                    return (StatusCode::NOT_FOUND, "not found").into_response();
                }
            }
            NavigationBootstrap::Reading(reading) => {
                let current = state.lifecycle.load_active();
                if current.as_ref().is_none_or(|session| {
                    session.reader_session_id.as_ref() != reading.reader_session_id
                        || session.revoked.load(Ordering::Acquire)
                }) {
                    return (StatusCode::NOT_FOUND, "not found").into_response();
                }
            }
        }
        let recovery_nonce = match generate_session_token() {
            Ok(nonce) => nonce,
            Err(_) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "bootstrap failed").into_response();
            }
        };
        let recovery_bootstrap = match &ticket.bootstrap {
            NavigationBootstrap::Library { .. } => {
                let document_id = match generate_session_token() {
                    Ok(id) => id,
                    Err(_) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "bootstrap failed")
                            .into_response();
                    }
                };
                NavigationBootstrap::Library { document_id }
            }
            NavigationBootstrap::Reading(reading) => NavigationBootstrap::Reading(reading.clone()),
        };
        let ticket = operation
            .navigation_ticket
            .replace(NavigationTicket {
                nonce: recovery_nonce.clone(),
                bootstrap: recovery_bootstrap,
            })
            .expect("validated navigation ticket must exist");
        (ticket, format!("/window/{recovery_nonce}/start"))
    };
    drop(operation);
    *state
        .diagnostics
        .receipt
        .lock()
        .expect("diagnostic receipt lock poisoned") = DiagnosticReceipt::default();
    tracing::info!(event = "reader_start_accepted");
    let (json, csp) = match ticket.bootstrap {
        NavigationBootstrap::Library { document_id } => {
            let global_preferences =
                global_preferences.expect("library preferences must be loaded");
            *state
                .library_document_id
                .write()
                .expect("library document lock poisoned") = Some(Arc::from(document_id.as_str()));
            state.latest_open_operation.store(0, Ordering::Release);
            let bootstrap = LibraryBootstrapResponse {
                display: state.native_actions.display_capabilities(),
                mode: "library",
                library_document_id: document_id,
                reader_session_id: state.reader_session_id.to_string(),
                launch_id: state.launch_id.to_string(),
                reader_origin: state.reader_origin.to_string(),
                asset_origin: state.reader_origin.to_string(),
                session_token: state.token.to_string(),
                tauri_ipc_enabled: false,
                native_actions: NativeActionsAvailability {
                    front_light_dialog: state.native_actions.front_light_dialog_available(),
                    side_buttons: state.native_actions.side_buttons_available(),
                    window_fullscreen: state.native_actions.window_fullscreen_available(),
                },
                preferences: global_preferences.preferences,
                preference_revisions: global_preferences.revisions,
                reader_typefaces: READER_TYPEFACES.to_vec(),
                diagnostics_level: state.host_info.diagnostics_level,
                dictionaries: dictionaries.unwrap_or(DictionariesSummary {
                    installed: 0,
                    operation_active: false,
                }),
                recovery_navigation_path,
                application: ApplicationIdentity {
                    name: state.host_info.application.clone(),
                    version: env!("CARGO_PKG_VERSION"),
                },
            };
            (serialize_bootstrap(&bootstrap), reader_csp(&[], &[], &[]))
        }
        NavigationBootstrap::Reading(reading) => {
            *state
                .library_document_id
                .write()
                .expect("library document lock poisoned") = None;
            let csp = reader_csp(
                &[&reading.publication_origin, &reading.measurement_origin],
                &[&reading.publication_origin],
                &[&reading.publication_origin],
            );
            let ReadingBootstrapSeed {
                reader_session_id,
                publication_id,
                publication_origin,
                measurement_origin,
                publication,
                initial_reading_position,
                position_revision,
                annotations,
                annotation_identifiers,
                preferences,
                preference_revisions,
                policy,
                pagination,
                media_overlay,
                dictionary,
            } = *reading;
            let bootstrap = ReadingBootstrapResponse {
                display: state.native_actions.display_capabilities(),
                mode: "reading",
                library_document_id: None,
                reader_session_id,
                launch_id: state.launch_id.to_string(),
                reader_origin: state.reader_origin.to_string(),
                publication_origin,
                asset_origin: state.reader_origin.to_string(),
                measurement_origin,
                session_token: state.token.to_string(),
                tauri_ipc_enabled: false,
                native_actions: NativeActionsAvailability {
                    front_light_dialog: state.native_actions.front_light_dialog_available(),
                    side_buttons: state.native_actions.side_buttons_available(),
                    window_fullscreen: state.native_actions.window_fullscreen_available(),
                },
                publication_id,
                publication,
                initial_reading_position,
                position_revision,
                annotations: annotations.public(),
                annotation_identifiers,
                preferences,
                preference_revisions,
                policy,
                reader_typefaces: READER_TYPEFACES.to_vec(),
                pagination,
                media_overlay,
                dictionary,
                diagnostics_level: state.host_info.diagnostics_level,
                recovery_navigation_path,
            };
            (serialize_bootstrap(&bootstrap), csp)
        }
    };
    let json = match json {
        Ok(json) => json,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "bootstrap failed").into_response(),
    };
    let Some(index) = reader_asset_bytes("index.html") else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "reader index missing").into_response();
    };
    let Ok(index) = std::str::from_utf8(&index) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "reader index is not UTF-8",
        )
            .into_response();
    };
    let Some(insertion) = index.find("</head>") else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "reader index has no head",
        )
            .into_response();
    };
    let bootstrap_element =
        format!(r#"<script id="native-bootstrap" type="application/json">{json}</script>"#);
    let html = format!(
        "{}{}{}",
        &index[..insertion],
        bootstrap_element,
        &index[insertion..]
    );
    response_with_bytes(
        StatusCode::OK,
        "text/html; charset=utf-8",
        html.as_bytes(),
        Some(&csp),
    )
}

pub fn serialize_bootstrap<T: Serialize>(bootstrap: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string(bootstrap)
        .map(|json| json.replace('<', "\\u003c").replace('>', "\\u003e"))
}

pub(crate) async fn content_agent(
    State(state): State<ReaderState>,
    Path(asset): Path<String>,
    headers: HeaderMap,
) -> Response {
    if asset != format!("{}.js", content_agent_sha256()) {
        return not_found().await;
    }
    tracing::debug!(event = "navigator_asset_served", asset = "content-agent");
    // Chromium omits Origin when an SVG document loads a cross-origin script.
    if !headers.contains_key(ORIGIN) || has_exact_header(&headers, ORIGIN, "null") {
        return opaque_public_javascript(CONTENT_AGENT);
    }
    public_javascript(&state, &headers, CONTENT_AGENT)
}

pub fn opaque_public_javascript(bytes: &[u8]) -> Response {
    let mut response = response_with_bytes(
        StatusCode::OK,
        "text/javascript; charset=utf-8",
        bytes,
        None,
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
        .headers_mut()
        .insert("access-control-allow-origin", HeaderValue::from_static("*"));
    response
        .headers_mut()
        .insert("vary", HeaderValue::from_static("Origin"));
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    response
}

pub(crate) async fn reader_font_asset(Path(path): Path<String>) -> Response {
    let Some((bundle_id, resource_path)) = path.split_once('/') else {
        return not_found().await;
    };
    let valid = reader_font_bundle_id().is_some_and(|current| current == bundle_id)
        && (resource_path == "reader-serif.css"
            || resource_path.strip_prefix("files/").is_some_and(|file| {
                !file.is_empty() && !file.contains('/') && file.ends_with(".woff2")
            }));
    if !valid {
        return not_found().await;
    }
    let asset_path = format!("public/reader-fonts/{path}");
    let Some(asset) = reader_asset_bytes(&asset_path) else {
        return not_found().await;
    };
    let mut response = response_with_bytes(
        StatusCode::OK,
        if path.ends_with(".css") {
            "text/css; charset=utf-8"
        } else {
            "font/woff2"
        },
        &asset,
        None,
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
        .headers_mut()
        .insert("access-control-allow-origin", HeaderValue::from_static("*"));
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    response
}

fn public_resource_origin(state: &ReaderState, headers: &HeaderMap) -> Option<HeaderValue> {
    let active = state.lifecycle.load_active()?;
    if active.revoked.load(Ordering::Acquire) {
        return None;
    }
    if has_exact_header(headers, ORIGIN, &active.authorities.foreground_origin) {
        HeaderValue::from_str(&active.authorities.foreground_origin).ok()
    } else if has_exact_header(headers, ORIGIN, &active.authorities.nested_origin) {
        HeaderValue::from_str(&active.authorities.nested_origin).ok()
    } else if has_exact_header(headers, ORIGIN, &active.authorities.measurement_origin) {
        HeaderValue::from_str(&active.authorities.measurement_origin).ok()
    } else {
        None
    }
}

pub fn public_javascript(state: &ReaderState, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let Some(origin) = public_resource_origin(state, headers) else {
        return (StatusCode::FORBIDDEN, "invalid publication origin").into_response();
    };
    let mut response = response_with_bytes(
        StatusCode::OK,
        "text/javascript; charset=utf-8",
        bytes,
        None,
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
        .headers_mut()
        .insert("access-control-allow-origin", origin);
    response
        .headers_mut()
        .insert("vary", HeaderValue::from_static("Origin"));
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );
    response
}

pub fn reader_font_base_url(reader_origin: &str) -> String {
    format!(
        "{reader_origin}/public/reader-fonts/{}",
        reader_font_bundle_id().expect("reader build must contain one versioned font bundle")
    )
}

pub(crate) fn reader_font_bundle_id() -> Option<&'static str> {
    READER_FONT_BUNDLE_ID
        .get_or_init(|| {
            let root = reader_assets().get_dir("public/reader-fonts")?;
            let mut candidates = root.dirs().filter_map(|directory| {
                let name = directory.path().file_name()?.to_str()?;
                (name.len() == 64
                    && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && directory.files().any(|file| {
                        file.path()
                            .file_name()
                            .is_some_and(|file_name| file_name == "reader-serif.css")
                    }))
                .then(|| name.to_owned())
            });
            let candidate = candidates.next()?;
            candidates.next().is_none().then_some(candidate)
        })
        .as_deref()
}

pub(crate) fn valid_asset_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['?', '#', '\\', '%'])
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}
