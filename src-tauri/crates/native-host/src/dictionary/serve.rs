//! Reading routes: lookup, entry, section, and pronunciation audio.

use super::remote_audio::{RemoteFailure, RemoteRecording};
use super::wire::{
    WireLookupMatch, WireSection, WireViewLookup, WireViewOutcome, parse_audio_handle,
    parse_entry_handle, wire_entry, wire_pronunciations,
};
use super::{
    DETAIL_PROJECTION, DictionaryAudioRequest, DictionaryEntryRequest, DictionaryLookupRequest,
    DictionarySectionRequest, DictionarySet, LOOKUP_PROJECTION, MAX_DICTIONARY_REQUEST_BYTES,
    MAX_HANDLE_BYTES, MAX_REQUEST_ID_BYTES, cancel_operation, catalog_listing, install,
    installations, operation, operations, uninstall,
};
use crate::ActiveReaderSession;
use crate::Arc;
use crate::Ordering;
use crate::ReaderState;
use crate::constant_time_equal;
use crate::http::{HostError, TrustedWrite};
use crate::valid_reader_session_id;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::{delete, get, post};
use elephant_ladder_dictionary_pack::{
    AUDIO_MEDIA_TYPE, AudioDelivery, LookupOptions, MAX_LOOKUP_RESULTS, MAX_QUERY_BYTES,
    MAX_SECTION_PAGE_ITEMS, PackError, RecordingStatus, SectionPage, ViewOutcome,
};
use serde::Deserialize;

// Routes.

/// Dictionary catalog, installation, and lookup routes.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new()
        .route("/native/dictionary/catalog", get(catalog_listing))
        .route(
            "/native/dictionary/installations",
            get(installations)
                .post(install)
                .layer(axum::extract::DefaultBodyLimit::max(
                    MAX_DICTIONARY_REQUEST_BYTES,
                )),
        )
        .route(
            "/native/dictionary/installations/{pack_id}/{pack_revision}",
            delete(uninstall),
        )
        .route("/native/dictionary/operations", get(operations))
        .route(
            "/native/dictionary/operations/{operation_id}",
            get(operation).delete(cancel_operation),
        )
        .route(
            "/native/dictionary/lookup",
            post(lookup).layer(axum::extract::DefaultBodyLimit::max(
                MAX_DICTIONARY_REQUEST_BYTES,
            )),
        )
        .route(
            "/native/dictionary/entry",
            post(entry).layer(axum::extract::DefaultBodyLimit::max(
                MAX_DICTIONARY_REQUEST_BYTES,
            )),
        )
        .route(
            "/native/dictionary/section",
            post(section).layer(axum::extract::DefaultBodyLimit::max(
                MAX_DICTIONARY_REQUEST_BYTES,
            )),
        )
        .route(
            "/native/dictionary/audio",
            post(audio).layer(axum::extract::DefaultBodyLimit::max(
                MAX_DICTIONARY_REQUEST_BYTES,
            )),
        )
}

pub(crate) fn dictionary_unavailable(status: StatusCode, reason: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({"status": "unavailable", "reason": reason})),
    )
        .into_response()
}

pub(crate) fn completed(value: serde_json::Value) -> Response {
    match serde_json::to_vec(&value) {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => HostError::Internal("dictionary-encode").into_response(),
    }
}

/// Why a dictionary request was not served. Kept as the reason rather than a built
/// response, so authorization does not decide how a rejection is rendered.
pub(crate) enum LeaseRefused {
    BadRequest,
    Unavailable,
}

impl LeaseRefused {
    pub(crate) fn into_response(self) -> Response {
        match self {
            Self::BadRequest => HostError::BadRequest("invalid-dictionary-request").into_response(),
            Self::Unavailable => {
                dictionary_unavailable(StatusCode::CONFLICT, "snapshot-unavailable")
            }
        }
    }
}

/// Authorizes a dictionary request and returns the session's dictionary lease.
pub(crate) fn leased_set(
    state: &ReaderState,
    reader_session_id: &str,
    revision: &str,
) -> Result<(Arc<ActiveReaderSession>, Arc<DictionarySet>), LeaseRefused> {
    if !valid_reader_session_id(reader_session_id) {
        return Err(LeaseRefused::BadRequest);
    }
    let session = state.lifecycle.load_active().filter(|session| {
        !session.revoked.load(Ordering::Acquire)
            && constant_time_equal(
                session.reader_session_id.as_bytes(),
                reader_session_id.as_bytes(),
            )
    });
    let Some(session) = session else {
        return Err(LeaseRefused::Unavailable);
    };
    let set = session.dictionary.clone();
    if set.views.is_empty() || set.revision != revision {
        return Err(LeaseRefused::Unavailable);
    }
    Ok((session, set))
}

pub(crate) fn still_active(session: &ActiveReaderSession) -> bool {
    !session.revoked.load(Ordering::Acquire)
}

pub(crate) fn valid_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

