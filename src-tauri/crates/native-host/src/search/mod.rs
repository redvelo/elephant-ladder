//! Full-text search over one publication snapshot.
//!
//! Search is an optional, independently revisioned snapshot product derived
//! during import. Missing, incompatible, or malformed search data never blocks
//! opening a publication: queries run on a dedicated read-only connection and
//! return bounded batches through Navigator's cancellable provider contract.

use crate::http::HostError;
use crate::{
    MAX_ANALYZED_RESOURCES, MAX_RESOURCE_ANALYSIS_BYTES, MAX_SEARCH_CONTINUATION_BYTES,
    MAX_SEARCH_DOCUMENTS, MAX_SEARCH_ENTRIES, MAX_SEARCH_ENTRIES_BYTES,
    MAX_SEARCH_ENTRY_TEXT_BYTES, MAX_SEARCH_EXCERPT_BYTES, MAX_SEARCH_EXCERPT_CODE_POINTS,
    MAX_SEARCH_ID_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_QUERY_CODE_POINTS,
    MAX_SEARCH_REQUEST_BYTES, MAX_SEARCH_RESULT_ROWS, MAX_SEARCH_RESULTS_PER_BATCH,
    MAX_SEARCH_SNAPSHOT_BYTES, MAX_SEARCH_STRING_BYTES, MAX_SMIL_NESTING, MAX_SMIL_NODES,
    MAX_TOTAL_ANALYSIS_BYTES, ReaderState, SEARCH_REVISION, TrustedWrite, constant_time_equal,
    duration_milliseconds, log_performance, reading_order_local_resources, search_index,
    valid_reader_session_id,
};
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::State, http::StatusCode,
    response::IntoResponse, response::Response, routing::post,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use epub_navigator::{
    kernel::DocumentAdapter, kernel::OccurrenceDocumentTarget, kernel::ReadingOrderOccurrence,
    kernel::project_reading_order,
};
use epub_stack::PublicationAnalysis;
use epub_stack::analysis::{AnalysisLimits, AnalysisOutcome};
use epub_stack::{EpubPath, EpubZip};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path as FsPath;
use std::{
    collections::BTreeMap, collections::HashMap, collections::HashSet, path::PathBuf,
    sync::atomic::Ordering, time::Instant,
};

