//! Committing an immutable publication snapshot and its derived products.
//!
//! A snapshot binds the exact source blob and selected rendition, validated
//! publication facts, a sealed response manifest, and the current reading
//! contract, and is committed in one durable transaction. Snapshot identity is
//! never reused: changed bytes, a different rendition, or an incompatible
//! reading contract produce a new one.

#[cfg(test)]
use super::publication_metadata_from_persisted_value;
use super::{
    PreparedTimeline, PublicationMetadata, PublicationSummary, Storage,
    decode_creator_directions_summary, decode_creators_summary,
    install_media_overlay_timelines_in_transaction, install_media_sidecars_in_transaction,
    prepare_media_overlay_timelines, publication_metadata, random_hex,
};
use crate::{
    ResponseManifest, media_overlay::MediaOverlayDescription,
    snapshot::validate_stored_reading_products, storage::InstalledBlob, storage::InstalledCover,
    storage::PublicationPolicyRelevance, storage::active_browser_origin_id,
    storage::allocate_browser_origin, storage::consume_import_reservation,
};
use epub_stack::package::Package;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use std::{collections::HashSet, path::PathBuf};

impl Storage {
    #[cfg(test)]
    /// Commits strict raw JSON products used by storage migration and corruption fixtures.
    pub(crate) fn commit_import_snapshot(
        &mut self,
        installed: InstalledImport<'_>,
        reading_revision: &str,
        title: &str,
        facts: &Value,
        responses: &Value,
    ) -> Result<ImportOutcome, String> {
        let facts_json = serde_json::to_string(facts)
            .map_err(|error| format!("could not serialize publication facts: {error}"))?;
        let responses_json = serde_json::to_string(responses)
            .map_err(|error| format!("could not serialize response manifest: {error}"))?;
        self.commit_import_snapshot_products(
            installed,
            reading_revision,
            title,
            PreparedSnapshotCommit {
                facts_json,
                responses_json,
                media_overlay_json: super::test_absent_media_overlay_json(),
                media_overlay_capability_json: serde_json::to_string(
                    &crate::media_overlay::MediaOverlayCapability::Absent {},
                )
                .expect("the absent narration capability always serializes"),
                media_overlay_timelines: Vec::new(),
                prepared_sidecars: Vec::new(),
                metadata: publication_metadata_from_persisted_value(facts),
                policy_relevance: PublicationPolicyRelevance::unknown(),
            },
        )
    }

    pub(crate) fn commit_import_snapshot_with_policy_relevance(
        &mut self,
        installed: InstalledImport<'_>,
        reading_revision: &str,
        title: &str,
        products: PolicyAwareReadingProducts<'_>,
    ) -> Result<ImportOutcome, String> {
        let PolicyAwareReadingProducts {
            facts,
            package,
            responses,
            media_overlay,
            media_overlay_timelines,
            prepared_sidecars,
            policy_relevance,
        } = products;
        validate_stored_reading_products(facts, responses)?;
        crate::media_overlay::validate_media_overlay_description(media_overlay)
            .map_err(|error| format!("derived media overlay description is invalid: {error}"))?;
        for timeline in media_overlay_timelines {
            crate::media_overlay::validate_timeline_for_description(timeline, media_overlay)
                .map_err(|error| format!("derived media overlay timeline is invalid: {error}"))?;
        }
        let facts_json = serde_json::to_string(facts)
            .map_err(|error| format!("could not serialize publication facts: {error}"))?;
        let responses_json = serde_json::to_string(responses)
            .map_err(|error| format!("could not serialize response manifest: {error}"))?;
        let media_overlay_json = serde_json::to_string(media_overlay)
            .map_err(|error| format!("could not serialize media overlay description: {error}"))?;
        // The capability depends on per-segment playability, so it is derived once here and
        // stored; opening a publication then needs no timeline at all.
        let media_overlay_capability_json =
            serde_json::to_string(&crate::media_overlay::derive_media_overlay_capability(
                media_overlay,
                media_overlay_timelines,
            ))
            .map_err(|error| format!("could not serialize media overlay capability: {error}"))?;
        let prepared_timelines = prepare_media_overlay_timelines(media_overlay_timelines)?;
        let metadata = publication_metadata(package);
        self.commit_import_snapshot_products(
            installed,
            reading_revision,
            title,
            PreparedSnapshotCommit {
                facts_json,
                responses_json,
                media_overlay_json,
                media_overlay_capability_json,
                media_overlay_timelines: prepared_timelines,
                prepared_sidecars,
                metadata,
                policy_relevance,
            },
        )
    }