pub(crate) fn valid_language(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part.len() <= 8
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

pub(crate) fn is_edge_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
        || matches!(
            character,
            '«' | '»'
                | '‹'
                | '›'
                | '“'
                | '”'
                | '„'
                | '‟'
                | '‘'
                | '’'
                | '‚'
                | '‛'
                | '…'
                | '–'
                | '—'
                | '¡'
                | '¿'
                | '·'
                | '•'
                | '、'
                | '。'
                | '，'
                | '．'
                | '！'
                | '？'
                | '：'
                | '；'
                | '（'
                | '）'
                | '「'
                | '」'
                | '『'
                | '』'
                | '【'
                | '】'
                | '《'
                | '》'
                | '〈'
                | '〉'
                | '〔'
                | '〕'
                | '・'
        )
}

pub(crate) async fn lookup(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<DictionaryLookupRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    let text = request.text.trim().to_owned();
    if !valid_text(&request.request_id, MAX_REQUEST_ID_BYTES)
        || text.is_empty()
        || text.chars().any(char::is_control)
        || request
            .language
            .as_deref()
            .is_some_and(|language| !valid_language(language))
    {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    let (session, set) = match leased_set(&state, &request.reader_session_id, &request.revision) {
        Ok(leased) => leased,
        Err(refused) => return refused.into_response(),
    };
    let language = request.language.clone();
    let looked_up = tokio::task::spawn_blocking(move || {
        let options = LookupOptions {
            limit: MAX_LOOKUP_RESULTS,
            projection: LOOKUP_PROJECTION,
        };
        let mut query = text.clone();
        if query.len() > MAX_QUERY_BYTES {
            let views = set
                .views
                .iter()
                .map(|view| WireViewLookup {
                    view_id: view.view_id.clone(),
                    outcome: WireViewOutcome::NoEntry,
                })
                .collect::<Vec<_>>();
            return serde_json::to_value(views).map(|views| (query, set.revision.clone(), views));
        }
        let mut outcomes = set.snapshot.lookup(&query, language.as_deref(), options);
        let stripped = text.trim_matches(is_edge_punctuation).trim();
        if !stripped.is_empty()
            && stripped != text
            && outcomes
                .iter()
                .all(|outcome| matches!(outcome.outcome, ViewOutcome::NoEntry))
        {
            query = stripped.to_owned();
            outcomes = set.snapshot.lookup(&query, language.as_deref(), options);
        }
        let views = outcomes
            .into_iter()
            .map(|lookup| {
                let view_index = set.view_index(lookup.view_id).unwrap_or_default();
                let matches = |outcome: elephant_ladder_dictionary_pack::LookupOutcome| {
                    outcome
                        .matches
                        .into_iter()
                        .map(|matched| WireLookupMatch {
                            match_class: matched.match_class,
                            entry: wire_entry(&set, view_index, matched.entry),
                        })
                        .collect::<Vec<_>>()
                };
                let outcome = match lookup.outcome {
                    ViewOutcome::Completed(outcome) => WireViewOutcome::Completed {
                        truncated: outcome.truncated,
                        entries: matches(outcome),
                    },
                    ViewOutcome::NoTranslation(outcome) => WireViewOutcome::NoTranslation {
                        entries: matches(outcome),
                    },
                    ViewOutcome::NoEntry => WireViewOutcome::NoEntry,
                    ViewOutcome::Failed(error) => {
                        tracing::warn!(
                            event = "dictionary.lookup.view_failed",
                            view = lookup.view_id,
                            error = %error
                        );
                        WireViewOutcome::Failed
                    }
                };
                WireViewLookup {
                    view_id: lookup.view_id.to_owned(),
                    outcome,
                }
            })
            .collect::<Vec<_>>();
        serde_json::to_value(views).map(|views| (query, set.revision.clone(), views))
    })
    .await;
    if !still_active(&session) {
        return dictionary_unavailable(StatusCode::CONFLICT, "snapshot-unavailable");
    }
    match looked_up {
        Ok(Ok((query, revision, views))) => completed(serde_json::json!({
            "status": "completed",
            "requestId": request.request_id,
            "revision": revision,
            "query": query,
            "views": views,
        })),
        _ => HostError::Internal("dictionary-lookup").into_response(),
    }
}

pub(crate) async fn entry(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<DictionaryEntryRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    if !valid_text(&request.entry, MAX_HANDLE_BYTES) {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    let (session, set) = match leased_set(&state, &request.reader_session_id, &request.revision) {
        Ok(leased) => leased,
        Err(refused) => return refused.into_response(),
    };
    let read = tokio::task::spawn_blocking(move || {
        let (view_index, reference) =
            parse_entry_handle(&set, &request.entry).ok_or(PackError::UnknownEntry)?;
        let view = &set.snapshot.views()[view_index];
        let entry = view.entry(&reference, DETAIL_PROJECTION)?;
        Ok::<_, PackError>(serde_json::to_value(wire_entry(&set, view_index, entry)))
    })
    .await;
    if !still_active(&session) {
        return dictionary_unavailable(StatusCode::CONFLICT, "snapshot-unavailable");
    }
    match read {
        Ok(Ok(Ok(entry))) => completed(serde_json::json!({"status": "completed", "entry": entry})),
        Ok(Err(PackError::UnknownEntry)) => {
            dictionary_unavailable(StatusCode::NOT_FOUND, "entry-unavailable")
        }
        _ => HostError::Internal("dictionary-entry").into_response(),
    }
}

pub(crate) async fn section(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<DictionarySectionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    if !valid_text(&request.entry, MAX_HANDLE_BYTES)
        || request.limit == 0
        || request.limit > MAX_SECTION_PAGE_ITEMS
    {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    let (session, set) = match leased_set(&state, &request.reader_session_id, &request.revision) {
        Ok(leased) => leased,
        Err(refused) => return refused.into_response(),
    };
    let read = tokio::task::spawn_blocking(move || {
        let (view_index, reference) =
            parse_entry_handle(&set, &request.entry).ok_or(PackError::UnknownEntry)?;
        let view = &set.snapshot.views()[view_index];
        let page = view.section_page(
            &reference,
            request.section.section(),
            request.offset,
            request.limit,
            DETAIL_PROJECTION,
        )?;
        let language = view.pack().corpus_language().to_owned();
        let wire = match page {
            SectionPage::Etymology(page) => WireSection::Etymology(page),
            SectionPage::Pronunciations(page) => {
                WireSection::Pronunciations(wire_pronunciations(&set, &language, page))
            }
            SectionPage::Hyphenations(page) => WireSection::Hyphenations(page),
            SectionPage::Forms(page) => WireSection::Forms(page),
            SectionPage::Senses(page) => WireSection::Senses(page),
            SectionPage::Translations(page) => WireSection::Translations(page),
            SectionPage::Relations(page) => WireSection::Relations(page),
            SectionPage::Descendants(page) => WireSection::Descendants(page),
            SectionPage::Notes(page) => WireSection::Notes(page),
            SectionPage::Categories(page) => WireSection::Categories(page),
            SectionPage::Examples(page) => WireSection::Examples(page),
            SectionPage::NoSuchSense => WireSection::NoSuchSense,
        };
        Ok::<_, PackError>(serde_json::to_value(wire))
    })
    .await;
    if !still_active(&session) {
        return dictionary_unavailable(StatusCode::CONFLICT, "snapshot-unavailable");
    }
    match read {
        Ok(Ok(Ok(section))) => {
            completed(serde_json::json!({"status": "completed", "section": section}))
        }
        Ok(Err(PackError::UnknownEntry)) => {
            dictionary_unavailable(StatusCode::NOT_FOUND, "entry-unavailable")
        }
        _ => HostError::Internal("dictionary-section-read").into_response(),
    }
}

pub(crate) async fn audio(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<DictionaryAudioRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    };
    if !valid_text(&request.audio, MAX_HANDLE_BYTES) {
        return HostError::BadRequest("invalid-dictionary-request").into_response();
    }
    let (session, set) = match leased_set(&state, &request.reader_session_id, &request.revision) {
        Ok(leased) => leased,
        Err(refused) => return refused.into_response(),
    };
    let read = tokio::task::spawn_blocking(move || {
        let (language, file_name) =
            parse_audio_handle(&request.audio).ok_or(PackError::UnknownEntry)?;
        let collection = set.audio.get(&language).ok_or(PackError::UnknownEntry)?;
        if collection.metadata().delivery == AudioDelivery::Remote {
            return collection
                .recording(&file_name)?
                .filter(|recording| recording.facts.status == RecordingStatus::Available)
                .and_then(|recording| RemoteRecording::from_facts(&recording.facts))
                .map(AudioSource::Remote)
                .ok_or(PackError::UnknownEntry);
        }
        collection
            .read(&file_name)?
            .map(AudioSource::Bundled)
            .ok_or(PackError::UnknownEntry)
    })
    .await;
    let (bytes, media_type) = match read {
        Ok(Ok(AudioSource::Bundled(bytes))) => (bytes, AUDIO_MEDIA_TYPE),
        Ok(Ok(AudioSource::Remote(recording))) => match recording.fetch().await {
            Ok(bytes) => (bytes, recording.media_type()),
            Err(RemoteFailure::Unreachable) => {
                tracing::warn!(event = "dictionary.remote_audio_unreachable");
                return HostError::Upstream("pronunciation-unreachable").into_response();
            }
            Err(RemoteFailure::Rejected) => {
                tracing::warn!(event = "dictionary.remote_audio_rejected");
                return HostError::Upstream("pronunciation-unverified").into_response();
            }
        },
        Ok(Err(PackError::UnknownEntry)) => {
            return dictionary_unavailable(StatusCode::NOT_FOUND, "audio-unavailable");
        }
        _ => {
            return HostError::Internal("dictionary-audio-read").into_response();
        }
    };
    if !still_active(&session) {
        return dictionary_unavailable(StatusCode::CONFLICT, "snapshot-unavailable");
    }
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, media_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) enum AudioSource {
    Bundled(Vec<u8>),
    Remote(RemoteRecording),
}

// Management routes.

/// What an install is asked for: one language, and where its recordings play from.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DictionaryInstallRequest {
    pub(crate) corpus_language: String,
    pub(crate) audio: String,
}
