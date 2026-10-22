//! Publications the platform asked this application to open.
//!
//! The platform hands over a local file: an argument on the desktop, a staged copy of a
//! content URI on Android. The host only offers it to the trusted reader, which imports it
//! through the ordinary import route, so acquisition keeps one implementation and one
//! privileged path. Bytes are read once and a platform copy is removed with them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::http::TrustedRead;
use crate::{
    MAX_ACQUISITION_BYTES, OpenedPublication, OpenedPublicationIntent, ReaderState,
    generate_session_token,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// A held request answers as soon as something arrives, so the wait only bounds an idle poll.
#[cfg(not(test))]
const WAIT: Duration = Duration::from_secs(20);
#[cfg(test)]
const WAIT: Duration = Duration::from_millis(400);
const MAX_FILE_NAME_BYTES: usize = 255;
/// A drop of a whole shelf is still one gesture; beyond this the rest is refused rather than
/// held, because each one waits on an import that has not started yet.
const MAX_WAITING: usize = 64;

/// A publication waiting for the reader to import it.
#[derive(Clone, Debug)]
pub(crate) struct PendingOpenedPublication {
    pub(crate) open_id: String,
    pub(crate) file_name: String,
    pub(crate) size_bytes: u64,
    pub(crate) path: PathBuf,
    pub(crate) owned: bool,
    pub(crate) intent: OpenedPublicationIntent,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingResponse<'a> {
    open_id: &'a str,
    file_name: &'a str,
    size_bytes: u64,
    intent: OpenedPublicationIntent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingQuery {
    after: Option<String>,
}

/// A file name is only a label here: the reader never opens a path, and the import route
/// names the publication from its own metadata.
fn safe_file_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_FILE_NAME_BYTES
        || trimmed.chars().any(|character| {
            character.is_control() || matches!(character, '/' | '\\' | ':' | '"' | '\0')
        })
    {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Reports the publication the platform handed over, waiting for one to arrive.
pub(super) async fn pending(
    State(state): State<ReaderState>,
    Query(query): Query<PendingQuery>,
    _: TrustedRead,
) -> Response {
    let deadline = Instant::now() + WAIT;
    let mut response = loop {
        receive_opened_publications(&state).await;
        let pending = state
            .pending_opened_publications
            .lock()
            .expect("pending opened publication lock poisoned")
            .front()
            .cloned();
        match pending {
            Some(pending) if query.after.as_deref() != Some(pending.open_id.as_str()) => {
                break Json(PendingResponse {
                    open_id: &pending.open_id,
                    file_name: &pending.file_name,
                    size_bytes: pending.size_bytes,
                    intent: pending.intent,
                })
                .into_response();
            }
            _ if Instant::now() >= deadline => break StatusCode::NO_CONTENT.into_response(),
            _ => tokio::time::sleep(POLL_INTERVAL).await,
        }
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Answers the bytes once. The offer is consumed whether or not the read succeeds, so a file
/// the platform staged never lingers and a failed import is reported by the reader.
pub(super) async fn bytes(
    State(state): State<ReaderState>,
    Path(open_id): Path<String>,
    _: TrustedRead,
) -> Response {
    let claimed = {
        let mut waiting = state
            .pending_opened_publications
            .lock()
            .expect("pending opened publication lock poisoned");
        waiting
            .iter()
            .position(|candidate| candidate.open_id == open_id)
            .and_then(|index| waiting.remove(index))
    };
    let Some(claimed) = claimed else {
        return (StatusCode::NOT_FOUND, "no publication was handed over").into_response();
    };
    let read = tokio::task::spawn_blocking(move || {
        let contents = std::fs::read(&claimed.path);
        if claimed.owned {
            let _ = std::fs::remove_file(&claimed.path);
        }
        contents
    })
    .await;
    match read {
        Ok(Ok(contents)) if contents.len() as u64 <= MAX_ACQUISITION_BYTES => {
            let length = contents.len();
            let mut response = Body::from(contents).into_response();
            let headers = response.headers_mut();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/epub+zip"),
            );
            headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Ok(Ok(_)) => {
            tracing::warn!(event = "opened_publication_rejected", reason = "size");
            (StatusCode::PAYLOAD_TOO_LARGE, "publication is too large").into_response()
        }
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(event = "opened_publication_rejected", reason = "unreadable");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "the handed publication could not be read",
            )
                .into_response()
        }
    }
}

/// Queues every arrival: a drop hands over as many books as the person chose, and each waits
/// its turn behind the import before it.
async fn receive_opened_publications(state: &ReaderState) {
    let actions = state.native_actions.clone();
    let Ok(Ok(opened)) =
        tokio::task::spawn_blocking(move || actions.take_opened_publications()).await
    else {
        return;
    };
    for publication in opened {
        let path = publication.path.clone();
        let Ok(metadata) = tokio::task::spawn_blocking(move || std::fs::metadata(&path)).await
        else {
            return;
        };
        if let Err(reason) = enqueue_opened_publication(state, publication, metadata) {
            tracing::warn!(event = "opened_publication_rejected", reason);
        }
    }
}

/// Hands the host a publication the way the platform does, from inside the process.
///
/// A non-product build offers its fixture through here, so the fixture travels the
/// same import route as a book handed over by the platform. The caller already has
/// the file; reading its metadata here is the one blocking step and it is cheap.
pub fn offer_opened_publication(
    state: &ReaderState,
    publication: OpenedPublication,
) -> Result<(), &'static str> {
    let metadata = std::fs::metadata(&publication.path);
    enqueue_opened_publication(state, publication, metadata)
}

fn enqueue_opened_publication(
    state: &ReaderState,
    publication: OpenedPublication,
    metadata: std::io::Result<std::fs::Metadata>,
) -> Result<(), &'static str> {
    let file_name = safe_file_name(&publication.file_name).ok_or("name")?;
    let metadata = metadata.map_err(|_| "unreadable")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ACQUISITION_BYTES {
        return Err("size");
    }
    let open_id = generate_session_token().map_err(|_| "token")?;
    let mut waiting = state
        .pending_opened_publications
        .lock()
        .expect("pending opened publication lock poisoned");
    if waiting.len() >= MAX_WAITING {
        return Err("waiting");
    }
    tracing::debug!(event = "opened_publication_received");
    waiting.push_back(PendingOpenedPublication {
        open_id,
        file_name,
        size_bytes: metadata.len(),
        path: publication.path,
        owned: publication.owned,
        intent: publication.intent,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::safe_file_name;

    #[test]
    fn file_names_carrying_paths_or_control_characters_are_refused() {
        assert_eq!(
            safe_file_name(" Alice.epub "),
            Some("Alice.epub".to_owned())
        );
        assert_eq!(safe_file_name(""), None);
        assert_eq!(safe_file_name("   "), None);
        assert_eq!(safe_file_name("../escape.epub"), None);
        assert_eq!(safe_file_name("books/alice.epub"), None);
        assert_eq!(safe_file_name("alice\n.epub"), None);
        assert_eq!(safe_file_name(&"a".repeat(256)), None);
    }
}
