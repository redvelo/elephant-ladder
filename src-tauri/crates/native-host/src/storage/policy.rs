//! Persisted active-content and remote-resource permissions.
//!
//! Defaults, per-publication overrides, and the effective policy are separate
//! stored values, so an inherited default and an explicit choice that happens to
//! match remain distinguishable. Writes are revision compare-and-swap.

use super::Storage;
use crate::{storage::fs, storage::is_blake3_hash};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

impl Storage {
    pub fn load_global_publication_policy(&self) -> Result<StoredPublicationPolicy, String> {
        let (global, global_revision) = load_global_publication_policy(&self.connection)?;
        Ok(stored_publication_policy(
            global,
            PublicationPolicyOverrides::default(),
            global_revision,
            0,
        ))
    }

    pub fn load_publication_policy(
        &self,
        publication_id: &str,
    ) -> Result<StoredPublicationPolicy, String> {
        ensure_live_policy_publication(&self.connection, publication_id)?;
        let (global, global_revision) = load_global_publication_policy(&self.connection)?;
        let (publication, publication_revision) =
            load_publication_policy_overrides(&self.connection, publication_id)?;
        Ok(stored_publication_policy(
            global,
            publication,
            global_revision,
            publication_revision,
        ))
    }

    pub fn load_publication_policy_relevance(
        &self,
        publication_id: &str,
        reading_revision: &str,
    ) -> Result<PublicationPolicyRelevance, String> {
        let stored = self
            .connection
            .query_row(
                "SELECT s.active_content_relevance, s.remote_resources_relevance,
                    b.blob_hash, b.size_bytes
             FROM publications p
             JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
               AND s.publication_id = p.publication_id
               AND s.blob_hash = p.imported_blob_hash
             JOIN archive_blobs b ON b.blob_hash = s.blob_hash
             WHERE p.publication_id = ?1 AND p.deletion_state = 'live'
               AND s.reading_revision = ?2 AND s.revoked_at IS NULL
               AND b.health_state = 'available' AND b.lifecycle_state = 'live'
               AND b.storage_name = b.blob_hash AND b.size_bytes > 0",
                params![publication_id, reading_revision],
                |row| {
                    // Invalid persisted classifications must never imply absence.
                    let relevance = |index| {
                        row.get_ref(index)
                            .ok()
                            .and_then(|value| value.as_str().ok())
                            .and_then(|value| PolicyRelevance::parse(value).ok())
                            .unwrap_or(PolicyRelevance::Unknown)
                    };
                    Ok((
                        PublicationPolicyRelevance {
                            active_content: relevance(0),
                            remote_resources: relevance(1),
                        },
                        row.get::<_, String>(2)?,
                        row.get::<_, u64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not read publication policy relevance: {error}"))?;
        let Some((relevance, hash, size)) = stored else {
            return Ok(PublicationPolicyRelevance::unknown());
        };
        if !is_blake3_hash(&hash)
            || !fs::metadata(self.blobs.join(&hash))
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == size)
        {
            return Ok(PublicationPolicyRelevance::unknown());
        }
        Ok(relevance)
    }

    pub(crate) fn compare_and_swap_global_publication_policy(
        &mut self,
        base_revision: u32,
        policy: GlobalPublicationPolicy,
    ) -> Result<SavePublicationPolicyOutcome, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin publication policy transaction: {error}"))?;
        let (current_global, global_revision) = load_global_publication_policy(&transaction)?;
        let current = stored_publication_policy(
            current_global,
            PublicationPolicyOverrides::default(),
            global_revision,
            0,
        );
        if base_revision != global_revision {
            return Ok(SavePublicationPolicyOutcome::RevisionMismatch(current));
        }
        if policy == current_global {
            return Ok(SavePublicationPolicyOutcome::Saved(current));
        }
        let Some(next_revision) = global_revision.checked_add(1) else {
            return Ok(SavePublicationPolicyOutcome::RevisionExhausted(current));
        };
        save_global_publication_policy(&transaction, policy, next_revision)?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication policy: {error}"))?;
        Ok(SavePublicationPolicyOutcome::Saved(
            stored_publication_policy(
                policy,
                PublicationPolicyOverrides::default(),
                next_revision,
                0,
            ),
        ))
    }

    pub(crate) fn compare_and_swap_publication_policy(
        &mut self,
        publication_id: &str,
        base_global_revision: u32,
        base_publication_revision: u32,
        policy: PublicationPolicyOverrides,
    ) -> Result<SavePublicationPolicyOutcome, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin publication policy transaction: {error}"))?;
        ensure_live_policy_publication(&transaction, publication_id)?;
        let (current_global, global_revision) = load_global_publication_policy(&transaction)?;
        let (current_publication, publication_revision) =
            load_publication_policy_overrides(&transaction, publication_id)?;
        let current = stored_publication_policy(
            current_global,
            current_publication,
            global_revision,
            publication_revision,
        );
        if base_global_revision != global_revision
            || base_publication_revision != publication_revision
        {
            return Ok(SavePublicationPolicyOutcome::RevisionMismatch(current));
        }

        if policy == current_publication {
            return Ok(SavePublicationPolicyOutcome::Saved(current));
        }
        let Some(next_publication_revision) = publication_revision.checked_add(1) else {
            return Ok(SavePublicationPolicyOutcome::RevisionExhausted(current));
        };
        transaction
            .execute(
                "INSERT INTO publication_policy_overrides
                    (publication_id, active_content, remote_resources, policy_revision)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(publication_id) DO UPDATE SET
                    active_content = excluded.active_content,
                    remote_resources = excluded.remote_resources,
                    policy_revision = excluded.policy_revision,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![
                    publication_id,
                    policy.active_content.map(ActiveContentOverride::as_str),
                    policy.remote_resources.map(RemoteResourcesOverride::as_str),
                    next_publication_revision
                ],
            )
            .map_err(|error| format!("could not save publication policy override: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication policy: {error}"))?;
        Ok(SavePublicationPolicyOutcome::Saved(
            stored_publication_policy(
                current_global,
                policy,
                global_revision,
                next_publication_revision,
            ),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActiveContentDefault {
    Automatic,
    Ask,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteResourcesDefault {
    Allowed,
    Ask,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActiveContentOverride {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteResourcesOverride {
    Allowed,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectiveActiveContentPolicy {
    Enabled,
    Ask,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectiveRemoteResourcesPolicy {
    Allowed,
    Ask,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalPublicationPolicy {
    pub active_content: ActiveContentDefault,
    pub remote_resources: RemoteResourcesDefault,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPolicyOverrides {
    pub active_content: Option<ActiveContentOverride>,
    pub remote_resources: Option<RemoteResourcesOverride>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPolicy {
    pub global: GlobalPublicationPolicy,
    pub publication: PublicationPolicyOverrides,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectivePublicationPolicy {
    pub active_content: EffectiveActiveContentPolicy,
    pub remote_resources: EffectiveRemoteResourcesPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPolicyRevisions {
    pub global: u32,
    pub publication: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredPublicationPolicy {
    pub policy: PublicationPolicy,
    pub effective: EffectivePublicationPolicy,
    pub revisions: PublicationPolicyRevisions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyRelevance {
    Detected,
    NotDetected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPolicyRelevance {
    pub active_content: PolicyRelevance,
    pub remote_resources: PolicyRelevance,
}

pub(crate) enum SavePublicationPolicyOutcome {
    Saved(StoredPublicationPolicy),
    RevisionMismatch(StoredPublicationPolicy),
    RevisionExhausted(StoredPublicationPolicy),
}

pub(crate) fn ensure_live_policy_publication(
    connection: &Connection,
    publication_id: &str,
) -> Result<(), String> {
    let live = connection
        .query_row(
            "SELECT 1 FROM publications
             WHERE publication_id = ?1 AND deletion_state = 'live'",
            [publication_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| format!("could not validate publication policy scope: {error}"))?
        .is_some();
    if !live {
        return Err("publication policy scope is unavailable".to_owned());
    }
    Ok(())
}

pub(crate) fn load_publication_policy_overrides(
    connection: &Connection,
    publication_id: &str,
) -> Result<(PublicationPolicyOverrides, u32), String> {
    let stored = connection
        .query_row(
            "SELECT active_content, remote_resources, policy_revision
             FROM publication_policy_overrides WHERE publication_id = ?1",
            [publication_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("could not load publication policy override: {error}"))?;
    let Some((active_content, remote_resources, revision)) = stored else {
        return Ok((PublicationPolicyOverrides::default(), 0));
    };
    let revision = stored_policy_revision(revision, "publication")?;
    Ok((
        PublicationPolicyOverrides {
            active_content: active_content
                .as_deref()
                .map(ActiveContentOverride::parse)
                .transpose()?,
            remote_resources: remote_resources
                .as_deref()
                .map(RemoteResourcesOverride::parse)
                .transpose()?,
        },
        revision,
    ))
}

pub(crate) fn stored_policy_revision(revision: i64, scope: &str) -> Result<u32, String> {
    u32::try_from(revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| format!("stored {scope} publication policy revision is invalid"))
}

pub(crate) fn stored_publication_policy(
    global: GlobalPublicationPolicy,
    publication: PublicationPolicyOverrides,
    global_revision: u32,
    publication_revision: u32,
) -> StoredPublicationPolicy {
    let policy = PublicationPolicy {
        global,
        publication,
    };
    StoredPublicationPolicy {
        policy,
        effective: policy.effective(),
        revisions: PublicationPolicyRevisions {
            global: global_revision,
            publication: publication_revision,
        },
    }
}

impl PublicationPolicyRelevance {
    pub(super) fn unknown() -> Self {
        Self {
            active_content: PolicyRelevance::Unknown,
            remote_resources: PolicyRelevance::Unknown,
        }
    }
}

pub(super) fn load_global_publication_policy(
    connection: &Connection,
) -> Result<(GlobalPublicationPolicy, u32), String> {
    let stored = connection
        .query_row(
            "SELECT active_content, remote_resources, policy_revision
             FROM publication_policy_defaults WHERE singleton_key = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("could not load global publication policy: {error}"))?;
    let Some((active_content, remote_resources, revision)) = stored else {
        return Ok((GlobalPublicationPolicy::default(), 0));
    };
    let revision = stored_policy_revision(revision, "global")?;
    Ok((
        GlobalPublicationPolicy {
            active_content: ActiveContentDefault::parse(&active_content)?,
            remote_resources: RemoteResourcesDefault::parse(&remote_resources)?,
        },
        revision,
    ))
}

pub(super) fn save_global_publication_policy(
    connection: &Connection,
    policy: GlobalPublicationPolicy,
    revision: u32,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO publication_policy_defaults
                (singleton_key, active_content, remote_resources, policy_revision)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(singleton_key) DO UPDATE SET
                active_content = excluded.active_content,
                remote_resources = excluded.remote_resources,
                policy_revision = excluded.policy_revision,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                policy.active_content.as_str(),
                policy.remote_resources.as_str(),
                revision
            ],
        )
        .map_err(|error| format!("could not save global publication policy: {error}"))?;
    Ok(())
}