    pub fn snapshot_is_current(&self, snapshot_id: &str) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT 1
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE s.snapshot_id = ?1 AND p.deletion_state = 'live' AND s.revoked_at IS NULL",
                [snapshot_id],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(|error| format!("could not validate the publication snapshot: {error}"))
    }

    pub fn live_snapshot_ids(&self) -> Result<HashSet<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT s.snapshot_id
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.deletion_state = 'live' AND s.revoked_at IS NULL",
            )
            .map_err(|error| format!("could not list live publication snapshots: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not list live publication snapshots: {error}"))?;
        rows.collect::<Result<HashSet<_>, _>>()
            .map_err(|error| format!("could not list live publication snapshots: {error}"))
    }

    pub(crate) fn commit_import_snapshot_products(
        &mut self,
        installed: InstalledImport<'_>,
        reading_revision: &str,
        title: &str,
        products: PreparedSnapshotCommit,
    ) -> Result<ImportOutcome, String> {
        let PreparedSnapshotCommit {
            facts_json,
            responses_json,
            media_overlay_json,
            media_overlay_capability_json,
            media_overlay_timelines,
            prepared_sidecars,
            metadata,
            policy_relevance,
        } = products;
        let media_sidecars_dir = self.media_sidecars.clone();
        let media_timelines_dir = self.media_timelines.clone();
        let creators_json = metadata.creators_json()?;
        let creator_directions_json = metadata.creator_directions_json()?;
        let InstalledImport {
            archive: blob,
            cover,
        } = installed;
        let reservation_id = blob.reservation_id.as_deref();
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| format!("could not begin import transaction: {error}"))?;
        if let Some(reservation_id) = reservation_id {
            let reserved_bytes = transaction
                .query_row(
                    "SELECT reserved_bytes FROM import_reservations WHERE reservation_id = ?1",
                    [reservation_id],
                    |row| row.get::<_, u64>(0),
                )
                .optional()
                .map_err(|error| format!("could not validate import reservation: {error}"))?
                .ok_or_else(|| "import reservation is unavailable".to_owned())?;
            if blob.size > reserved_bytes {
                return Err("import exceeds its reserved archive bytes".to_owned());
            }
        }
        if let Some(expected_publication_id) = &blob.expected_publication_id {
            let still_current = transaction
                .query_row(
                    "SELECT 1 FROM publications
                     WHERE publication_id = ?1 AND imported_blob_hash = ?2
                       AND deletion_state = 'live'",
                    params![expected_publication_id, blob.hash],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("could not validate prepared publication: {error}"))?
                .is_some();
            if !still_current {
                return Err("publication changed while preparation was running".to_owned());
            }
        }
        let stored_blob = transaction
            .query_row(
                "SELECT storage_name, size_bytes, lifecycle_state FROM archive_blobs
                 WHERE blob_hash = ?1",
                [&blob.hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not validate imported blob state: {error}"))?;
        if stored_blob.as_ref().is_some_and(|(name, size, lifecycle)| {
            name != &blob.hash || *size != blob.size || lifecycle != "live"
        }) {
            return Err("matching archive blob is not available for import".to_owned());
        }
        transaction
            .execute(
                "INSERT INTO archive_blobs
                    (blob_hash, storage_name, size_bytes, health_state, lifecycle_state)
                 VALUES (?1, ?1, ?2, 'available', 'live')
                  ON CONFLICT(blob_hash) DO UPDATE SET
                     size_bytes = excluded.size_bytes,
                     health_state = 'available'",
                params![blob.hash, blob.size],
            )
            .map_err(|error| format!("could not record imported blob: {error}"))?;
        transaction
            .execute(
                "DELETE FROM blob_cleanup_operations WHERE blob_hash = ?1",
                [&blob.hash],
            )
            .map_err(|error| format!("could not cancel stale archive cleanup: {error}"))?;

        let existing = transaction
            .query_row(
                "SELECT p.publication_id, p.title_summary, p.creators_summary,
                         p.language_summary, p.reading_direction_summary, p.imported_at,
                        p.deletion_state, r.updated_at, s.snapshot_id, s.reading_revision,
                        s.cover_hash, s.cover_size_bytes, s.cover_mime,
                        p.title_direction_summary, p.creator_directions_summary
                 FROM publications p
                 LEFT JOIN reading_positions r ON r.publication_id = p.publication_id
                 LEFT JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.imported_blob_hash = ?1",
                [&blob.hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<String>>(10)?,
                        row.get::<_, Option<u64>>(11)?,
                        row.get::<_, Option<String>>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, String>(14)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not query imported publication: {error}"))?;
        if let Some((
            publication_id,
            stored_title,
            stored_creators_json,
            stored_language,
            stored_reading_direction,
            imported_at,
            state,
            last_read_at,
            snapshot_id,
            stored_reading_revision,
            stored_cover_hash,
            stored_cover_size,
            stored_cover_mime,
            stored_title_direction,
            stored_creator_directions_json,
        )) = existing
        {
            if state != "live" {
                return Err("matching publication is being deleted".to_owned());
            }
            active_browser_origin_id(&transaction, &publication_id)?;
            if stored_reading_revision.as_deref() != Some(reading_revision) {
                let replacement_snapshot_id = random_hex(16)?;
                transaction
                    .execute(
                        "INSERT INTO publication_snapshots
                            (snapshot_id, publication_id, blob_hash, reading_revision,
                             facts_json, responses_json, media_overlay_json,
                             media_overlay_capability_json,
                             active_content_relevance,
                             remote_resources_relevance, cover_hash, cover_size_bytes, cover_mime)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            replacement_snapshot_id,
                            publication_id,
                            blob.hash,
                            reading_revision,
                            facts_json,
                            responses_json,
                            media_overlay_json,
                            media_overlay_capability_json,
                            policy_relevance.active_content.as_str(),
                            policy_relevance.remote_resources.as_str(),
                            cover.map(|cover| cover.hash.as_str()),
                            cover.map(|cover| cover.size),
                            cover.map(|cover| cover.mime.as_str()),
                        ],
                    )
                    .map_err(|error| format!("could not record replacement snapshot: {error}"))?;
                install_media_sidecars_in_transaction(
                    &transaction,
                    &media_sidecars_dir,
                    &replacement_snapshot_id,
                    prepared_sidecars,
                )?;
                install_media_overlay_timelines_in_transaction(
                    &transaction,
                    &media_timelines_dir,
                    &replacement_snapshot_id,
                    media_overlay_timelines,
                )?;
                transaction
                    .execute(
                        "UPDATE publications
                         SET current_snapshot_id = ?2, title_summary = ?3,
                             creators_summary = ?4, language_summary = ?5,
                             reading_direction_summary = ?6,
                             title_direction_summary = ?7,
                             creator_directions_summary = ?8
                         WHERE publication_id = ?1",
                        params![
                            publication_id,
                            replacement_snapshot_id,
                            title,
                            creators_json,
                            metadata.language,
                            metadata.reading_direction,
                            metadata.title_direction,
                            creator_directions_json,
                        ],
                    )
                    .map_err(|error| format!("could not select replacement snapshot: {error}"))?;
                consume_import_reservation(&transaction, reservation_id)?;
                transaction.commit().map_err(|error| {
                    format!("could not commit replacement publication snapshot: {error}")
                })?;
                return Ok(ImportOutcome {
                    created: false,
                    snapshot_id: Some(replacement_snapshot_id),
                    blob_path: installed.archive.path.clone(),
                    publication: PublicationSummary {
                        publication_id,
                        title: title.to_owned(),
                        title_direction: metadata.title_direction.clone(),
                        creators: metadata.creators.clone(),
                        creator_directions: metadata.creator_directions.clone(),
                        language: metadata.language.clone(),
                        reading_direction: metadata.reading_direction.clone(),
                        imported_at,
                        last_read_at,
                        has_cover: cover.is_some(),
                    },
                });
            }
            let stored_cover = match (stored_cover_hash, stored_cover_size, stored_cover_mime) {
                (Some(hash), Some(size), Some(mime)) => Some((hash, size, mime)),
                (None, None, None) => None,
                _ => return Err("stored cover descriptor is incomplete".to_owned()),
            };
            match (stored_cover.as_ref(), cover) {
                (Some((hash, size, mime)), Some(cover))
                    if hash != &cover.hash || *size != cover.size || mime != &cover.mime =>
                {
                    return Err("matching publication has different cover metadata".to_owned());
                }
                (None, Some(cover)) => {
                    let snapshot_id = snapshot_id
                        .as_deref()
                        .ok_or_else(|| "matching publication has no current snapshot".to_owned())?;
                    transaction
                        .execute(
                            "UPDATE publication_snapshots
                             SET cover_hash = ?2, cover_size_bytes = ?3, cover_mime = ?4
                             WHERE snapshot_id = ?1",
                            params![snapshot_id, cover.hash, cover.size, cover.mime],
                        )
                        .map_err(|error| {
                            format!("could not backfill publication cover: {error}")
                        })?;
                }
                _ => {}
            }
            // A duplicate import re-derives when stored sidecars are missing, so install what it
            // produced; otherwise every open repeats the derivation and still finds none.
            if let Some(snapshot_id) = snapshot_id.as_deref() {
                install_media_sidecars_in_transaction(
                    &transaction,
                    &media_sidecars_dir,
                    snapshot_id,
                    prepared_sidecars,
                )?;
                install_media_overlay_timelines_in_transaction(
                    &transaction,
                    &media_timelines_dir,
                    snapshot_id,
                    media_overlay_timelines,
                )?;
            }
            consume_import_reservation(&transaction, reservation_id)?;
            transaction
                .commit()
                .map_err(|error| format!("could not commit duplicate import: {error}"))?;
            return Ok(ImportOutcome {
                created: false,
                snapshot_id,
                blob_path: installed.archive.path.clone(),
                publication: PublicationSummary {
                    publication_id,
                    title: stored_title,
                    title_direction: stored_title_direction,
                    creators: decode_creators_summary(&stored_creators_json)?,
                    creator_directions: decode_creator_directions_summary(
                        &stored_creator_directions_json,
                    )?,
                    language: stored_language,
                    reading_direction: stored_reading_direction,
                    imported_at,
                    last_read_at,
                    has_cover: stored_cover.is_some() || cover.is_some(),
                },
            });
        }

        let publication_id = random_hex(16)?;
        let snapshot_id = random_hex(16)?;
        transaction
            .execute(
                "INSERT INTO publications
                    (publication_id, imported_blob_hash, title_summary,
                     creators_summary, language_summary, reading_direction_summary,
                     title_direction_summary, creator_directions_summary, deletion_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'live')",
                params![
                    publication_id,
                    blob.hash,
                    title,
                    creators_json,
                    metadata.language,
                    metadata.reading_direction,
                    metadata.title_direction,
                    creator_directions_json,
                ],
            )
            .map_err(|error| format!("could not record imported publication: {error}"))?;
        allocate_browser_origin(&transaction, &publication_id)?;
        transaction
            .execute(
                "INSERT INTO publication_snapshots
                    (snapshot_id, publication_id, blob_hash, reading_revision, facts_json,
                      responses_json, media_overlay_json, media_overlay_capability_json,
                      active_content_relevance,
                      remote_resources_relevance,
                      cover_hash, cover_size_bytes, cover_mime)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    snapshot_id,
                    publication_id,
                    blob.hash,
                    reading_revision,
                    facts_json,
                    responses_json,
                    media_overlay_json,
                    media_overlay_capability_json,
                    policy_relevance.active_content.as_str(),
                    policy_relevance.remote_resources.as_str(),
                    cover.map(|cover| cover.hash.as_str()),
                    cover.map(|cover| cover.size),
                    cover.map(|cover| cover.mime.as_str()),
                ],
            )
            .map_err(|error| format!("could not record imported snapshot: {error}"))?;
        install_media_sidecars_in_transaction(
            &transaction,
            &media_sidecars_dir,
            &snapshot_id,
            prepared_sidecars,
        )?;
        install_media_overlay_timelines_in_transaction(
            &transaction,
            &media_timelines_dir,
            &snapshot_id,
            media_overlay_timelines,
        )?;
        transaction
            .execute(
                "UPDATE publications SET current_snapshot_id = ?2 WHERE publication_id = ?1",
                params![publication_id, snapshot_id],
            )
            .map_err(|error| format!("could not select imported snapshot: {error}"))?;
        let imported_at = transaction
            .query_row(
                "SELECT imported_at FROM publications WHERE publication_id = ?1",
                [&publication_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| format!("could not read imported publication: {error}"))?;
        consume_import_reservation(&transaction, reservation_id)?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit imported publication: {error}"))?;
        Ok(ImportOutcome {
            created: true,
            snapshot_id: Some(snapshot_id),
            blob_path: installed.archive.path.clone(),
            publication: PublicationSummary {
                publication_id,
                title: title.to_owned(),
                title_direction: metadata.title_direction,
                creators: metadata.creators,
                creator_directions: metadata.creator_directions,
                language: metadata.language,
                reading_direction: metadata.reading_direction,
                imported_at,
                last_read_at: None,
                has_cover: cover.is_some(),
            },
        })
    }
}

pub(crate) struct InstalledImport<'a> {
    pub archive: &'a InstalledBlob,
    pub cover: Option<&'a InstalledCover>,
}

pub(crate) struct PolicyAwareReadingProducts<'a> {
    pub facts: &'a Value,
    pub package: &'a Package,
    pub responses: &'a ResponseManifest,
    pub media_overlay: &'a MediaOverlayDescription,
    pub media_overlay_timelines: &'a [crate::media_overlay::MediaOverlayTimeline],
    pub prepared_sidecars: Vec<crate::media_overlay::backing::PreparedSidecar>,
    pub policy_relevance: PublicationPolicyRelevance,
}

pub(crate) struct ImportOutcome {
    pub created: bool,
    pub publication: PublicationSummary,
    pub snapshot_id: Option<String>,
    pub blob_path: PathBuf,
}

pub(crate) struct PreparedSnapshotCommit {
    pub(crate) facts_json: String,
    pub(crate) responses_json: String,
    pub(crate) media_overlay_json: String,
    pub(crate) media_overlay_capability_json: String,
    pub(crate) media_overlay_timelines: Vec<PreparedTimeline>,
    pub(crate) prepared_sidecars: Vec<crate::media_overlay::backing::PreparedSidecar>,
    pub(crate) metadata: PublicationMetadata,
    pub(crate) policy_relevance: PublicationPolicyRelevance,
}
