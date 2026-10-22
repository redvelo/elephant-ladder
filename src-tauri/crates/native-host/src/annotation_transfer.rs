use crate::http::{HostError, Trusted, TrustedRead, TrustedWrite};
use axum::extract::Query;
use time::OffsetDateTime;

use crate::NativeActionError;
use crate::ReaderState;
use crate::annotation_exchange::{
    ExchangePublication, ImportError, ImportReport, export_annotation_set, merge_annotation_set,
    parse_annotation_set,
};
use crate::http::not_found;
use crate::library::valid_publication_id;
use crate::reading_order_local_resources;
use crate::request_library_document;
use crate::storage;
use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::response::Response;
use epub_navigator::kernel::project_reading_order;
use epub_stack::EpubPath;
use epub_stack::EpubZip;
use serde::{Deserialize, Serialize};

const MAX_EXPORT_FILE_STEM_CHARS: usize = 80;

struct ExchangeContext {
    source: storage::AnnotationExchangeSource,
    unique_identifier: Option<String>,
    package_path: EpubPath,
    reading_order_resources: Vec<Option<String>>,
}

impl ExchangeContext {
    fn publication(&self) -> ExchangePublication<'_> {
        ExchangePublication {
            unique_identifier: self.unique_identifier.as_deref(),
            blob_hash: &self.source.blob_hash,
            title: &self.source.title,
            creators: &self.source.creators,
            package_path: &self.package_path,
            reading_order_resources: &self.reading_order_resources,
        }
    }
}

enum ContextError {
    NotFound,
    Failed,
}

fn load_context(
    state: &ReaderState,
    publication_id: &str,
) -> Result<ExchangeContext, ContextError> {
    let source = state
        .storage
        .lock()
        .map_err(|_| ContextError::Failed)?
        .load_annotation_exchange_source(publication_id)
        .map_err(|_| ContextError::Failed)?
        .ok_or(ContextError::NotFound)?;
    let epub = EpubZip::open(&source.blob_path)
        .map_err(|_| ContextError::Failed)?
        .default_rendition()
        .map_err(|_| ContextError::Failed)?;
    let facts =
        epub_navigator::facts::PublicationFacts::of(&epub).map_err(|_| ContextError::Failed)?;
    let reading_order = project_reading_order(&facts).map_err(|_| ContextError::Failed)?;
    Ok(ExchangeContext {
        unique_identifier: facts
            .package
            .unique_identifier()
            .map(|identifier| identifier.as_str().trim().to_owned())
            .filter(|identifier| !identifier.is_empty()),
        package_path: facts.resources.package_path().clone(),
        reading_order_resources: reading_order_local_resources(&reading_order),
        source,
    })
}

fn export_json(context: &ExchangeContext) -> Result<String, String> {
    let mut set_id = [0_u8; 16];
    getrandom::fill(&mut set_id).map_err(|error| error.to_string())?;
    export_annotation_set(
        &context.publication(),
        &context.source.annotations.records,
        set_id,
        OffsetDateTime::now_utc(),
    )
}

fn context_failure(error: ContextError, event: &'static str) -> Response {
    match error {
        ContextError::NotFound => HostError::NotFound.into_response(),
        ContextError::Failed => {
            tracing::error!(event, error_kind = "context");
            HostError::Internal("annotation-transfer").into_response()
        }
    }
}

fn worker_failure(event: &'static str) -> Response {
    tracing::error!(event, error_kind = "worker");
    HostError::Internal("annotation-transfer").into_response()
}