/// Search routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    Router::new().route(
        "/native/search",
        post(search_publication).layer(DefaultBodyLimit::max(MAX_SEARCH_REQUEST_BYTES)),
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchSnapshot {
    pub(crate) entries: Vec<SearchEntry>,
    pub(crate) documents: BTreeMap<String, SearchDocumentState>,
    pub(crate) effective_reading_order_resources: Vec<Option<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchEntry {
    pub(crate) id: String,
    pub(crate) resource_path: String,
    pub(crate) fragment: Option<String>,
    pub(crate) text: String,
    pub(crate) reading_order_occurrence_indexes: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchDocumentState {
    Complete,
    Partial,
    Unavailable,
    Inapplicable,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeSearchRequest {
    reader_session_id: String,
    request: SearchRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchRequest {
    request_id: String,
    query: String,
    scope: SearchScope,
    initiator: SearchInitiator,
    continuation: Option<String>,
    limit: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum SearchScope {
    Publication,
    Occurrence { occurrence: u32, resource: String },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SearchInitiator {
    QueryEntry,
    Selection,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchContinuation {
    offset: u32,
    binding: String,
}

struct SearchLifecycleTrace {
    request_key: String,
    started: Instant,
    phase: &'static str,
    finished: bool,
}

async fn search_publication(
    State(state): State<ReaderState>,
    _: TrustedWrite,
    body: Result<Json<NativeSearchRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return HostError::BadRequest("invalid-search-request").into_response();
    };
    if !valid_reader_session_id(&request.reader_session_id)
        || validate_search_request(&request.request).is_err()
    {
        return HostError::BadRequest("invalid-search-request").into_response();
    }
    let session = state.lifecycle.load_active();
    let Some(session) = session.filter(|session| {
        !session.revoked.load(Ordering::Acquire)
            && constant_time_equal(
                session.reader_session_id.as_bytes(),
                request.reader_session_id.as_bytes(),
            )
    }) else {
        return search_unavailable(StatusCode::CONFLICT, "snapshot-unavailable", None);
    };

    let expression = match compile_search_expression(&request.request.query) {
        Ok(expression) => expression,
        Err(()) => return HostError::BadRequest("invalid-search-query").into_response(),
    };
    let binding = search_request_binding(&request.reader_session_id, &request.request);
    let offset = match request.request.continuation.as_deref() {
        Some(continuation) => match decode_search_continuation(continuation, &binding) {
            Some(offset) => offset,
            None => return HostError::BadRequest("invalid-search-continuation").into_response(),
        },
        None => 0,
    };
    let (occurrence, resource) = match &request.request.scope {
        SearchScope::Publication => (None, None),
        SearchScope::Occurrence {
            occurrence,
            resource,
        } => (Some(*occurrence), Some(resource.clone())),
    };
    if let (Some(occurrence), Some(resource)) = (occurrence, resource.as_deref())
        && reading_order_local_resources(&session.entry.reading_order)
            .get(occurrence as usize)
            .and_then(Option::as_deref)
            != Some(resource)
    {
        return search_unavailable(StatusCode::OK, "scope-unavailable", None);
    }

    let mut trace = SearchLifecycleTrace::start(&request.request);
    let search_index = state.search_index.clone();
    let snapshot_id = session.entry.snapshot_id.to_string();
    let limit = request.request.limit;
    let searched = tokio::task::spawn_blocking(move || {
        search_index.search(search_index::SearchStorageRequest {
            snapshot_id: &snapshot_id,
            expected_revision: SEARCH_REVISION,
            expression: &expression,
            occurrence,
            resource_path: resource.as_deref(),
            offset,
            limit,
        })
    })
    .await;
    trace.worker_returned();
    if session.revoked.load(Ordering::Acquire) {
        trace.finish("snapshot-unavailable");
        return search_unavailable(StatusCode::CONFLICT, "snapshot-unavailable", None);
    }

    match searched {
        Ok(Ok(search_index::SearchStorageOutcome::IndexUnavailable)) => {
            let pending = state
                .search_indexing
                .lock()
                .expect("search indexing lock poisoned")
                .contains(session.entry.snapshot_id.as_ref());
            if pending {
                trace.finish("index-pending");
                search_unavailable(
                    StatusCode::OK,
                    "index-pending",
                    Some("Search is still being indexed for this publication."),
                )
            } else {
                trace.finish("index-unavailable");
                search_unavailable(StatusCode::OK, "index-unavailable", None)
            }
        }
        Ok(Ok(search_index::SearchStorageOutcome::ScopeUnavailable)) => {
            trace.finish("scope-unavailable");
            search_unavailable(StatusCode::OK, "scope-unavailable", None)
        }
        Ok(Ok(search_index::SearchStorageOutcome::Ready(ready))) => {
            if let Some(diagnostics) = ready.diagnostics {
                tracing::info!(
                    event = "publication_search_empty",
                    corpus_entries = diagnostics.corpus_entries,
                    linked_entries = diagnostics.linked_entries,
                    indexed_entries = diagnostics.indexed_entries,
                    matching_entries = diagnostics.matching_entries
                );
                if diagnostics.corpus_entries > 0 && diagnostics.linked_entries == 0 {
                    trace.finish("index-unavailable");
                    return search_unavailable(
                        StatusCode::OK,
                        "index-unavailable",
                        Some("Searchable text was not associated with the reading order."),
                    );
                }
                if diagnostics.indexed_entries != diagnostics.corpus_entries {
                    trace.finish("index-unavailable");
                    return search_unavailable(
                        StatusCode::OK,
                        "index-unavailable",
                        Some("The search text index is incomplete."),
                    );
                }
            }
            let mut results = Vec::with_capacity(ready.rows.len());
            for row in ready.rows {
                let Some(excerpt) = search_excerpt(&row.snippet) else {
                    tracing::error!(
                        event = "publication_search_invalid_result",
                        error_kind = "snippet"
                    );
                    trace.finish("failed");
                    return HostError::Internal("search").into_response();
                };
                let Some(source) = reading_order_source(
                    &session.entry.facts,
                    &session.entry.reading_order,
                    row.occurrence,
                ) else {
                    tracing::error!(
                        event = "publication_search_invalid_result",
                        error_kind = "occurrence"
                    );
                    trace.finish("failed");
                    return HostError::Internal("search").into_response();
                };
                let exact = serde_json::json!({
                    "type": "FragmentSelector",
                    "value": search_text_fragment(&excerpt),
                    "conformsTo": "https://wicg.github.io/scroll-to-text-fragment/"
                });
                let plain = serde_json::json!({
                    "type": "FragmentSelector",
                    "value": search_text_fragment(&(String::new(), excerpt.1.clone(), String::new())),
                    "conformsTo": "https://wicg.github.io/scroll-to-text-fragment/"
                });
                let alternatives = if exact == plain {
                    vec![exact]
                } else {
                    vec![exact, plain]
                };
                let selector = alternatives
                    .into_iter()
                    .map(|exact| match &row.fragment {
                        Some(fragment) => serde_json::json!({
                            "type": "FragmentSelector",
                            "value": fragment,
                            "refinedBy": exact
                        }),
                        None => exact,
                    })
                    .collect::<Vec<_>>();
                let target_json = match serde_json::to_string(&serde_json::json!({
                    "source": source,
                    "selector": selector
                })) {
                    Ok(target) => target,
                    Err(_) => {
                        trace.finish("failed");
                        return HostError::Internal("search").into_response();
                    }
                };
                let result_id = format!(
                    "{}:{}",
                    row.occurrence,
                    blake3::hash(row.entry_id.as_bytes()).to_hex()
                );
                results.push(serde_json::json!({
                    "id": result_id,
                    "section": {
                        "id": format!("occurrence-{}", row.occurrence),
                        "occurrence": row.occurrence,
                        "resource": row.resource_path,
                        "label": Value::Null
                    },
                    "targetJson": target_json,
                    "excerpt": {
                        "before": excerpt.0,
                        "match": excerpt.1,
                        "after": excerpt.2
                    }
                }));
            }
            let total_occurrences = match &request.request.scope {
                SearchScope::Publication => {
                    reading_order_local_resources(&session.entry.reading_order)
                        .len()
                        .try_into()
                        .unwrap_or(u32::MAX)
                }
                SearchScope::Occurrence { .. } => 1,
            };
            let searched_occurrences = ready.coverage.searched_occurrences.min(total_occurrences);
            let coverage = if searched_occurrences == total_occurrences {
                serde_json::json!({
                    "state": "complete",
                    "searchedOccurrences": searched_occurrences,
                    "totalOccurrences": total_occurrences
                })
            } else {
                serde_json::json!({
                    "state": "partial",
                    "searchedOccurrences": searched_occurrences,
                    "totalOccurrences": total_occurrences,
                    "reason": "Some reading-order documents were not completely analyzed."
                })
            };
            let continuation = ready.has_more.then(|| {
                encode_search_continuation(SearchContinuation {
                    offset: offset.saturating_add(limit),
                    binding,
                })
            });
            if session.revoked.load(Ordering::Acquire) {
                trace.finish("snapshot-unavailable");
                return search_unavailable(StatusCode::CONFLICT, "snapshot-unavailable", None);
            }
            trace.finish("completed");
            Json(serde_json::json!({
                "status": "completed",
                "batch": {
                    "requestId": request.request.request_id,
                    "results": results,
                    "coverage": coverage,
                    "continuation": continuation,
                    "totalResultCount": Value::Null
                }
            }))
            .into_response()
        }
        Ok(Err(_)) => {
            tracing::error!(event = "publication_search_failed", error_kind = "storage");
            trace.finish("failed");
            HostError::Internal("search").into_response()
        }
        Err(_) => {
            tracing::error!(event = "publication_search_failed", error_kind = "worker");
            trace.finish("failed");
            HostError::Internal("search").into_response()
        }
    }
}

fn validate_search_request(request: &SearchRequest) -> Result<(), ()> {
    if request.request_id.is_empty()
        || request.request_id.len() > MAX_SEARCH_ID_BYTES
        || request.query.is_empty()
        || request.query.len() > MAX_SEARCH_QUERY_BYTES
        || request.query.chars().count() > MAX_SEARCH_QUERY_CODE_POINTS
        || request.query.chars().any(char::is_control)
        || request
            .continuation
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_SEARCH_CONTINUATION_BYTES)
        || request.limit == 0
        || request.limit > MAX_SEARCH_RESULTS_PER_BATCH
    {
        return Err(());
    }
    if let SearchScope::Occurrence { resource, .. } = &request.scope
        && (resource.is_empty()
            || resource.len() > MAX_SEARCH_STRING_BYTES
            || EpubPath::new(resource).is_err())
    {
        return Err(());
    }
    Ok(())
}

pub(crate) fn compile_search_expression(query: &str) -> Result<String, ()> {
    let terms = query.split_whitespace().collect::<Vec<_>>();
    if terms.is_empty() {
        return Err(());
    }
    Ok(terms
        .iter()
        .enumerate()
        .map(|(index, term)| {
            let escaped = term.replace('"', "\"\"");
            if index + 1 == terms.len() {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" AND "))
}

fn search_request_binding(reader_session_id: &str, request: &SearchRequest) -> String {
    let mut hasher = blake3::Hasher::new();
    for value in [
        reader_session_id,
        request.request_id.as_str(),
        request.query.as_str(),
        match request.initiator {
            SearchInitiator::QueryEntry => "query-entry",
            SearchInitiator::Selection => "selection",
        },
    ] {
        hasher.update(value.as_bytes());
        hasher.update(&[0]);
    }
    match &request.scope {
        SearchScope::Publication => {
            hasher.update(b"publication");
        }
        SearchScope::Occurrence {
            occurrence,
            resource,
        } => {
            hasher.update(b"occurrence");
            hasher.update(&occurrence.to_le_bytes());
            hasher.update(resource.as_bytes());
        }
    };
    hasher.update(&request.limit.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn encode_search_continuation(continuation: SearchContinuation) -> String {
    BASE64_STANDARD
        .encode(serde_json::to_vec(&continuation).expect("search continuation must serialize"))
}

fn decode_search_continuation(value: &str, expected_binding: &str) -> Option<u32> {
    let bytes = BASE64_STANDARD.decode(value).ok()?;
    let continuation: SearchContinuation = serde_json::from_slice(&bytes).ok()?;
    (continuation.offset <= 1_000_000
        && constant_time_equal(continuation.binding.as_bytes(), expected_binding.as_bytes()))
    .then_some(continuation.offset)
}

fn search_excerpt(snippet: &str) -> Option<(String, String, String)> {
    let start = snippet.find(search_index::SEARCH_HIGHLIGHT_START)?;
    let highlighted = &snippet[start + search_index::SEARCH_HIGHLIGHT_START.len()..];
    let end = highlighted.find(search_index::SEARCH_HIGHLIGHT_END)?;
    let clean = |value: &str| {
        value
            .replace(search_index::SEARCH_HIGHLIGHT_START, "")
            .replace(search_index::SEARCH_HIGHLIGHT_END, "")
    };
    let mut before = clean(&snippet[..start])
        .chars()
        .rev()
        .take(128)
        .collect::<Vec<_>>();
    before.reverse();
    let mut matched = clean(&highlighted[..end])
        .chars()
        .take(256)
        .collect::<Vec<_>>();
    let mut after = clean(&highlighted[end + search_index::SEARCH_HIGHLIGHT_END.len()..])
        .chars()
        .take(128)
        .collect::<Vec<_>>();
    if matched.is_empty() {
        return None;
    }
    while before.len() + matched.len() + after.len() > MAX_SEARCH_EXCERPT_CODE_POINTS
        || before
            .iter()
            .chain(&matched)
            .chain(&after)
            .collect::<String>()
            .len()
            > MAX_SEARCH_EXCERPT_BYTES
    {
        if before.len() >= after.len() && !before.is_empty() {
            before.remove(0);
        } else if !after.is_empty() {
            after.pop();
        } else if matched.len() > 1 {
            matched.pop();
        } else {
            return None;
        }
    }
    Some((
        before.into_iter().collect(),
        matched.into_iter().collect(),
        after.into_iter().collect(),
    ))
}

pub(crate) fn search_text_fragment(excerpt: &(String, String, String)) -> String {
    const MAX_CONTEXT_CODE_POINTS: usize = 32;

    let before = excerpt.0.strip_prefix("...").unwrap_or(&excerpt.0);
    let mut prefix = before
        .chars()
        .rev()
        .take(MAX_CONTEXT_CODE_POINTS)
        .collect::<Vec<_>>();
    prefix.reverse();
    let prefix = prefix.into_iter().collect::<String>();
    let after = excerpt.2.strip_suffix("...").unwrap_or(&excerpt.2);
    let suffix = after
        .chars()
        .take(MAX_CONTEXT_CODE_POINTS)
        .collect::<String>();
    let exact = utf8_percent_encode(&excerpt.1, NON_ALPHANUMERIC);

    match (prefix.is_empty(), suffix.is_empty()) {
        (false, false) => format!(
            "text={}-,{},-{}",
            utf8_percent_encode(&prefix, NON_ALPHANUMERIC),
            exact,
            utf8_percent_encode(&suffix, NON_ALPHANUMERIC)
        ),
        (false, true) => format!(
            "text={}-,{}",
            utf8_percent_encode(&prefix, NON_ALPHANUMERIC),
            exact
        ),
        (true, false) => format!(
            "text={},-{}",
            exact,
            utf8_percent_encode(&suffix, NON_ALPHANUMERIC)
        ),
        (true, true) => format!("text={exact}"),
    }
}

pub(crate) fn reading_order_source(
    facts: &Value,
    reading_order: &[ReadingOrderOccurrence],
    occurrence: u32,
) -> Option<String> {
    let occurrence = reading_order.get(occurrence as usize)?;
    let declaration = usize::try_from(occurrence.selected_declaration?).ok()?;
    facts
        .get("package")?
        .get("manifest")?
        .get("items")?
        .as_array()?
        .get(declaration)?
        .get("href")?
        .as_str()
        .map(str::to_owned)
}

fn search_unavailable(status: StatusCode, reason: &'static str, message: Option<&str>) -> Response {
    (
        status,
        Json(serde_json::json!({
            "status": "unavailable",
            "reason": reason,
            "message": message
        })),
    )
        .into_response()
}

pub(crate) fn derive_search_index(path: &FsPath) -> Result<SearchSnapshot, String> {
    let zip =
        EpubZip::open(path).map_err(|error| format!("could not open imported EPUB: {error}"))?;
    let epub = zip
        .default_rendition()
        .map_err(|error| format!("could not open imported rendition: {error}"))?;
    let facts = epub_navigator::facts::PublicationFacts::of(&epub)
        .map_err(|error| format!("could not create imported facts: {error}"))?;
    let reading_order = project_reading_order(&facts)
        .map_err(|error| format!("could not project imported reading order: {error}"))?;
    let analysis = epub.analyze_with_limits(search_analysis_limits());
    let search = derive_search_snapshot(&analysis, &reading_order)?;
    validate_search_product(
        &facts,
        &search,
        &reading_order_local_resources(&reading_order),
    )?;
    Ok(search)
}

/// Builds the search index off the import path; the reader answers `index-pending` meanwhile.
pub(crate) fn ensure_search_index(state: &ReaderState, snapshot_id: String, blob_path: PathBuf) {
    if !state
        .search_indexing
        .lock()
        .expect("search indexing lock poisoned")
        .insert(snapshot_id.clone())
    {
        return;
    }
    let storage = state.storage.clone();
    let search_index = state.search_index.clone();
    let indexing = state.search_indexing.clone();
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let mut derive_ms = 0;
        let mut commit_ms = 0;
        let outcome = (|| -> Result<&'static str, String> {
            if search_index.exists(&snapshot_id, SEARCH_REVISION)? {
                return Ok("present");
            }
            let derive_started = Instant::now();
            let search = derive_search_index(&blob_path)?;
            derive_ms = duration_milliseconds(derive_started.elapsed());
            let live = {
                let storage = storage.locked()?;
                if !storage.snapshot_is_current(&snapshot_id)? {
                    return Ok("stale");
                }
                storage.live_snapshot_ids()?
            };
            let commit_started = Instant::now();
            search_index.write(&snapshot_id, SEARCH_REVISION, &search)?;
            commit_ms = duration_milliseconds(commit_started.elapsed());
            search_index.retain(&live)?;
            Ok("indexed")
        })();
        drop(storage);
        drop(search_index);
        indexing
            .lock()
            .expect("search indexing lock poisoned")
            .remove(&snapshot_id);
        match outcome {
            Ok(outcome) => log_performance(serde_json::json!({
                "event": "publication_search_indexed",
                "outcome": outcome,
                "totalMs": duration_milliseconds(started.elapsed()),
                "deriveMs": derive_ms,
                "commitMs": commit_ms,
            })),
            Err(error) => tracing::warn!(
                event = "publication_search_indexing_failed",
                error = %error
            ),
        }
    });
}

pub(crate) fn validate_search_product(
    facts: &epub_navigator::facts::PublicationFacts<'_>,
    search: &SearchSnapshot,
    effective_reading_order_resources: &[Option<String>],
) -> Result<(), String> {
    validate_search_snapshot(search)?;
    let local_paths = facts
        .resources
        .resources()
        .filter_map(|resource| resource.local_path().map(EpubPath::as_str))
        .collect::<HashSet<_>>();
    if search.documents.len() != local_paths.len()
        || search
            .documents
            .keys()
            .any(|path| !local_paths.contains(path.as_str()))
    {
        return Err("search snapshot does not cover the local resource inventory".to_owned());
    }
    if search.effective_reading_order_resources != effective_reading_order_resources {
        return Err("search snapshot does not match the effective reading order".to_owned());
    }
    Ok(())
}

pub(crate) fn search_occurrence_index(
    reading_order: &[ReadingOrderOccurrence],
) -> HashMap<&str, Vec<usize>> {
    let mut index = HashMap::new();
    for occurrence in reading_order {
        if let OccurrenceDocumentTarget::Loadable {
            resource,
            adapter: DocumentAdapter::ReflowableXhtml | DocumentAdapter::FixedXhtml,
        } = &occurrence.document
        {
            index
                .entry(resource.as_str())
                .or_insert_with(Vec::new)
                .push(occurrence.ordinal as usize);
        }
    }
    index
}

pub(crate) fn validate_search_snapshot(snapshot: &SearchSnapshot) -> Result<(), String> {
    if snapshot.entries.len() > MAX_SEARCH_ENTRIES
        || snapshot.documents.len() > MAX_SEARCH_DOCUMENTS
        || snapshot.effective_reading_order_resources.len() > MAX_SEARCH_DOCUMENTS
    {
        return Err("search snapshot exceeds its item bounds".to_owned());
    }
    let entries_json = serde_json::to_vec(&snapshot.entries)
        .map_err(|error| format!("could not validate search entries: {error}"))?;
    if entries_json.len() > MAX_SEARCH_ENTRIES_BYTES {
        return Err("search entries exceed the 16 MiB storage bound".to_owned());
    }
    let snapshot_json = serde_json::to_vec(snapshot)
        .map_err(|error| format!("could not validate search snapshot: {error}"))?;
    if snapshot_json.len() > MAX_SEARCH_SNAPSHOT_BYTES {
        return Err("search snapshot exceeds the 24 MiB storage bound".to_owned());
    }

    for path in snapshot.documents.keys() {
        if path.is_empty() || path.len() > MAX_SEARCH_STRING_BYTES || EpubPath::new(path).is_err() {
            return Err("search snapshot contains an invalid document path".to_owned());
        }
    }
    for path in snapshot.effective_reading_order_resources.iter().flatten() {
        if path.is_empty()
            || path.len() > MAX_SEARCH_STRING_BYTES
            || EpubPath::new(path).is_err()
            || !snapshot.documents.contains_key(path)
        {
            return Err(
                "search snapshot contains an invalid effective reading-order resource".to_owned(),
            );
        }
    }
    let mut ids = HashSet::with_capacity(snapshot.entries.len());
    let mut result_rows = 0usize;
    for entry in &snapshot.entries {
        if entry.id.is_empty()
            || entry.id.len() > MAX_SEARCH_STRING_BYTES
            || entry.resource_path.is_empty()
            || entry.resource_path.len() > MAX_SEARCH_STRING_BYTES
            || EpubPath::new(&entry.resource_path).is_err()
            || entry
                .fragment
                .as_ref()
                .is_some_and(|fragment| fragment.len() > MAX_SEARCH_STRING_BYTES)
            || entry.text.is_empty()
            || entry.text.len() > MAX_SEARCH_ENTRY_TEXT_BYTES
            || !ids.insert(entry.id.as_str())
            || !matches!(
                snapshot.documents.get(&entry.resource_path),
                Some(SearchDocumentState::Complete | SearchDocumentState::Partial)
            )
        {
            return Err("search snapshot contains an invalid entry".to_owned());
        }
        if entry
            .reading_order_occurrence_indexes
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err("search entry occurrence indexes are not strictly increasing".to_owned());
        }
        if entry
            .reading_order_occurrence_indexes
            .iter()
            .any(|occurrence| {
                snapshot
                    .effective_reading_order_resources
                    .get(*occurrence)
                    .and_then(Option::as_deref)
                    != Some(entry.resource_path.as_str())
            })
        {
            return Err("search entry occurrence has a different effective resource".to_owned());
        }
        result_rows = result_rows
            .checked_add(entry.reading_order_occurrence_indexes.len())
            .filter(|rows| *rows <= MAX_SEARCH_RESULT_ROWS)
            .ok_or_else(|| "search snapshot exceeds its result row bound".to_owned())?;
    }
    Ok(())
}

impl SearchLifecycleTrace {
    fn start(request: &SearchRequest) -> Self {
        let request_key = blake3::hash(request.request_id.as_bytes())
            .to_hex()
            .to_string();
        tracing::info!(
            event = "publication_search_started",
            request_key,
            scope = match request.scope {
                SearchScope::Publication => "publication",
                SearchScope::Occurrence { .. } => "occurrence",
            },
            continuation = request.continuation.is_some()
        );
        Self {
            request_key,
            started: Instant::now(),
            phase: "awaiting-worker",
            finished: false,
        }
    }

    fn worker_returned(&mut self) {
        self.phase = "building-response";
        tracing::info!(
            event = "publication_search_worker_returned",
            request_key = self.request_key,
            elapsed_ms = duration_milliseconds(self.started.elapsed())
        );
    }

    fn finish(&mut self, outcome: &'static str) {
        self.finished = true;
        tracing::info!(
            event = "publication_search_finished",
            request_key = self.request_key,
            outcome,
            elapsed_ms = duration_milliseconds(self.started.elapsed())
        );
    }
}

impl Drop for SearchLifecycleTrace {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        tracing::warn!(
            event = "publication_search_cancelled",
            request_key = self.request_key,
            phase = self.phase,
            elapsed_ms = duration_milliseconds(self.started.elapsed())
        );
    }
}

/// Analysis and search derivation shared by import, reopen, and the fixture
/// runner. The `fixture_` prefix these carried was misleading: they are the
/// product's own search derivation path.
pub(crate) fn derive_search_snapshot(
    analysis: &PublicationAnalysis,
    reading_order: &[ReadingOrderOccurrence],
) -> Result<SearchSnapshot, String> {
    let mut documents = BTreeMap::new();
    for facts in analysis.analyzed_resources() {
        let resource = facts.resource();
        let Some(path) = resource.local_path() else {
            continue;
        };
        let state = match facts.content() {
            AnalysisOutcome::Complete(content) if content.as_xhtml().is_some() => {
                SearchDocumentState::Complete
            }
            AnalysisOutcome::Partial { value, .. } if value.as_xhtml().is_some() => {
                SearchDocumentState::Partial
            }
            AnalysisOutcome::Unavailable(_) => SearchDocumentState::Unavailable,
            AnalysisOutcome::NotApplicable => SearchDocumentState::Inapplicable,
            AnalysisOutcome::Complete(_) | AnalysisOutcome::Partial { .. } => {
                SearchDocumentState::Inapplicable
            }
        };
        if documents.insert(path.as_str().to_owned(), state).is_some() {
            return Err(format!("duplicate search document path: {path}"));
        }
    }

    let occurrence_index = search_occurrence_index(reading_order);
    let entries = epub_navigator::search_text_entries(analysis)
        .filter_map(|entry| entry.resource.local_path().map(|path| (entry, path)))
        .map(|(entry, path)| SearchEntry {
            id: format!(
                "{}#text-{}-{}",
                path.as_str(),
                entry.range.start(),
                entry.range.end()
            ),
            resource_path: path.as_str().to_owned(),
            fragment: entry.fragment.map(str::to_owned),
            text: entry.text.into_owned(),
            reading_order_occurrence_indexes: occurrence_index
                .get(path.as_str())
                .cloned()
                .unwrap_or_default(),
        })
        .collect();
    let snapshot = SearchSnapshot {
        entries,
        documents,
        effective_reading_order_resources: reading_order_local_resources(reading_order),
    };
    validate_search_snapshot(&snapshot)?;
    Ok(snapshot)
}

pub fn search_analysis_limits() -> AnalysisLimits {
    let mut limits = AnalysisLimits::default();
    limits.max_analyzed_resources = Some(MAX_ANALYZED_RESOURCES);
    limits.max_resource_analysis_bytes = Some(MAX_RESOURCE_ANALYSIS_BYTES);
    limits.max_total_analysis_bytes = Some(MAX_TOTAL_ANALYSIS_BYTES);
    // Fingerprinting is declined rather than bounded: nothing in this application reads a
    // resource fingerprint, and a zero budget is how the analysis API says no. The preflight
    // refuses each resource before reading it, so the facts record why the value is absent
    // instead of claiming it was computed.
    limits.max_total_fingerprint_bytes = Some(0);
    limits.max_smil_nodes = Some(MAX_SMIL_NODES);
    limits.max_smil_nesting = Some(MAX_SMIL_NESTING);
    limits
}
