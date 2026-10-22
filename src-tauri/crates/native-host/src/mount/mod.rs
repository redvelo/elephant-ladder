//! The publication response plane: mounted origins and the bytes they serve.
//!
//! Snapshot derivation seals one response decision per canonical physical
//! resource, and serving never broadens or reinterprets it. Publication origins
//! support only `GET` and `HEAD`, decode the raw path exactly once, and reject
//! malformed UTF-8, separators, empty or dot segments, controls, and
//! double-decoding traps. Unknown paths return 404: there is no application-shell
//! fallback.

pub(crate) mod instrument;
pub use instrument::instrument_svg_response;
pub use instrument::instrument_xhtml_response;
pub use instrument::nested_publication_base_url;
pub use instrument::publication_document_transformation;
pub(crate) use instrument::{transformed_document, transformed_measurement_document};

pub(crate) mod representation;
pub use representation::PublicationResponseProfile;
pub use representation::publication_representation;
pub(crate) use representation::{
    ResponseRepresentation, apply_image_negotiation, apply_publication_validator,
    measurement_dependency_mime, opaque_container_resource_headers, publication_document_headers,
    publication_not_modified, publication_validator, publication_validator_matches,
};

pub(crate) mod stream;
pub use stream::ResourceStreamContext;
pub use stream::stream_imported_resource;

pub(crate) mod image;
pub(crate) use image::{image_adaptation, image_representation};
pub(crate) mod stylesheet;
pub(crate) use stylesheet::{declares_stylesheet, stylesheet_representation};

mod path;
pub use path::decode_publication_path;
pub use path::image_document_source;

mod range;
pub use range::single_range_header;
pub(crate) use range::{RangeDecision, content_type, parse_range};

pub mod csp;
pub(crate) use csp::{
    measurement_dependency_csp, measurement_document_csp, publication_csp_for_profile,
};

use crate::http::{
    HttpObservation, HttpServerRole, not_found, not_found_response, observe_http, session_headers,
};
use crate::reader::content_agent_url;
use crate::snapshot::{ResponseManifest, ResponseRule};
use crate::{
    ActiveReaderSession, Foreground, IMAGE_DOCUMENT_QUERY, Measurement, SnapshotRegistrySlot,
    TransformCacheKey, apply_reader_cover_cors, apply_reader_page_image_cors,
    duration_milliseconds, has_exact_header, log_performance,
    reader_lifecycle::ActiveReaderSessionView, selected_cover_path,
};
use axum::{
    Router, body::Body, body::Bytes, extract::Extension, extract::OriginalUri, extract::State,
    http::HeaderMap, http::HeaderValue, http::Method, http::Request, http::StatusCode, http::Uri,
    http::header::HOST, http::header::ORIGIN, middleware, middleware::Next, response::IntoResponse,
    response::Response,
};
use epub_stack::EpubPath;
use std::{sync::Arc, sync::atomic::Ordering, time::Instant};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct ImportedPublicationState {
    pub asset_origin: Arc<str>,
    pub active_reader_session: ActiveReaderSessionView,
    pub snapshots: SnapshotRegistrySlot,
    pub stream_admission: Arc<Semaphore>,
    pub transform_admission: Arc<Semaphore>,
}

#[derive(Clone)]
pub struct MeasurementPublicationState {
    pub reader_origin: Arc<str>,
    pub asset_origin: Arc<str>,
    pub active_reader_session: ActiveReaderSessionView,
    pub stream_admission: Arc<Semaphore>,
    pub transform_admission: Arc<Semaphore>,
}

/// Every plane serves one publication over its whole authority, so each router is
/// a single fallback under the same observation layer. They differ only in the
/// role they report and in whether the authority gate runs as a layer or inside
/// the handler.
fn publication_plane_router<S: Clone + Send + Sync + 'static>(
    state: S,
    role: HttpServerRole,
    resource: axum::routing::MethodRouter<S>,
) -> Router {
    Router::new()
        .fallback_service(resource.with_state(state))
        .layer(middleware::from_fn_with_state(
            HttpObservation { role },
            observe_http,
        ))
}

