use crate::http::TrustedRead;
use std::time::{Duration, Instant};

use axum::extract::Query;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use epub_stack::annotation::AnnotationTarget;
use epub_stack::resource::{AuthoredHref, resolve_publication_href};

use crate::ReaderState;
use crate::ReadingPosition;
use crate::ReadingPositionOccurrenceHint;
use crate::SnapshotEntry;
use crate::generate_session_token;
use crate::reading_order_local_resources;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::response::IntoResponse;
use axum::response::Response;
use base64::Engine;
use epub_stack::EpubPath;
use epub_stack::EpubZip;
use serde::{Deserialize, Serialize};

pub const ANNOTATION_LINK_SCHEME: &str = "elephant-ladder";
pub const MAX_ANNOTATION_LINK_BYTES: usize = 16 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 1024;
const LINK_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LINK_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, PartialEq)]
pub(crate) struct AnnotationLink {
    blob_hash: Option<String>,
    identifier: Option<String>,
    target_json: String,
}

/// Accepts only `elephant-ladder://annotation` links naming a book and a valid local target.
pub(crate) fn parse_annotation_link(value: &str) -> Option<AnnotationLink> {
    if value.len() > MAX_ANNOTATION_LINK_BYTES {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != ANNOTATION_LINK_SCHEME
        || url.host_str() != Some("annotation")
        || !matches!(url.path(), "" | "/")
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let (mut blob_hash, mut identifier, mut target) = (None, None, None);
    for (key, value) in url.query_pairs() {
        let slot = match key.as_ref() {
            "h" => &mut blob_hash,
            "id" => &mut identifier,
            "target" => &mut target,
            _ => return None,
        };
        if slot.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    if blob_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    }) || identifier.as_ref().is_some_and(|identifier| {
        identifier.trim().is_empty()
            || identifier.len() > MAX_IDENTIFIER_BYTES
            || identifier.chars().any(char::is_control)
    }) || (blob_hash.is_none() && identifier.is_none())
    {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(target?).ok()?;
    let parsed = AnnotationTarget::parse_json(std::str::from_utf8(&decoded).ok()?).ok()?;
    let target = AnnotationTarget::builder()
        .source(parsed.source().unwrap_or_default().to_owned())
        .selectors(parsed.selectors().to_vec())
        .build()
        .ok()?;
    Some(AnnotationLink {
        blob_hash,
        identifier,
        target_json: target.to_json_string().ok()?,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PendingAnnotationLink {
    Open {
        link_id: String,
        publication_id: String,
        target_json: String,
    },
    Missing {
        link_id: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
enum PendingResponse<'a> {
    #[serde(rename_all = "camelCase")]
    Open {
        link_id: &'a str,
        publication_id: &'a str,
    },
    #[serde(rename_all = "camelCase")]
    Missing { link_id: &'a str },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingQuery {
    after: Option<String>,
}

// A pending fetch completes promptly in a hidden page, unlike throttled timers.
pub(super) async fn pending(
    State(state): State<ReaderState>,
    Query(query): Query<PendingQuery>,
    _: TrustedRead,
) -> Response {
    let deadline = Instant::now() + LINK_WAIT;
    let mut response = loop {
        receive_native_links(&state).await;
        let pending = {
            let mut pending = state
                .pending_annotation_link
                .lock()
                .expect("pending annotation link lock poisoned");
            match pending.as_ref() {
                Some(PendingAnnotationLink::Open { link_id, .. })
                    if query.after.as_deref() == Some(link_id.as_str()) =>
                {
                    None
                }
                Some(PendingAnnotationLink::Missing { .. }) => pending.take(),
                _ => pending.clone(),
            }
        };
        match pending {
            Some(PendingAnnotationLink::Open {
                link_id,
                publication_id,
                ..
            }) => {
                break Json(PendingResponse::Open {
                    link_id: &link_id,
                    publication_id: &publication_id,
                })
                .into_response();
            }
            Some(PendingAnnotationLink::Missing { link_id }) => {
                break Json(PendingResponse::Missing { link_id: &link_id }).into_response();
            }
            None if Instant::now() >= deadline => break StatusCode::NO_CONTENT.into_response(),
            None => tokio::time::sleep(LINK_POLL_INTERVAL).await,
        }
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn receive_native_links(state: &ReaderState) {
    let actions = state.native_actions.clone();
    let Ok(Ok(links)) = tokio::task::spawn_blocking(move || actions.take_annotation_links()).await
    else {
        return;
    };
    let Some(link) = links
        .iter()
        .rev()
        .find_map(|link| parse_annotation_link(link))
    else {
        if !links.is_empty() {
            tracing::warn!(event = "annotation_link_rejected");
        }
        return;
    };
    let resolver_state = state.clone();
    let publication_id = tokio::task::spawn_blocking(move || {
        find_publication(&resolver_state, &link).map(|id| (id, link))
    })
    .await;
    let Ok(link_id) = generate_session_token() else {
        return;
    };
    let pending = match publication_id {
        Ok(Some((publication_id, link))) => PendingAnnotationLink::Open {
            link_id,
            publication_id,
            target_json: link.target_json,
        },
        Ok(None) => PendingAnnotationLink::Missing { link_id },
        Err(_) => return,
    };
    *state
        .pending_annotation_link
        .lock()
        .expect("pending annotation link lock poisoned") = Some(pending);
}

fn find_publication(state: &ReaderState, link: &AnnotationLink) -> Option<String> {
    let candidates = {
        let storage = state.storage.lock().ok()?;
        storage
            .list_publications()
            .ok()?
            .into_iter()
            .filter_map(|publication| {
                let blob = storage
                    .load_publication_blob(&publication.publication_id)
                    .ok()??;
                Some((publication.publication_id, blob))
            })
            .collect::<Vec<_>>()
    };
    if let Some(hash) = &link.blob_hash
        && let Some((publication_id, _)) = candidates.iter().find(|(_, blob)| &blob.hash == hash)
    {
        return Some(publication_id.clone());
    }
    let identifier = link.identifier.as_deref()?.trim();
    candidates.into_iter().find_map(|(publication_id, blob)| {
        let epub = EpubZip::open(&blob.path).ok()?.default_rendition().ok()?;
        let matches = epub
            .package()
            .unique_identifier()
            .is_some_and(|candidate| candidate.as_str().trim() == identifier);
        matches.then_some(publication_id)
    })
}

/// Consumes a pending link for this publication as the initial reading position.
pub(super) fn take_initial_position(
    state: &ReaderState,
    publication_id: &str,
    entry: &SnapshotEntry,
) -> Option<ReadingPosition> {
    let mut pending = state
        .pending_annotation_link
        .lock()
        .expect("pending annotation link lock poisoned");
    let Some(PendingAnnotationLink::Open {
        publication_id: pending_publication,
        ..
    }) = pending.as_ref()
    else {
        return None;
    };
    if pending_publication != publication_id {
        return None;
    }
    let Some(PendingAnnotationLink::Open { target_json, .. }) = pending.take() else {
        return None;
    };
    drop(pending);
    let package_path =
        EpubPath::new(entry.facts.get("resources")?.get("packagePath")?.as_str()?).ok()?;
    let source = AnnotationTarget::parse_json(&target_json)
        .ok()?
        .source()
        .unwrap_or_default()
        .to_owned();
    let resource = resolve_publication_href(&AuthoredHref::new(source), &package_path)?;
    let occurrence = reading_order_local_resources(&entry.reading_order)
        .iter()
        .position(|candidate| candidate.as_deref() == Some(resource.as_str()))?;
    Some(ReadingPosition {
        target_json,
        occurrence_hint: Some(ReadingPositionOccurrenceHint {
            occurrence: u32::try_from(occurrence).ok()?,
            resource: resource.as_str().to_owned(),
        }),
        roll_progression: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(query: &str) -> String {
        format!("elephant-ladder://annotation?{query}")
    }

    fn target(value: &str) -> String {
        URL_SAFE_NO_PAD.encode(value)
    }

    #[test]
    fn accepts_book_identity_and_normalizes_the_target() {
        let hash = "a".repeat(64);
        let parsed = parse_annotation_link(&link(&format!(
            "h={hash}&id=urn%3Aisbn%3A1&target={}",
            target(r#"{"source":"chapter.xhtml","meta":{"x":1}}"#)
        )))
        .unwrap();
        assert_eq!(
            parsed,
            AnnotationLink {
                blob_hash: Some(hash),
                identifier: Some("urn:isbn:1".to_owned()),
                target_json: r#"{"source":"chapter.xhtml"}"#.to_owned(),
            }
        );
    }

    #[test]
    fn rejects_malformed_links() {
        let valid_target = target(r#"{"source":"chapter.xhtml"}"#);
        for value in [
            link(&format!("target={valid_target}")),
            link(&format!("h=ABC&target={valid_target}")),
            link(&format!("id=x&id=y&target={valid_target}")),
            link(&format!("id=x&extra=1&target={valid_target}")),
            link("id=x&target=%%%"),
            link(&format!(
                "id=x&target={}",
                target(r#"{"source":"https://example.com/x"}"#)
            )),
            format!("elephant-ladder://other?id=x&target={valid_target}"),
            format!("https://annotation?id=x&target={valid_target}"),
            format!("elephant-ladder://annotation/path?id=x&target={valid_target}"),
            link(&format!(
                "id={}&target={valid_target}",
                "x".repeat(MAX_ANNOTATION_LINK_BYTES)
            )),
        ] {
            assert!(parse_annotation_link(&value).is_none(), "{value}");
        }
    }
}