pub(super) async fn export(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    _: TrustedRead,
) -> Response {
    if !valid_publication_id(&publication_id) {
        return not_found().await;
    }
    let exported = tokio::task::spawn_blocking(move || {
        let context = load_context(&state, &publication_id)?;
        export_json(&context).map_err(|_| ContextError::Failed)
    })
    .await;
    match exported {
        Ok(Ok(json)) => (
            [
                (CONTENT_TYPE, "application/ld+json"),
                (CACHE_CONTROL, "no-store"),
            ],
            json,
        )
            .into_response(),
        Ok(Err(error)) => context_failure(error, "annotation_export_failed"),
        Err(_) => worker_failure("annotation_export_failed"),
    }
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum SaveResponse {
    Saved { location: String },
    Cancelled,
}

pub(super) async fn save(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    _: Trusted,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, message).into_response();
    }
    if !valid_publication_id(&publication_id) {
        return not_found().await;
    }
    let saved = tokio::task::spawn_blocking(move || {
        let context = load_context(&state, &publication_id).map_err(Some)?;
        let json = export_json(&context).map_err(|_| Some(ContextError::Failed))?;
        let file_name = export_file_name(&context.source.title);
        state
            .native_actions
            .save_file(&file_name, "application/json", json.as_bytes())
            .map_err(|error| match error {
                NativeActionError::Unsupported => None,
                NativeActionError::Failed(_) => Some(ContextError::Failed),
            })
    })
    .await;
    match saved {
        Ok(Ok(Some(location))) => Json(SaveResponse::Saved { location }).into_response(),
        Ok(Ok(None)) => Json(SaveResponse::Cancelled).into_response(),
        Ok(Err(None)) => (StatusCode::NOT_IMPLEMENTED, "native action unsupported").into_response(),
        Ok(Err(Some(error))) => context_failure(error, "annotation_save_failed"),
        Err(_) => worker_failure("annotation_save_failed"),
    }
}

fn export_file_name(title: &str) -> String {
    let stem = title
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_' | '.') {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stem = stem
        .trim_matches('.')
        .chars()
        .take(MAX_EXPORT_FILE_STEM_CHARS)
        .collect::<String>();
    let stem = if stem.is_empty() {
        "Publication"
    } else {
        stem.trim()
    };
    format!("{stem} annotations.json")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ImportQuery {
    #[serde(default)]
    allow_publication_mismatch: bool,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum ImportResponse {
    Imported { report: ImportReport },
    PublicationMismatch,
    PublicationOpen,
}

pub(super) async fn import(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    Query(query): Query<ImportQuery>,
    headers: HeaderMap,
    _: TrustedWrite,
    body: Bytes,
) -> Response {
    if let Err(message) = request_library_document(&headers, &state) {
        return (StatusCode::CONFLICT, message).into_response();
    }
    if !valid_publication_id(&publication_id) {
        return not_found().await;
    }
    let Ok(input) = String::from_utf8(body.to_vec()) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "annotation set is malformed",
        )
            .into_response();
    };
    // Holding the lifecycle gate keeps the publication from opening against a stale projection.
    let _operation = state.lifecycle.lock().await;
    if state
        .lifecycle
        .load_active()
        .is_some_and(|active| active.publication_id.as_ref() == publication_id)
    {
        return (StatusCode::CONFLICT, Json(ImportResponse::PublicationOpen)).into_response();
    }
    let worker_state = state.clone();
    let imported = tokio::task::spawn_blocking(move || {
        let state = worker_state;
        let context = load_context(&state, &publication_id).map_err(ImportFailure::Context)?;
        let publication = context.publication();
        let set = parse_annotation_set(&input, &publication, query.allow_publication_mismatch)
            .map_err(ImportFailure::Import)?;
        let (_, report) = state
            .storage
            .lock()
            .map_err(|_| ImportFailure::Context(ContextError::Failed))?
            .update_annotation_records(
                &publication_id,
                &context.reading_order_resources,
                |existing| merge_annotation_set(&set, &publication, existing),
            )
            .map_err(ImportFailure::Save)?;
        Ok(report)
    })
    .await;
    match imported {
        Ok(Ok(report)) => Json(ImportResponse::Imported { report }).into_response(),
        Ok(Err(ImportFailure::Import(ImportError::PublicationMismatch))) => (
            StatusCode::CONFLICT,
            Json(ImportResponse::PublicationMismatch),
        )
            .into_response(),
        Ok(Err(ImportFailure::Import(ImportError::Malformed))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "annotation set is malformed",
        )
            .into_response(),
        Ok(Err(ImportFailure::Save(storage::SaveAnnotationError::Invalid))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "imported annotations exceed publication limits",
        )
            .into_response(),
        Ok(Err(ImportFailure::Save(storage::SaveAnnotationError::Storage))) => {
            context_failure(ContextError::Failed, "annotation_import_failed")
        }
        Ok(Err(ImportFailure::Context(error))) => {
            context_failure(error, "annotation_import_failed")
        }
        Err(_) => worker_failure("annotation_import_failed"),
    }
}

enum ImportFailure {
    Context(ContextError),
    Import(ImportError),
    Save(storage::SaveAnnotationError),
}