pub fn imported_publication_router(state: ImportedPublicationState) -> Router {
    publication_plane_router(
        state.clone(),
        HttpServerRole::ImportedPublication,
        axum::routing::any(imported_publication_resource).layer(middleware::from_fn_with_state(
            state,
            imported_publication_policy,
        )),
    )
}

pub fn nested_publication_router(state: ImportedPublicationState) -> Router {
    // The nested plane gates inside its handler, because the reply needs the
    // opaque container headers either way.
    publication_plane_router(
        state,
        HttpServerRole::NestedPublication,
        axum::routing::any(nested_publication_resource),
    )
}

pub fn measurement_publication_router(state: MeasurementPublicationState) -> Router {
    publication_plane_router(
        state.clone(),
        HttpServerRole::Measurement,
        axum::routing::any(measurement_publication_resource).layer(middleware::from_fn_with_state(
            state,
            measurement_publication_policy,
        )),
    )
}

/// Which of the active session's authorities a plane answers on.
#[derive(Clone, Copy)]
pub(crate) enum PlaneAuthority {
    Foreground,
    Nested,
    Measurement,
}

/// Who a request on a publication plane may be served from.
///
/// Kept as the outcome rather than a response, so the one gate decides admission
/// and each plane decides how a rejection is rendered.
pub(crate) enum PlaneAdmission {
    Session(Arc<ActiveReaderSession>),
    NoSession,
    WrongAuthority,
}

/// The authority gate every publication plane passes through.
///
/// A request is served only from the live session that owns the exact authority
/// this plane answers on. A revoked session owns nothing, and a HOST that names
/// any other authority is refused rather than served from whichever session
/// happens to be current.
pub(crate) fn admit_plane_request(
    sessions: &ActiveReaderSessionView,
    authority: PlaneAuthority,
    headers: &HeaderMap,
) -> PlaneAdmission {
    let Some(session) = sessions.load() else {
        return PlaneAdmission::NoSession;
    };
    if session.revoked.load(Ordering::Acquire) {
        return PlaneAdmission::NoSession;
    }
    let expected = match authority {
        PlaneAuthority::Foreground => &session.authorities.foreground_authority,
        PlaneAuthority::Nested => &session.authorities.nested_authority,
        PlaneAuthority::Measurement => &session.authorities.measurement_authority,
    };
    if !has_exact_header(headers, HOST, expected) {
        return PlaneAdmission::WrongAuthority;
    }
    PlaneAdmission::Session(session)
}

async fn imported_publication_policy(
    State(state): State<ImportedPublicationState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    gate_plane(
        &state.active_reader_session,
        PlaneAuthority::Foreground,
        request,
        next,
    )
    .await
}

async fn measurement_publication_policy(
    State(state): State<MeasurementPublicationState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    gate_plane(
        &state.active_reader_session,
        PlaneAuthority::Measurement,
        request,
        next,
    )
    .await
}

/// Admits the request and hands the session it was admitted from to the handler.
async fn gate_plane(
    sessions: &ActiveReaderSessionView,
    authority: PlaneAuthority,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    match admit_plane_request(sessions, authority, request.headers()) {
        PlaneAdmission::Session(session) => {
            request.extensions_mut().insert(session);
            next.run(request).await
        }
        PlaneAdmission::NoSession => not_found().await,
        PlaneAdmission::WrongAuthority => {
            (StatusCode::BAD_REQUEST, "invalid authority").into_response()
        }
    }
}

async fn nested_publication_resource(
    State(state): State<ImportedPublicationState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let session = match admit_plane_request(
        &state.active_reader_session,
        PlaneAuthority::Nested,
        &headers,
    ) {
        PlaneAdmission::Session(session) => session,
        PlaneAdmission::NoSession => return not_found().await,
        PlaneAdmission::WrongAuthority => {
            return (StatusCode::BAD_REQUEST, "invalid authority").into_response();
        }
    };
    let mut response = imported_publication_resource_for_profile(
        state,
        session,
        PublicationResponseProfile::ContainerConstrained,
        method,
        uri,
        headers,
    )
    .await;
    opaque_container_resource_headers(response.headers_mut());
    response
}

