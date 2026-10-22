//! Resumable publication deletion.
//!
//! Deletion marks the publication, blocks new opens and duplicate reuse, removes
//! publication-owned state in bounded phases, detaches snapshots and blob
//! references transactionally, then moves unreferenced archives through an
//! fsynced trash phase. A missing file cannot wedge the sequence, and startup
//! resumes an interrupted phase idempotently.

use super::Storage;
use crate::{
    storage::active_browser_origin_id, storage::allocate_browser_origin, storage::fs,
    storage::is_blake3_hash, storage::remove_file_if_present, storage::remove_unreferenced_covers,
    storage::remove_unreferenced_media_sidecars, storage::sync_directory, storage::verify_blob,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl Storage {
    pub(crate) fn begin_publication_deletion(
        &mut self,
        publication_id: &str,
    ) -> Result<BeginPublicationDeletionOutcome, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin publication deletion: {error}"))?;
        let deletion_state = transaction
            .query_row(
                "SELECT deletion_state FROM publications WHERE publication_id = ?1",
                [publication_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("could not select publication for deletion: {error}"))?;
        let Some(deletion_state) = deletion_state else {
            return Ok(BeginPublicationDeletionOutcome::NotFound);
        };
        if !matches!(
            deletion_state.as_str(),
            "live" | "deleting" | "delete_failed"
        ) {
            return Err("publication deletion state is invalid".to_owned());
        }
        let snapshot_ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT snapshot_id FROM publication_snapshots
                     WHERE publication_id = ?1 ORDER BY snapshot_id",
                )
                .map_err(|error| format!("could not prepare publication snapshots: {error}"))?;
            statement
                .query_map([publication_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("could not query publication snapshots: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("could not read publication snapshots: {error}"))?
        };
        if deletion_state == "live" {
            let retired = transaction
                .execute(
                    "UPDATE browser_origin_allocations
                     SET lifecycle_state = 'purge_pending', retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE publication_id = ?1 AND lifecycle_state = 'active'",
                    [publication_id],
                )
                .map_err(|error| format!("could not retire publication browser origin: {error}"))?;
            if retired != 1 {
                return Err("publication has no active browser origin".to_owned());
            }
        }
        transaction
            .execute(
                "UPDATE publications SET deletion_state = 'deleting' WHERE publication_id = ?1",
                [publication_id],
            )
            .map_err(|error| format!("could not mark publication for deletion: {error}"))?;
        transaction
            .execute(
                "INSERT INTO deletion_operations (publication_id, phase)
                 VALUES (?1, 'marked')
                 ON CONFLICT(publication_id) DO UPDATE SET
                    last_error = NULL, next_retry_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                [publication_id],
            )
            .map_err(|error| format!("could not record publication deletion: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication deletion mark: {error}"))?;
        Ok(BeginPublicationDeletionOutcome::Started { snapshot_ids })
    }

    pub(crate) fn reset_publication_website_data(
        &mut self,
        publication_id: &str,
    ) -> Result<ResetPublicationWebsiteDataOutcome, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin publication website-data reset: {error}"))?;
        let live = transaction
            .query_row(
                "SELECT 1 FROM publications
                 WHERE publication_id = ?1 AND deletion_state = 'live'",
                [publication_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| {
                format!("could not select publication for website-data reset: {error}")
            })?
            .is_some();
        if !live {
            return Ok(ResetPublicationWebsiteDataOutcome::NotFound);
        }
        let retired_origin_id = active_browser_origin_id(&transaction, publication_id)?;
        let retired = transaction
            .execute(
                "UPDATE browser_origin_allocations
                 SET lifecycle_state = 'purge_pending', retired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     purge_attempts = 0, last_purge_error = NULL
                 WHERE origin_id = ?1 AND publication_id = ?2 AND lifecycle_state = 'active'",
                params![retired_origin_id, publication_id],
            )
            .map_err(|error| format!("could not retire publication browser origin: {error}"))?;
        if retired != 1 {
            return Err("publication browser origin changed during reset".to_owned());
        }
        allocate_browser_origin(&transaction, publication_id)?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication website-data reset: {error}"))?;
        Ok(ResetPublicationWebsiteDataOutcome::Reset { retired_origin_id })
    }

    pub fn record_browser_origin_purge_attempt(
        &mut self,
        origin_id: &str,
        report_json: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        if report_json.len() > 16 * 1024 || error.is_some_and(|value| value.len() > 1024) {
            return Err("browser-origin purge report is too large".to_owned());
        }
        let updated = self
            .connection
            .execute(
                "UPDATE browser_origin_allocations
                 SET purge_attempts = purge_attempts + 1,
                     last_purge_report_json = ?2,
                     last_purge_error = ?3
                 WHERE origin_id = ?1 AND lifecycle_state = 'purge_pending'",
                params![origin_id, report_json, error],
            )
            .map_err(|cause| format!("could not record browser-origin purge attempt: {cause}"))?;
        if updated != 1 {
            return Err("browser origin is not pending purge".to_owned());
        }
        Ok(())
    }

    pub fn complete_publication_deletion(&mut self, publication_id: &str) -> Result<(), String> {
        let blob_hashes = self.detach_publication(publication_id)?;
        for blob_hash in blob_hashes {
            self.complete_blob_cleanup(&blob_hash)?;
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not finalize publication deletion: {error}"))?;
        transaction
            .execute(
                "DELETE FROM publications
                 WHERE publication_id = ?1 AND deletion_state IN ('deleting', 'delete_failed')",
                [publication_id],
            )
            .map_err(|error| format!("could not remove deleted publication: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication deletion: {error}"))?;
        remove_unreferenced_covers(&self.connection, &self.covers)?;
        remove_unreferenced_media_sidecars(&self.connection, &self.media_sidecars)?;
        Ok(())
    }

    pub fn maintain_deletions(&mut self) -> Result<(), String> {
        self.resume_pending_deletions()
    }

    pub fn cleanup_unreferenced_covers(&self) -> Result<(), String> {
        remove_unreferenced_covers(&self.connection, &self.covers)
    }

    pub(crate) fn detach_publication(
        &mut self,
        publication_id: &str,
    ) -> Result<Vec<String>, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin publication detach: {error}"))?;
        let operation = transaction
            .query_row(
                "SELECT phase FROM deletion_operations WHERE publication_id = ?1",
                [publication_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("could not load publication deletion: {error}"))?;
        let Some(phase) = operation else {
            return Ok(Vec::new());
        };
        if phase == "detached" {
            let hash = transaction
                .query_row(
                    "SELECT detached_imported_blob_hash FROM deletion_operations
                     WHERE publication_id = ?1",
                    [publication_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(|error| format!("could not load detached publication blob: {error}"))?;
            let hashes = match hash {
                Some(hash)
                    if transaction
                        .query_row(
                            "SELECT 1 FROM blob_cleanup_operations WHERE blob_hash = ?1",
                            [&hash],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(|error| {
                            format!("could not inspect detached blob cleanup: {error}")
                        })?
                        .is_some() =>
                {
                    vec![hash]
                }
                _ => Vec::new(),
            };
            transaction
                .commit()
                .map_err(|error| format!("could not close publication detach: {error}"))?;
            return Ok(hashes);
        }
        if phase != "marked" {
            return Err("publication deletion phase is invalid".to_owned());
        }
        let imported_blob_hash = transaction
            .query_row(
                "SELECT imported_blob_hash FROM publications WHERE publication_id = ?1",
                [publication_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| format!("could not load publication source: {error}"))?
            .flatten();
        let mut blob_hashes = {
            let mut statement = transaction
                .prepare(
                    "SELECT DISTINCT blob_hash FROM publication_snapshots
                     WHERE publication_id = ?1",
                )
                .map_err(|error| format!("could not prepare publication blobs: {error}"))?;
            statement
                .query_map([publication_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("could not query publication blobs: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("could not read publication blobs: {error}"))?
        };
        if let Some(hash) = &imported_blob_hash
            && !blob_hashes.contains(hash)
        {
            blob_hashes.push(hash.clone());
        }
        transaction
            .execute(
                "UPDATE publications
                 SET current_snapshot_id = NULL, imported_blob_hash = NULL
                 WHERE publication_id = ?1",
                [publication_id],
            )
            .map_err(|error| format!("could not detach publication pointers: {error}"))?;
        transaction
            .execute(
                "DELETE FROM publication_snapshots WHERE publication_id = ?1",
                [publication_id],
            )
            .map_err(|error| format!("could not remove publication snapshots: {error}"))?;
        let mut cleanup_hashes = Vec::new();
        for blob_hash in blob_hashes {
            let references = transaction
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM publications WHERE imported_blob_hash = ?1) +
                        (SELECT COUNT(*) FROM publication_snapshots WHERE blob_hash = ?1)",
                    [&blob_hash],
                    |row| row.get::<_, u64>(0),
                )
                .map_err(|error| format!("could not count archive blob references: {error}"))?;
            if references == 0 {
                transaction
                    .execute(
                        "UPDATE archive_blobs SET lifecycle_state = 'gc_pending'
                         WHERE blob_hash = ?1 AND lifecycle_state = 'live'",
                        [&blob_hash],
                    )
                    .map_err(|error| format!("could not mark archive blob for cleanup: {error}"))?;
                transaction
                    .execute(
                        "INSERT INTO blob_cleanup_operations (blob_hash, phase)
                         VALUES (?1, 'pending') ON CONFLICT(blob_hash) DO NOTHING",
                        [&blob_hash],
                    )
                    .map_err(|error| format!("could not record archive blob cleanup: {error}"))?;
                cleanup_hashes.push(blob_hash);
            }
        }
        transaction
            .execute(
                "UPDATE deletion_operations
                 SET phase = 'detached', detached_imported_blob_hash = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE publication_id = ?1",
                params![publication_id, imported_blob_hash],
            )
            .map_err(|error| format!("could not record publication detach: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit publication detach: {error}"))?;
        // The pagination cache is a separate file, so no foreign key follows the
        // snapshots that just went. Collecting here keeps the space back promptly,
        // but it runs after the library change has already committed: failing the
        // detach over disposable cache rows would report a successful deletion as
        // failed and schedule a retry for work that is done. Startup collects
        // whatever is left.
        if let Err(error) = self.retain_pagination_snapshots() {
            tracing::warn!(
                event = "pagination_cache_retain_failed",
                reason = %error,
                "cached pages outlived their snapshots and will be collected at startup"
            );
        }
        Ok(cleanup_hashes)
    }

    pub(crate) fn complete_blob_cleanup(&mut self, blob_hash: &str) -> Result<(), String> {
        let blob = self
            .connection
            .query_row(
                "SELECT b.storage_name, b.size_bytes, b.lifecycle_state, o.phase
                 FROM archive_blobs b
                 JOIN blob_cleanup_operations o ON o.blob_hash = b.blob_hash
                 WHERE b.blob_hash = ?1",
                [blob_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not load archive blob cleanup: {error}"))?;
        let Some((storage_name, size, lifecycle, phase)) = blob else {
            return Ok(());
        };
        if storage_name != blob_hash || !is_blake3_hash(blob_hash) {
            return Err("archive blob cleanup metadata is invalid".to_owned());
        }
        let source = self.blobs.join(&storage_name);
        let trashed = self.trash.join(&storage_name);
        let references = self
            .connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM publications WHERE imported_blob_hash = ?1) +
                    (SELECT COUNT(*) FROM publication_snapshots WHERE blob_hash = ?1)",
                [blob_hash],
                |row| row.get::<_, u64>(0),
            )
            .map_err(|error| format!("could not check archive blob cleanup references: {error}"))?;
        if references != 0 {
            if !source.exists() && trashed.exists() {
                fs::rename(&trashed, &source).map_err(|error| {
                    format!("could not restore referenced archive blob: {error}")
                })?;
                sync_directory(&self.trash)?;
                sync_directory(&self.blobs)?;
            }
            let transaction = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| format!("could not cancel archive blob cleanup: {error}"))?;
            transaction
                .execute(
                    "UPDATE archive_blobs SET lifecycle_state = 'live' WHERE blob_hash = ?1",
                    [blob_hash],
                )
                .map_err(|error| format!("could not restore archive blob lifecycle: {error}"))?;
            transaction
                .execute(
                    "DELETE FROM blob_cleanup_operations WHERE blob_hash = ?1",
                    [blob_hash],
                )
                .map_err(|error| format!("could not cancel archive blob cleanup: {error}"))?;
            transaction
                .commit()
                .map_err(|error| format!("could not commit cancelled blob cleanup: {error}"))?;
            return Ok(());
        }
        if phase == "pending" {
            if source.exists() {
                if trashed.exists() {
                    return Err("archive blob exists in both live and trash storage".to_owned());
                }
                verify_blob(&source, blob_hash, size)?;
                fs::rename(&source, &trashed)
                    .map_err(|error| format!("could not move archive blob to trash: {error}"))?;
                sync_directory(&self.blobs)?;
                sync_directory(&self.trash)?;
            } else if trashed.exists() {
                verify_blob(&trashed, blob_hash, size)?;
            }
            self.connection
                .execute(
                    "UPDATE archive_blobs SET lifecycle_state = 'trashed' WHERE blob_hash = ?1",
                    [blob_hash],
                )
                .map_err(|error| format!("could not mark archive blob trashed: {error}"))?;
            self.connection
                .execute(
                    "UPDATE blob_cleanup_operations
                     SET phase = 'trashed', updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE blob_hash = ?1",
                    [blob_hash],
                )
                .map_err(|error| format!("could not advance archive blob cleanup: {error}"))?;
        } else if phase != "trashed" || lifecycle != "trashed" {
            return Err("archive blob cleanup phase is invalid".to_owned());
        }
        remove_file_if_present(&trashed)?;
        sync_directory(&self.trash)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not finalize archive blob cleanup: {error}"))?;
        let references = transaction
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM publications WHERE imported_blob_hash = ?1) +
                    (SELECT COUNT(*) FROM publication_snapshots WHERE blob_hash = ?1)",
                [blob_hash],
                |row| row.get::<_, u64>(0),
            )
            .map_err(|error| format!("could not recheck archive blob references: {error}"))?;
        if references != 0 {
            return Err("trashed archive blob became referenced".to_owned());
        }
        transaction
            .execute(
                "DELETE FROM blob_cleanup_operations WHERE blob_hash = ?1",
                [blob_hash],
            )
            .map_err(|error| format!("could not remove archive cleanup operation: {error}"))?;
        transaction
            .execute(
                "DELETE FROM archive_blobs WHERE blob_hash = ?1",
                [blob_hash],
            )
            .map_err(|error| format!("could not remove archive blob record: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit archive blob cleanup: {error}"))?;
        Ok(())
    }

    pub(crate) fn resume_pending_deletions(&mut self) -> Result<(), String> {
        let publications = self.pending_deletion_publications()?;
        for publication_id in publications {
            if let Err(error) = self.complete_publication_deletion(&publication_id) {
                self.record_deletion_failure(&publication_id, &error)?;
            }
        }
        let blobs = self.pending_blob_cleanups()?;
        for blob_hash in blobs {
            if let Err(error) = self.complete_blob_cleanup(&blob_hash) {
                self.record_blob_cleanup_failure(&blob_hash, &error)?;
            }
        }
        Ok(())
    }

    pub(crate) fn pending_deletion_publications(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT publication_id FROM deletion_operations
                 WHERE next_retry_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ORDER BY updated_at",
            )
            .map_err(|error| format!("could not prepare pending deletions: {error}"))?;
        let publications = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query pending deletions: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read pending deletions: {error}"))?;
        Ok(publications)
    }

    pub(crate) fn pending_blob_cleanups(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT blob_hash FROM blob_cleanup_operations
                 WHERE next_retry_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ORDER BY updated_at",
            )
            .map_err(|error| format!("could not prepare pending blob cleanups: {error}"))?;
        let blobs = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query pending blob cleanups: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read pending blob cleanups: {error}"))?;
        Ok(blobs)
    }

    /// Records a failed deletion and backs its retry off.
    ///
    /// `attempts` is what sets the next delay — one minute, then doubling to an
    /// hour — so a deletion that can never succeed stops waking the host every
    /// minute forever. The publication is marked `delete_failed` on the first
    /// failure so the state is visible rather than silently retrying.
    pub fn record_deletion_failure(
        &mut self,
        publication_id: &str,
        error: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE deletion_operations SET attempts = attempts + 1, last_error = ?2,
                    next_retry_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now',
                        '+' || min(1 << min(attempts, 6), 64) || ' minutes'),
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE publication_id = ?1",
                params![publication_id, error],
            )
            .map_err(|cause| format!("could not record publication deletion failure: {cause}"))?;
        self.connection
            .execute(
                "UPDATE publications SET deletion_state = 'delete_failed' WHERE publication_id = ?1",
                [publication_id],
            )
            .map_err(|cause| format!("could not mark publication deletion failed: {cause}"))?;
        Ok(())
    }

    pub(crate) fn record_blob_cleanup_failure(
        &mut self,
        blob_hash: &str,
        error: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE blob_cleanup_operations SET attempts = attempts + 1, last_error = ?2,
                    next_retry_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now',
                        '+' || min(1 << min(attempts, 6), 64) || ' minutes'),
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE blob_hash = ?1",
                params![blob_hash, error],
            )
            .map_err(|cause| format!("could not record blob cleanup failure: {cause}"))?;
        Ok(())
    }
}

pub(crate) enum BeginPublicationDeletionOutcome {
    Started { snapshot_ids: Vec<String> },
    NotFound,
}

pub(crate) enum ResetPublicationWebsiteDataOutcome {
    Reset { retired_origin_id: String },
    NotFound,
}
