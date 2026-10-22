//! Active-content and remote-resource permissions.
//!
//! Resolution is fail-closed: an unresolved `ask` policy prompts before a reader
//! session exists or any publisher script runs, and the two decisions are
//! independent. Saves are revision compare-and-swap and are refused while a
//! reader is active, so a change applies on the next open rather than revoking a
//! live runtime.

use crate::http::{HostError, HostResult};
use crate::library::valid_publication_id;
use crate::{
    MAX_PUBLICATION_POLICY_REQUEST_BYTES, PRODUCTION_READING_REVISION, ReaderState, TrustedJson,
    TrustedRead, request_library_document, storage,
};
use axum::{
    Json, Router, extract::DefaultBodyLimit, extract::Path, extract::State, http::HeaderMap,
    response::IntoResponse, routing::get,
};
use epub_stack::{
    PublicationAnalysis, analysis::coverage::Completeness, analysis::reference::AuthoredReference,
    analysis::reference::HrefRole, analysis::reference::HrefTarget, content::ContentFacts,
    package::manifest::KnownManifestProperty, resource::ResourceAddress,
};
use serde::{Deserialize, Serialize};

/// Publication-permission routes on the trusted reader origin.
pub(crate) fn routes() -> Router<ReaderState> {
    let limit = DefaultBodyLimit::max(MAX_PUBLICATION_POLICY_REQUEST_BYTES);
    Router::new()
        .route(
            "/native/publication-policy",
            get(read_global_publication_policy)
                .put(save_global_publication_policy)
                .layer(limit),
        )
        .route(
            "/native/publications/{publication_id}/policy",
            get(read_publication_policy)
                .put(save_publication_policy)
                .layer(limit),
        )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedPublicationPolicy {
    pub active_content: ResolvedActiveContentPolicy,
    pub remote_resources: ResolvedRemoteResourcesPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolvedActiveContentPolicy {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolvedRemoteResourcesPolicy {
    Allowed,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyDecisionRequirement {
    pub active_content: bool,
    pub remote_resources: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveGlobalPublicationPolicyRequest {
    base_revision: u32,
    policy: storage::GlobalPublicationPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavePublicationPolicyRequest {
    base_revisions: storage::PublicationPolicyRevisions,
    policy: storage::PublicationPolicyOverrides,
}

pub fn resolve_publication_policy(
    stored: &storage::StoredPublicationPolicy,
    relevance: storage::PublicationPolicyRelevance,
) -> Result<ResolvedPublicationPolicy, PolicyDecisionRequirement> {
    let (active_content, active_content_required) = match stored.policy.publication.active_content {
        Some(storage::ActiveContentOverride::Enabled) => {
            (ResolvedActiveContentPolicy::Enabled, false)
        }
        Some(storage::ActiveContentOverride::Disabled) => {
            (ResolvedActiveContentPolicy::Disabled, false)
        }
        None => match stored.policy.global.active_content {
            storage::ActiveContentDefault::Automatic => (
                if relevance.active_content == storage::PolicyRelevance::Unknown {
                    ResolvedActiveContentPolicy::Disabled
                } else {
                    ResolvedActiveContentPolicy::Enabled
                },
                false,
            ),
            storage::ActiveContentDefault::Ask => (
                ResolvedActiveContentPolicy::Disabled,
                relevance.active_content != storage::PolicyRelevance::NotDetected,
            ),
            storage::ActiveContentDefault::Disabled => {
                (ResolvedActiveContentPolicy::Disabled, false)
            }
        },
    };
    let (remote_resources, remote_resources_required) =
        match stored.policy.publication.remote_resources {
            Some(storage::RemoteResourcesOverride::Allowed) => {
                (ResolvedRemoteResourcesPolicy::Allowed, false)
            }
            Some(storage::RemoteResourcesOverride::Blocked) => {
                (ResolvedRemoteResourcesPolicy::Blocked, false)
            }
            None => match stored.policy.global.remote_resources {
                storage::RemoteResourcesDefault::Allowed => {
                    (ResolvedRemoteResourcesPolicy::Allowed, false)
                }
                storage::RemoteResourcesDefault::Ask => (
                    ResolvedRemoteResourcesPolicy::Blocked,
                    relevance.remote_resources != storage::PolicyRelevance::NotDetected,
                ),
                storage::RemoteResourcesDefault::Blocked => {
                    (ResolvedRemoteResourcesPolicy::Blocked, false)
                }
            },
        };
    if active_content_required || remote_resources_required {
        return Err(PolicyDecisionRequirement {
            active_content: active_content_required,
            remote_resources: remote_resources_required,
        });
    }
    Ok(ResolvedPublicationPolicy {
        active_content,
        remote_resources,
    })
}

async fn read_global_publication_policy(
    State(state): State<ReaderState>,
    _: TrustedRead,
) -> HostResult {
    let storage = state.storage.clone();
    match tokio::task::spawn_blocking(move || storage.locked()?.load_global_publication_policy())
        .await
    {
        Ok(Ok(policy)) => Ok(Json(policy).into_response()),
        Ok(Err(_)) => Err(publication_policy_failure("read", "storage")),
        Err(_) => Err(publication_policy_failure("read", "worker")),
    }
}

async fn save_global_publication_policy(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    TrustedJson(request): TrustedJson<SaveGlobalPublicationPolicyRequest>,
) -> HostResult {
    let operation = state.lifecycle.lock_owned().await;
    request_library_document(&headers, &state).map_err(HostError::conflict_reason)?;
    if operation.reader_active() {
        return Err(HostError::conflict_reason("publication is open"));
    }
    let storage = state.storage.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _operation = operation;
        storage
            .locked()?
            .compare_and_swap_global_publication_policy(request.base_revision, request.policy)
    })
    .await;
    publication_policy_save_response(outcome)
}

async fn read_publication_policy(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    _: TrustedRead,
) -> HostResult {
    if !valid_publication_id(&publication_id) {
        return Err(HostError::NotFound);
    }
    let storage = state.storage.clone();
    match tokio::task::spawn_blocking(move || {
        let storage = storage.locked()?;
        let policy = storage.load_publication_policy(&publication_id)?;
        let relevance = storage
            .load_publication_policy_relevance(&publication_id, PRODUCTION_READING_REVISION)?;
        Ok::<_, String>(serde_json::json!({
            "policy": policy.policy,
            "effective": policy.effective,
            "revisions": policy.revisions,
            "relevance": relevance,
        }))
    })
    .await
    {
        Ok(Ok(policy)) => Ok(Json(policy).into_response()),
        Ok(Err(error)) if publication_policy_unavailable(&error) => Err(HostError::NotFound),
        Ok(Err(_)) => Err(publication_policy_failure("read", "storage")),
        Err(_) => Err(publication_policy_failure("read", "worker")),
    }
}

async fn save_publication_policy(
    State(state): State<ReaderState>,
    Path(publication_id): Path<String>,
    headers: HeaderMap,
    TrustedJson(request): TrustedJson<SavePublicationPolicyRequest>,
) -> HostResult {
    if !valid_publication_id(&publication_id) {
        return Err(HostError::NotFound);
    }
    let operation = state.lifecycle.lock_owned().await;
    request_library_document(&headers, &state).map_err(HostError::conflict_reason)?;
    if operation.reader_active() {
        return Err(HostError::conflict_reason("publication is open"));
    }
    let storage = state.storage.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _operation = operation;
        storage.locked()?.compare_and_swap_publication_policy(
            &publication_id,
            request.base_revisions.global,
            request.base_revisions.publication,
            request.policy,
        )
    })
    .await;
    match outcome {
        Ok(Err(error)) if publication_policy_unavailable(&error) => Err(HostError::NotFound),
        outcome => publication_policy_save_response(outcome),
    }
}

fn publication_policy_save_response(
    outcome: Result<Result<storage::SavePublicationPolicyOutcome, String>, tokio::task::JoinError>,
) -> HostResult {
    match outcome {
        Ok(Ok(storage::SavePublicationPolicyOutcome::Saved(policy))) => {
            Ok(Json(serde_json::json!({
                "status": "saved",
                "policy": policy.policy,
                "effective": policy.effective,
                "revisions": policy.revisions,
            }))
            .into_response())
        }
        Ok(Ok(storage::SavePublicationPolicyOutcome::RevisionMismatch(policy))) => {
            Err(publication_policy_conflict("revision-mismatch", policy))
        }
        Ok(Ok(storage::SavePublicationPolicyOutcome::RevisionExhausted(policy))) => {
            Err(publication_policy_conflict("revision-exhausted", policy))
        }
        Ok(Err(_)) => Err(publication_policy_failure("save", "storage")),
        Err(_) => Err(publication_policy_failure("save", "worker")),
    }
}

fn publication_policy_conflict(
    reason: &'static str,
    policy: storage::StoredPublicationPolicy,
) -> HostError {
    HostError::conflict(
        reason,
        serde_json::json!({
            "policy": policy.policy,
            "effective": policy.effective,
            "revisions": policy.revisions,
        }),
    )
}

fn publication_policy_failure(operation: &'static str, error_kind: &'static str) -> HostError {
    tracing::error!(event = "publication_policy_failed", operation, error_kind);
    HostError::Internal(operation)
}

fn publication_policy_unavailable(error: &str) -> bool {
    error == "publication policy scope is unavailable"
}

pub(crate) fn publication_policy_relevance(
    analysis: &PublicationAnalysis,
) -> storage::PublicationPolicyRelevance {
    let active_content_detected = analysis.resources().resources().any(|resource| {
        resource.is_scripted()
            || analysis
                .resource(resource.ordinal())
                .and_then(|resource| resource.content().value())
                .and_then(ContentFacts::executable_content_detected)
                == Some(true)
    });
    let active_content = if active_content_detected {
        storage::PolicyRelevance::Detected
    } else if analysis.coverage().content().is_complete() {
        storage::PolicyRelevance::NotDetected
    } else {
        storage::PolicyRelevance::Unknown
    };

    let remote_manifest_evidence = analysis.resources().resources().any(|resource| {
        matches!(resource.address(), ResourceAddress::Remote(_))
            || resource.declarations().any(|declaration| {
                declaration.properties().iter().any(|property| {
                    property.known_value() == Some(KnownManifestProperty::RemoteResources)
                })
            })
    });
    let remote_reference_evidence = analysis.references().any(|reference| {
        matches!(
            reference,
            AuthoredReference::Href(reference)
                if matches!(reference.target(), HrefTarget::Remote { .. })
                    && remote_network_role(reference.role())
        )
    });
    let relationships_complete = !analysis.coverage().relationships().is_empty()
        && analysis
            .coverage()
            .relationships()
            .iter()
            .all(|coverage| matches!(coverage.completeness, Completeness::Complete));
    let remote_resources = if remote_manifest_evidence || remote_reference_evidence {
        storage::PolicyRelevance::Detected
    } else if analysis.coverage().content().is_complete() && relationships_complete {
        storage::PolicyRelevance::NotDetected
    } else {
        storage::PolicyRelevance::Unknown
    };

    storage::PublicationPolicyRelevance {
        active_content,
        remote_resources,
    }
}

fn remote_network_role(role: HrefRole) -> bool {
    matches!(
        role,
        HrefRole::Ncx
            | HrefRole::Stylesheet
            | HrefRole::FormAction
            | HrefRole::Image
            | HrefRole::Script
            | HrefRole::Audio
            | HrefRole::Video
            | HrefRole::Source
            | HrefRole::Track
            | HrefRole::Poster
            | HrefRole::Object
            | HrefRole::Embed
            | HrefRole::Iframe
            | HrefRole::Svg
            | HrefRole::SmilText
            | HrefRole::SmilAudio
            | HrefRole::CssImport
            | HrefRole::CssUrl
            | HrefRole::Font
    )
}