async fn imported_publication_resource(
    State(state): State<ImportedPublicationState>,
    Extension(session): Extension<Arc<ActiveReaderSession>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    imported_publication_resource_for_profile(
        state,
        session,
        PublicationResponseProfile::SpineLevel,
        method,
        uri,
        headers,
    )
    .await
}

/// The policy this session's publication is served under on one plane.
///
/// Every response on a publication plane carries it, and it is derived from the
/// same six inputs each time, so the planes cannot drift apart in what they admit.
fn session_publication_csp(
    profile: PublicationResponseProfile,
    state: &ImportedPublicationState,
    session: &ActiveReaderSession,
) -> String {
    publication_csp_for_profile(
        profile,
        &state.asset_origin,
        &session.authorities.foreground_origin,
        &session.authorities.nested_origin,
        session.policy,
        session.content_script_nonce.as_deref(),
    )
}

/// The request-shape gates every publication plane applies before it looks at a
/// path: only safe methods, and never a worker script.
///
/// Run before any session check, so a revoked session sees the same refusal for a
/// disallowed method as a live one.
pub fn publication_request_refused(method: &Method, headers: &HeaderMap) -> Option<Response> {
    if method != Method::GET && method != Method::HEAD {
        return Some((StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response());
    }
    blocked_publication_worker(headers)
}

/// Why a plane will not serve this path.
pub enum ResourceRefused {
    NotFound,
    BadPath(&'static str),
}

impl ResourceRefused {
    pub fn into_response(self) -> Response {
        match self {
            Self::NotFound => not_found_response(),
            Self::BadPath(message) => (StatusCode::BAD_REQUEST, message).into_response(),
        }
    }
}

/// One resource a plane has agreed to serve, resolved against a sealed manifest.
pub struct PublicationResource<'a> {
    pub path: EpubPath,
    pub path_text: String,
    pub rule: &'a ResponseRule,
    /// The request asked for the alternate document that wraps a fixed image.
    pub image_document: bool,
}

/// Resolves a request URI to the response rule sealed for it at derivation.
///
/// Every plane decides this the same way: the alternate representation is
/// recognized before the archive path is decoded, the path is never repaired, and
/// a path with no rule — or a rule for bytes that are not there — is not served.
pub fn admit_publication_resource<'a>(
    uri: &Uri,
    responses: &'a ResponseManifest,
) -> Result<PublicationResource<'a>, ResourceRefused> {
    let image_document = uri.query() == Some(IMAGE_DOCUMENT_QUERY);
    let resource_uri = if image_document {
        uri.path()
            .parse::<Uri>()
            .expect("an existing URI path is valid")
    } else {
        uri.clone()
    };
    let path = decode_publication_path(&resource_uri).map_err(ResourceRefused::BadPath)?;
    let path_text = path.as_str().to_owned();
    let rule = responses
        .resources
        .get(&path_text)
        .ok_or(ResourceRefused::NotFound)?;
    if rule.missing || rule.undeclared {
        return Err(ResourceRefused::NotFound);
    }
    Ok(PublicationResource {
        path,
        path_text,
        rule,
        image_document,
    })
}

