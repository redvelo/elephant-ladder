//! Content-addressed archive storage.
//!
//! Import reserves capacity first, streams into an unpredictable staging file,
//! and installs a verified blob atomically. A committed blob is reused only
//! after its size and full digest are verified, and blob deduplication is global
//! and distinct from library-item duplicate handling.

use super::{IMPORT_RESERVATION_LIFETIME, Storage, is_epub_core_image_mime, random_hex};
use crate::{
    storage::fs, storage::is_blake3_hash, storage::remove_file_if_present, storage::sync_directory,
    storage::verify_blob,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
#[cfg(test)]
use std::{fs::OpenOptions, io::Write};
use std::{path::Path, path::PathBuf};

impl Storage {
    /// The bytes of every book this library committed, which is the figure the import quota
    /// is kept against and the one a person is shown.
    pub(crate) fn committed_archive_bytes(&self) -> Result<u64, String> {
        let bytes: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM archive_blobs \
                 WHERE lifecycle_state = 'live'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("could not read archive usage: {error}"))?;
        Ok(u64::try_from(bytes).unwrap_or(0))
    }

    pub(crate) fn reserve_import(
        &mut self,
        reserved_bytes: u64,
    ) -> Result<ReserveImportOutcome, String> {
        if reserved_bytes == 0 || reserved_bytes > i64::MAX as u64 {
            return Err("invalid import reservation size".to_owned());
        }
        self.release_expired_import_reservations()?;
        let reservation_id = random_hex(16)?;
        let staging_name = format!("{reservation_id}.part");
        let unmaterialized_reservation_bytes = self.unmaterialized_reservation_bytes()?;
        let required_free_bytes = unmaterialized_reservation_bytes
            .checked_add(reserved_bytes)
            .ok_or_else(|| "archive storage capacity overflow".to_owned())?;
        let available_bytes = fs2::available_space(&self.staging)
            .map_err(|error| format!("could not inspect available archive storage: {error}"))?;
        if required_free_bytes > available_bytes {
            return Ok(ReserveImportOutcome::QuotaExceeded);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin import reservation: {error}"))?;
        let (blob_bytes, reservation_bytes) = transaction
            .query_row(
                "SELECT
                    COALESCE((SELECT SUM(size_bytes) FROM archive_blobs
                              WHERE lifecycle_state = 'live'), 0),
                    COALESCE((SELECT SUM(reserved_bytes) FROM import_reservations), 0)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| format!("could not account for archive storage: {error}"))?;
        let used_bytes = u64::try_from(blob_bytes)
            .ok()
            .and_then(|blobs| {
                u64::try_from(reservation_bytes)
                    .ok()
                    .and_then(|reservations| blobs.checked_add(reservations))
            })
            .ok_or_else(|| "archive storage accounting is invalid".to_owned())?;
        if let Some(archive_quota_bytes) = self.archive_quota_bytes {
            let required_bytes = used_bytes
                .checked_add(reserved_bytes)
                .ok_or_else(|| "archive storage accounting overflow".to_owned())?;
            if required_bytes > archive_quota_bytes {
                return Ok(ReserveImportOutcome::QuotaExceeded);
            }
        }
        transaction
            .execute(
                "INSERT INTO import_reservations
                    (reservation_id, staging_name, reserved_bytes, expires_at)
                 VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?4))",
                params![
                    reservation_id,
                    staging_name,
                    reserved_bytes,
                    IMPORT_RESERVATION_LIFETIME
                ],
            )
            .map_err(|error| format!("could not record import reservation: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit import reservation: {error}"))?;
        Ok(ReserveImportOutcome::Reserved(ImportReservation {
            id: reservation_id,
            staging_path: self.staging.join(staging_name),
        }))
    }

    pub(crate) fn unmaterialized_reservation_bytes(&self) -> Result<u64, String> {
        let reservations = {
            let mut statement = self
                .connection
                .prepare("SELECT staging_name, reserved_bytes FROM import_reservations")
                .map_err(|error| format!("could not prepare staging accounting: {error}"))?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
                })
                .map_err(|error| format!("could not query staging accounting: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("could not read staging accounting: {error}"))?
        };
        reservations
            .into_iter()
            .try_fold(0_u64, |total, (staging_name, reserved_bytes)| {
                let materialized = match fs::metadata(self.staging.join(staging_name)) {
                    Ok(metadata) if metadata.is_file() => metadata.len().min(reserved_bytes),
                    Ok(_) => 0,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                    Err(error) => {
                        return Err(format!("could not inspect reserved staging file: {error}"));
                    }
                };
                total
                    .checked_add(reserved_bytes - materialized)
                    .ok_or_else(|| "staging reservation accounting overflow".to_owned())
            })
    }

    pub fn cancel_import_reservation(
        &mut self,
        reservation_id: &str,
        candidate_blob_hash: Option<&str>,
    ) -> Result<(), String> {
        let reservation = self
            .connection
            .query_row(
                "SELECT staging_name, blob_hash FROM import_reservations
                 WHERE reservation_id = ?1",
                [reservation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(|error| format!("could not load import reservation: {error}"))?;
        if let Some((staging_name, _)) = &reservation {
            remove_file_if_present(&self.staging.join(staging_name))?;
            sync_directory(&self.staging)?;
        }
        let reserved_blob_hash = reservation.and_then(|(_, hash)| hash);
        if let Some(hash) = reserved_blob_hash
            .as_deref()
            .or(candidate_blob_hash)
            .filter(|hash| is_blake3_hash(hash))
        {
            let recorded = self
                .connection
                .query_row(
                    "SELECT 1 FROM archive_blobs WHERE blob_hash = ?1",
                    [hash],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("could not inspect cancelled import blob: {error}"))?
                .is_some();
            if !recorded {
                remove_file_if_present(&self.blobs.join(hash))?;
                sync_directory(&self.blobs)?;
            }
        }
        self.connection
            .execute(
                "DELETE FROM import_reservations WHERE reservation_id = ?1",
                [reservation_id],
            )
            .map_err(|error| format!("could not release import reservation: {error}"))?;
        Ok(())
    }

    pub(crate) fn release_expired_import_reservations(&mut self) -> Result<(), String> {
        let expired = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT reservation_id FROM import_reservations
                     WHERE expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                )
                .map_err(|error| format!("could not prepare expired reservations: {error}"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| format!("could not query expired reservations: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("could not read expired reservations: {error}"))?
        };
        for reservation_id in expired {
            self.cancel_import_reservation(&reservation_id, None)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn install_blob(&self, bytes: &[u8]) -> Result<InstalledBlob, String> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let size = u64::try_from(bytes.len()).map_err(|_| "archive size overflow".to_owned())?;
        let path = self.blobs.join(&hash);

        if path.exists() {
            verify_blob(&path, &hash, size)?;
        } else {
            let staging_name = format!("{}.part", random_hex(16)?);
            let staging_path = self.staging.join(staging_name);
            let result: Result<(), String> = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&staging_path)
                    .map_err(|error| format!("could not create archive staging file: {error}"))?;
                file.write_all(bytes)
                    .map_err(|error| format!("could not write archive staging file: {error}"))?;
                file.sync_all()
                    .map_err(|error| format!("could not sync archive staging file: {error}"))?;
                fs::rename(&staging_path, &path)
                    .map_err(|error| format!("could not commit archive blob: {error}"))?;
                sync_directory(&self.blobs)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&staging_path);
            }
            result?;
        }

        Ok(InstalledBlob {
            hash,
            path,
            size,
            reservation_id: None,
            expected_publication_id: None,
        })
    }

    pub fn staging_path(&self) -> &Path {
        &self.staging
    }

    pub(crate) fn install_staged_blob(
        &self,
        reservation_id: &str,
        hash: &str,
        size: u64,
    ) -> Result<InstalledBlob, String> {
        if !is_blake3_hash(hash) {
            return Err("invalid staged archive hash".to_owned());
        }
        let (staging_name, reserved_bytes) = self
            .connection
            .query_row(
                "SELECT staging_name, reserved_bytes FROM import_reservations
                 WHERE reservation_id = ?1",
                [reservation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .optional()
            .map_err(|error| format!("could not load import reservation: {error}"))?
            .ok_or_else(|| "import reservation is unavailable".to_owned())?;
        if staging_name != format!("{reservation_id}.part") || size > reserved_bytes {
            return Err("staged archive does not match its reservation".to_owned());
        }
        let staging_path = self.staging.join(staging_name);
        let metadata = staging_path
            .metadata()
            .map_err(|error| format!("could not inspect staged archive: {error}"))?;
        if !metadata.is_file() || metadata.len() != size {
            return Err("staged archive has the wrong size or type".to_owned());
        }
        verify_blob(&staging_path, hash, size)?;
        let existing_lifecycle = self
            .connection
            .query_row(
                "SELECT lifecycle_state FROM archive_blobs WHERE blob_hash = ?1",
                [hash],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| format!("could not inspect staged archive lifecycle: {error}"))?;
        if existing_lifecycle
            .as_deref()
            .is_some_and(|lifecycle| lifecycle != "live")
        {
            return Err("matching archive blob cleanup is in progress".to_owned());
        }
        self.connection
            .execute(
                "UPDATE import_reservations SET blob_hash = ?2 WHERE reservation_id = ?1",
                params![reservation_id, hash],
            )
            .map_err(|error| format!("could not bind blob to import reservation: {error}"))?;
        let path = self.blobs.join(hash);
        if path.exists() {
            verify_blob(&path, hash, size)?;
            fs::remove_file(&staging_path)
                .map_err(|error| format!("could not remove duplicate staging file: {error}"))?;
        } else {
            fs::rename(&staging_path, &path)
                .map_err(|error| format!("could not commit archive blob: {error}"))?;
            sync_directory(&self.blobs)?;
        }
        sync_directory(&self.staging)?;
        Ok(InstalledBlob {
            hash: hash.to_owned(),
            path,
            size,
            reservation_id: Some(reservation_id.to_owned()),
            expected_publication_id: None,
        })
    }

    pub(crate) fn install_staged_cover(
        &self,
        staging_path: &Path,
        hash: &str,
        size: u64,
        mime: &str,
    ) -> Result<InstalledCover, String> {
        if !is_blake3_hash(hash) || !is_epub_core_image_mime(mime) {
            return Err("invalid staged cover metadata".to_owned());
        }
        verify_blob(staging_path, hash, size)?;
        let path = self.covers.join(hash);
        if path.exists() {
            verify_blob(&path, hash, size)?;
            fs::remove_file(staging_path).map_err(|error| {
                format!("could not remove duplicate cover staging file: {error}")
            })?;
        } else {
            fs::rename(staging_path, &path)
                .map_err(|error| format!("could not commit cover artifact: {error}"))?;
            sync_directory(&self.covers)?;
        }
        Ok(InstalledCover {
            hash: hash.to_owned(),
            size,
            mime: mime.to_owned(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct ImportReservation {
    pub id: String,
    pub staging_path: PathBuf,
}

#[derive(Debug)]
pub(crate) enum ReserveImportOutcome {
    Reserved(ImportReservation),
    QuotaExceeded,
}

pub(crate) struct InstalledBlob {
    pub hash: String,
    pub path: PathBuf,
    pub size: u64,
    pub(crate) reservation_id: Option<String>,
    pub(crate) expected_publication_id: Option<String>,
}

pub(crate) struct InstalledCover {
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

pub(crate) struct StoredCover {
    pub path: PathBuf,
    pub size: u64,
    pub mime: String,
}

pub(crate) fn validate_sha256_hash(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("pagination cache hash must be lowercase 64-character SHA-256 hex".to_owned())
    }
}