pub(crate) async fn imported_publication_resource_for_profile(
    state: ImportedPublicationState,
    session: Arc<ActiveReaderSession>,
    profile: PublicationResponseProfile,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = publication_request_refused(&method, &headers) {
        return response;
    }
    if session.revoked.load(Ordering::Acquire) {
        return not_found().await;
    }
    let entry = &session.entry;
    let resource = match admit_publication_resource(&uri, &entry.responses) {
        Ok(resource) => resource,
        Err(refused) => return refused.into_response(),
    };
    let PublicationResource {
        path,
        path_text,
        rule,
        image_document,
    } = resource;
    let document_transformation = publication_document_transformation(profile, rule);
    let adapted_image = if image_document {
        None
    } else {
        image_adaptation(&headers, &rule.mime)
    };
    let representation = if image_document {
        "image-document"
    } else {
        match document_transformation {
            Some(Foreground::Svg) => "svg",
            Some(_) => "xhtml",
            None if adapted_image.is_some() => "image-png",
            None if declares_stylesheet(&rule.mime) => "stylesheet",
            None => "raw",
        }
    };
    // Instrumented representations embed the content-agent URL, the nested publication origin,
    // and the per-session nonce; raw archive bytes do not. Without them a rebuilt agent would
    // revalidate a cached document that still references the previous agent.
    let instrumentation = (representation != "raw").then(|| {
        [
            content_agent_url(&state.asset_origin).as_str(),
            &session.authorities.nested_origin,
            session.content_script_nonce.as_deref().unwrap_or_default(),
        ]
        .join("\n")
    });
    let validator = publication_validator(
        &entry.snapshot_id,
        &path_text,
        representation,
        profile,
        instrumentation.as_deref().unwrap_or_default(),
    );
    if method == Method::GET
        && single_range_header(&headers).is_none()
        && publication_validator_matches(&headers, &validator)
    {
        log_performance(serde_json::json!({
            "event": "publication_resource",
            "traceId": session.trace_id.as_ref(),
            "outcome": "not-modified",
            "representation": representation,
        }));
        return publication_not_modified(
            &validator,
            &session_publication_csp(profile, &state, &session),
        );
    }
    if image_document {
        if !matches!(profile, PublicationResponseProfile::SpineLevel)
            || !matches!(rule.foreground, Foreground::FixedImage)
        {
            return (
                StatusCode::FORBIDDEN,
                "image document is not an effective roll frame",
            )
                .into_response();
        }
        let wrapper_key = TransformCacheKey {
            path: path_text.clone(),
            representation: ResponseRepresentation::ImageDocument,
            profile,
            nested_publication_base_url: None,
            content_script_nonce: session.content_script_nonce.clone(),
        };
        let cached = entry
            .transforms
            .lock()
            .expect("transform cache lock poisoned")
            .get(&wrapper_key);
        let bytes = match cached {
            Some(bytes) => bytes,
            None => {
                let source = image_document_source(&path);
                let instrumented = match instrument_xhtml_response(
                    source.as_bytes(),
                    &state.asset_origin,
                    false,
                    session.content_script_nonce.as_deref(),
                    None,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "image document instrumentation failed",
                        )
                            .into_response();
                    }
                };
                entry
                    .transforms
                    .lock()
                    .expect("transform cache lock poisoned")
                    .insert(wrapper_key, Bytes::from(instrumented))
                    .bytes
            }
        };
        if session.revoked.load(Ordering::Acquire) {
            return not_found().await;
        }
        let csp = session_publication_csp(profile, &state, &session);
        let mut response = publication_representation(
            method == Method::HEAD,
            "application/xhtml+xml; charset=utf-8",
            bytes,
            false,
            single_range_header(&headers),
            &csp,
        );
        apply_reader_cover_cors(
            &mut response,
            &path,
            &headers,
            &state.asset_origin,
            &entry.facts,
        );
        apply_publication_validator(&mut response, &validator);
        return response;
    }
    if let Some(document_transformation) = document_transformation {
        let trace_id = session.trace_id.clone();
        let cover_path = path.clone();
        let started = Instant::now();
        let transformed = transformed_document(
            session.clone(),
            path,
            state.asset_origin.clone(),
            state.transform_admission.clone(),
            document_transformation,
            profile,
            nested_publication_base_url(profile, &session.authorities.nested_origin).map(Arc::from),
        )
        .await;
        return match transformed {
            Ok(outcome) if !session.revoked.load(Ordering::Acquire) => {
                log_performance(serde_json::json!({
                    "event": "publication_document",
                    "traceId": trace_id.as_ref(),
                    "outcome": "ready",
                    "resourceClass": if matches!(document_transformation, Foreground::Svg) { "svg" } else { "xhtml" },
                    "cache": if outcome.hit { "hit" } else if outcome.cached { "miss" } else { "not-admitted" },
                    "evictions": outcome.evictions,
                    "sourceBytes": outcome.source_bytes,
                    "outputBytes": outcome.bytes.len(),
                    "resourceReadMs": outcome.resource_read_ms,
                    "transformMs": outcome.transform_ms,
                    "prewarm": false,
                    "totalMs": duration_milliseconds(started.elapsed()),
                }));
                let csp = session_publication_csp(profile, &state, &session);
                let mut response = publication_representation(
                    method == Method::HEAD,
                    &rule.mime,
                    outcome.bytes,
                    false,
                    single_range_header(&headers),
                    &csp,
                );
                apply_reader_cover_cors(
                    &mut response,
                    &cover_path,
                    &headers,
                    &state.asset_origin,
                    &entry.facts,
                );
                apply_publication_validator(&mut response, &validator);
                response
            }
            Ok(_) => not_found().await,
            Err(_error) => {
                let resource_class = if matches!(document_transformation, Foreground::Svg) {
                    "svg"
                } else {
                    "xhtml"
                };
                tracing::error!(
                    event = "publication_instrumentation_failed",
                    resource_kind = resource_class
                );
                log_performance(serde_json::json!({
                    "event": "publication_document",
                    "traceId": trace_id.as_ref(),
                    "outcome": "failed",
                    "resourceClass": resource_class,
                    "cache": "miss",
                    "prewarm": false,
                    "totalMs": duration_milliseconds(started.elapsed()),
                }));
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("{} instrumentation failed", resource_class.to_uppercase()),
                )
                    .into_response()
            }
        };
    }
    if let Some(kind) = adapted_image {
        let restated =
            image_representation(session.clone(), path.clone(), rule.size_bytes, kind).await;
        if let Some(bytes) = restated {
            if session.revoked.load(Ordering::Acquire) {
                return not_found().await;
            }
            let mut response = publication_representation(
                method == Method::HEAD,
                "image/png",
                bytes,
                false,
                None,
                &session_publication_csp(profile, &state, &session),
            );
            apply_publication_validator(&mut response, &validator);
            apply_image_negotiation(&mut response);
            if matches!(profile, PublicationResponseProfile::SpineLevel) {
                apply_reader_page_image_cors(&mut response, rule, &headers, &state.asset_origin);
            }
            return response;
        }
    }
    if declares_stylesheet(&rule.mime) {
        let rewritten =
            stylesheet_representation(session.clone(), path.clone(), rule.size_bytes).await;
        if let Some(bytes) = rewritten {
            if session.revoked.load(Ordering::Acquire) {
                return not_found().await;
            }
            let mut response = publication_representation(
                method == Method::HEAD,
                &rule.mime,
                bytes,
                false,
                None,
                &session_publication_csp(profile, &state, &session),
            );
            apply_publication_validator(&mut response, &validator);
            return response;
        }
    }
    let csp = session_publication_csp(profile, &state, &session);
    let sidecar = entry.media_sidecars.get(&path_text).cloned();
    // Narration audio plays in the cross-site reader page; the body stays opaque without CORS.
    let cross_origin_media = sidecar.is_some()
        && headers
            .get("sec-fetch-dest")
            .is_some_and(|dest| dest == "audio" || dest == "video");
    let source = match rule.fallback_source.as_deref() {
        Some(fallback) => {
            EpubPath::new(fallback).expect("a sealed fallback source is a canonical path")
        }
        None => path,
    };
    let mut response = stream_imported_resource(
        entry.epub.clone(),
        source,
        rule,
        method == Method::HEAD,
        single_range_header(&headers),
        ResourceStreamContext {
            csp,
            admission: state.stream_admission.clone(),
            session: Some(session.clone()),
            request_origin: headers.get(ORIGIN).cloned(),
            reader_cover_path: selected_cover_path(&entry.facts),
            reader_cover_origin: Some(state.asset_origin.clone()),
            sidecar,
        },
    )
    .await;
    apply_publication_validator(&mut response, &validator);
    if matches!(profile, PublicationResponseProfile::SpineLevel) {
        apply_reader_page_image_cors(&mut response, rule, &headers, &state.asset_origin);
    }
    if cross_origin_media {
        response.headers_mut().insert(
            "cross-origin-resource-policy",
            HeaderValue::from_static("cross-origin"),
        );
    }
    response
}

async fn measurement_publication_resource(
    State(state): State<MeasurementPublicationState>,
    session: Option<Extension<Arc<ActiveReaderSession>>>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
    }
    if let Some(response) = blocked_publication_worker(&headers) {
        return response;
    }
    let Some(Extension(session)) = session else {
        return not_found().await;
    };
    if session.revoked.load(Ordering::Acquire) {
        return not_found().await;
    }
    let path = match decode_publication_path(&uri) {
        Ok(path) => path,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let Some(rule) = session.entry.responses.resources.get(path.as_str()) else {
        return not_found().await;
    };
    if rule.missing {
        return not_found().await;
    }
    match rule.measurement {
        Measurement::Denied => {
            let mut response = (
                StatusCode::FORBIDDEN,
                "resource is not eligible for measurement",
            )
                .into_response();
            session_headers(response.headers_mut(), Some(&measurement_dependency_csp()));
            response
        }
        Measurement::Unchanged => {
            if let Some(kind) = image_adaptation(&headers, &rule.mime) {
                let restated =
                    image_representation(session.clone(), path.clone(), rule.size_bytes, kind)
                        .await;
                if let Some(bytes) = restated {
                    let mut response = publication_representation(
                        method == Method::HEAD,
                        "image/png",
                        bytes,
                        false,
                        None,
                        &measurement_dependency_csp(),
                    );
                    apply_image_negotiation(&mut response);
                    return response;
                }
            }
            if declares_stylesheet(&rule.mime) {
                let rewritten =
                    stylesheet_representation(session.clone(), path.clone(), rule.size_bytes).await;
                if let Some(bytes) = rewritten {
                    return publication_representation(
                        method == Method::HEAD,
                        &rule.mime,
                        bytes,
                        false,
                        None,
                        &measurement_dependency_csp(),
                    );
                }
            }
            stream_imported_resource(
                session.entry.epub.clone(),
                path,
                rule,
                method == Method::HEAD,
                single_range_header(&headers),
                ResourceStreamContext {
                    csp: measurement_dependency_csp(),
                    admission: state.stream_admission.clone(),
                    session: Some(session.clone()),
                    request_origin: headers.get(ORIGIN).cloned(),
                    reader_cover_path: None,
                    reader_cover_origin: None,
                    sidecar: None,
                },
            )
            .await
        }
        Measurement::InstrumentedXhtml => {
            let started = Instant::now();
            let transformed = transformed_measurement_document(
                session.clone(),
                path,
                state.asset_origin.clone(),
                state.transform_admission.clone(),
            )
            .await;
            if session.revoked.load(Ordering::Acquire) {
                return not_found().await;
            }
            match transformed {
                Ok((bytes, nonce)) => {
                    log_performance(serde_json::json!({
                        "event": "pagination_measurement_document",
                        "outcome": "ready",
                        "resourceClass": "xhtml",
                        "outputBytes": bytes.len(),
                        "totalMs": duration_milliseconds(started.elapsed()),
                    }));
                    publication_representation(
                        method == Method::HEAD,
                        &rule.mime,
                        bytes,
                        false,
                        single_range_header(&headers),
                        &measurement_document_csp(&state.reader_origin, &nonce),
                    )
                }
                Err(_) => {
                    log_performance(serde_json::json!({
                        "event": "pagination_measurement_document",
                        "outcome": "failed",
                        "resourceClass": "xhtml",
                        "totalMs": duration_milliseconds(started.elapsed()),
                    }));
                    (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "measurement XHTML instrumentation failed",
                    )
                        .into_response()
                }
            }
        }
    }
}

pub fn blocked_publication_worker(headers: &HeaderMap) -> Option<Response> {
    if has_exact_header(headers, "service-worker", "script") {
        return Some((StatusCode::FORBIDDEN, "service workers are blocked").into_response());
    }
    // Fetch Metadata distinguishes shared workers where the WebView supplies it. Requests
    // without this browser-owned metadata cannot be classified honestly at the HTTP boundary.
    has_exact_header(headers, "sec-fetch-dest", "sharedworker")
        .then(|| (StatusCode::FORBIDDEN, "shared workers are blocked").into_response())
}
