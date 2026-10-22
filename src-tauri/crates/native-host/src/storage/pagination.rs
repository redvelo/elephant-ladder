//! Pagination-cache persistence.
//!
//! Records are opaque to the host: Navigator owns their format and basis
//! identity. Storage enforces only the per-record, per-snapshot, and
//! process-wide byte bounds, evicting whole least-recently-used bases. Cache
//! corruption, eviction, or quota exhaustion never prevents foreground reading.

use super::{
    MAX_PAGINATION_BASES_PER_SNAPSHOT, MAX_PAGINATION_BULK_BYTES, MAX_PAGINATION_BULK_OPERATIONS,
    MAX_PAGINATION_GLOBAL_BYTES, MAX_PAGINATION_RECORD_BYTES, MAX_PAGINATION_SNAPSHOT_BYTES,
    Storage,
};
use crate::{storage::is_blake3_hash, storage::validate_sha256_hash};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{fs, path::Path};

impl Storage {
    pub(crate) fn read_pagination_records(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
        record_hashes: &[String],
    ) -> Result<PaginationCacheRecordsReadOutcome, String> {
        validate_sha256_hash(basis_hash)?;
        for record_hash in record_hashes {
            validate_sha256_hash(record_hash)?;
        }
        if !pagination_snapshot_is_available(
            &self.connection,
            &self.blobs,
            publication_id,
            snapshot_id,
        )? {
            return Ok(PaginationCacheRecordsReadOutcome::Unavailable);
        }
        let transaction = self
            .pagination
            .transaction()
            .map_err(|error| format!("could not begin pagination cache read: {error}"))?;
        let records = {
            let mut statement = transaction
                .prepare(
                    "SELECT value FROM pagination_records
                 WHERE snapshot_id = ?1 AND basis_hash = ?2 AND record_hash = ?3",
                )
                .map_err(|error| format!("could not prepare pagination cache read: {error}"))?;
            record_hashes
                .iter()
                .map(|record_hash| {
                    statement
                        .query_row(params![snapshot_id, basis_hash, record_hash], |row| {
                            row.get::<_, Vec<u8>>(0)
                        })
                        .optional()
                        .map(|value| {
                            value.filter(|value| value.len() <= MAX_PAGINATION_RECORD_BYTES)
                        })
                        .map_err(|error| format!("could not read pagination cache record: {error}"))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        touch_pagination_basis(&transaction, snapshot_id, basis_hash)?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit pagination cache read: {error}"))?;
        Ok(PaginationCacheRecordsReadOutcome::Records(records))
    }

    pub(crate) fn mutate_pagination_records(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
        mutations: &[PaginationCacheMutation],
    ) -> Result<PaginationCacheMutationOutcome, String> {
        validate_sha256_hash(basis_hash)?;
        if mutations.is_empty() || mutations.len() > MAX_PAGINATION_BULK_OPERATIONS {
            return Err("pagination cache mutation count is invalid".to_owned());
        }
        let mut total_bytes = 0_usize;
        for (index, mutation) in mutations.iter().enumerate() {
            let record_hash = match mutation {
                PaginationCacheMutation::Write { record_hash, value } => {
                    if value.len() > MAX_PAGINATION_RECORD_BYTES {
                        return Err("pagination cache record exceeds 64 KiB".to_owned());
                    }
                    total_bytes = total_bytes
                        .checked_add(value.len())
                        .filter(|total| *total <= MAX_PAGINATION_BULK_BYTES)
                        .ok_or_else(|| "pagination cache mutation data is too large".to_owned())?;
                    record_hash
                }
                PaginationCacheMutation::Remove { record_hash } => record_hash,
            };
            validate_sha256_hash(record_hash)?;
            if mutations[..index].iter().any(|previous| {
                let previous_hash = match previous {
                    PaginationCacheMutation::Write { record_hash, .. }
                    | PaginationCacheMutation::Remove { record_hash } => record_hash,
                };
                previous_hash == record_hash
            }) {
                return Err("pagination cache mutation record hashes must be unique".to_owned());
            }
        }

        if !pagination_snapshot_is_available(
            &self.connection,
            &self.blobs,
            publication_id,
            snapshot_id,
        )? {
            return Ok(PaginationCacheMutationOutcome::Unavailable);
        }
        let transaction = self
            .pagination
            .transaction()
            .map_err(|error| format!("could not begin pagination cache mutation: {error}"))?;

        if mutations
            .iter()
            .any(|mutation| matches!(mutation, PaginationCacheMutation::Write { .. }))
        {
            let access_order = next_pagination_access_order(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO pagination_bases (snapshot_id, basis_hash, access_order)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(snapshot_id, basis_hash) DO UPDATE SET
                        access_order = excluded.access_order",
                    params![snapshot_id, basis_hash, access_order],
                )
                .map_err(|error| format!("could not record pagination cache basis: {error}"))?;
        }
        for mutation in mutations {
            match mutation {
                PaginationCacheMutation::Write { record_hash, value } => {
                    transaction
                        .execute(
                            "INSERT INTO pagination_records
                                (snapshot_id, basis_hash, record_hash, value)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT(snapshot_id, basis_hash, record_hash) DO UPDATE SET
                                value = excluded.value",
                            params![snapshot_id, basis_hash, record_hash, value],
                        )
                        .map_err(|error| {
                            format!("could not write pagination cache record: {error}")
                        })?;
                }
                PaginationCacheMutation::Remove { record_hash } => {
                    transaction
                        .execute(
                            "DELETE FROM pagination_records
                             WHERE snapshot_id = ?1 AND basis_hash = ?2 AND record_hash = ?3",
                            params![snapshot_id, basis_hash, record_hash],
                        )
                        .map_err(|error| {
                            format!("could not remove pagination cache record: {error}")
                        })?;
                }
            }
        }
        transaction
            .execute(
                "DELETE FROM pagination_bases
                 WHERE snapshot_id = ?1 AND basis_hash = ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM pagination_records
                       WHERE snapshot_id = ?1 AND basis_hash = ?2
                   )",
                params![snapshot_id, basis_hash],
            )
            .map_err(|error| format!("could not remove empty pagination cache basis: {error}"))?;

        loop {
            let (bytes, bases) = pagination_snapshot_usage(&transaction, snapshot_id)?;
            if bytes <= MAX_PAGINATION_SNAPSHOT_BYTES && bases <= MAX_PAGINATION_BASES_PER_SNAPSHOT
            {
                break;
            }
            if !evict_oldest_pagination_basis(
                &transaction,
                Some(snapshot_id),
                snapshot_id,
                basis_hash,
            )? {
                return Ok(PaginationCacheMutationOutcome::SnapshotLimitExceeded);
            }
        }
        while pagination_global_usage(&transaction)? > MAX_PAGINATION_GLOBAL_BYTES {
            if !evict_oldest_pagination_basis(&transaction, None, snapshot_id, basis_hash)? {
                return Err("pagination cache global quota could not be enforced".to_owned());
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit pagination cache mutation: {error}"))?;
        Ok(PaginationCacheMutationOutcome::Applied)
    }

    pub(crate) fn remove_pagination_basis(
        &mut self,
        publication_id: &str,
        snapshot_id: &str,
        basis_hash: &str,
    ) -> Result<PaginationCacheMutationOutcome, String> {
        validate_sha256_hash(basis_hash)?;
        if !pagination_snapshot_is_available(
            &self.connection,
            &self.blobs,
            publication_id,
            snapshot_id,
        )? {
            return Ok(PaginationCacheMutationOutcome::Unavailable);
        }
        let transaction = self
            .pagination
            .transaction()
            .map_err(|error| format!("could not begin pagination basis removal: {error}"))?;
        transaction
            .execute(
                "DELETE FROM pagination_bases WHERE snapshot_id = ?1 AND basis_hash = ?2",
                params![snapshot_id, basis_hash],
            )
            .map_err(|error| format!("could not remove pagination cache basis: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("could not commit pagination basis removal: {error}"))?;
        Ok(PaginationCacheMutationOutcome::Applied)
    }
}

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PaginationCacheReadOutcome {
    Found(Vec<u8>),
    Missing,
    Unavailable,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PaginationCacheRecordsReadOutcome {
    Records(Vec<Option<Vec<u8>>>),
    Unavailable,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PaginationCacheMutationOutcome {
    Applied,
    Unavailable,
    SnapshotLimitExceeded,
}

pub(crate) enum PaginationCacheMutation {
    Write { record_hash: String, value: Vec<u8> },
    Remove { record_hash: String },
}

pub(crate) fn pagination_snapshot_is_available(
    connection: &Connection,
    blobs: &Path,
    publication_id: &str,
    snapshot_id: &str,
) -> Result<bool, String> {
    let blob = connection
        .query_row(
            "SELECT b.blob_hash, b.storage_name
             FROM publications p
             JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
             JOIN archive_blobs b ON b.blob_hash = s.blob_hash
             WHERE p.publication_id = ?1 AND p.deletion_state = 'live'
               AND p.current_snapshot_id = ?2 AND s.snapshot_id = ?2
               AND s.revoked_at IS NULL
               AND b.health_state = 'available' AND b.lifecycle_state = 'live'",
            params![publication_id, snapshot_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("could not validate pagination cache snapshot: {error}"))?;
    let Some((blob_hash, storage_name)) = blob else {
        return Ok(false);
    };
    if !is_blake3_hash(&blob_hash) || storage_name != blob_hash {
        return Ok(false);
    }
    Ok(blobs.join(storage_name).is_file())
}

pub(crate) fn next_pagination_access_order(transaction: &Transaction<'_>) -> Result<i64, String> {
    let current = transaction
        .query_row(
            "SELECT COALESCE(MAX(access_order), 0) FROM pagination_bases",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("could not read pagination cache access order: {error}"))?;
    current
        .checked_add(1)
        .ok_or_else(|| "pagination cache access order is exhausted".to_owned())
}

pub(crate) fn touch_pagination_basis(
    transaction: &Transaction<'_>,
    snapshot_id: &str,
    basis_hash: &str,
) -> Result<(), String> {
    let access_order = transaction
        .query_row(
            "SELECT access_order FROM pagination_bases
             WHERE snapshot_id = ?1 AND basis_hash = ?2",
            params![snapshot_id, basis_hash],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("could not read pagination cache access: {error}"))?;
    let Some(access_order) = access_order else {
        return Ok(());
    };
    let latest_access_order = transaction
        .query_row(
            "SELECT COALESCE(MAX(access_order), 0) FROM pagination_bases",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("could not read pagination cache access order: {error}"))?;
    if access_order == latest_access_order {
        return Ok(());
    }
    let next_access_order = latest_access_order
        .checked_add(1)
        .ok_or_else(|| "pagination cache access order is exhausted".to_owned())?;
    transaction
        .execute(
            "UPDATE pagination_bases SET access_order = ?3
             WHERE snapshot_id = ?1 AND basis_hash = ?2",
            params![snapshot_id, basis_hash, next_access_order],
        )
        .map_err(|error| format!("could not update pagination cache access: {error}"))?;
    Ok(())
}

pub(crate) fn pagination_snapshot_usage(
    transaction: &Transaction<'_>,
    snapshot_id: &str,
) -> Result<(i64, i64), String> {
    transaction
        .query_row(
            "SELECT
                COALESCE((SELECT SUM(length(value)) FROM pagination_records
                          WHERE snapshot_id = ?1), 0),
                (SELECT COUNT(*) FROM pagination_bases WHERE snapshot_id = ?1)",
            [snapshot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("could not measure pagination snapshot usage: {error}"))
}

pub(crate) fn pagination_global_usage(transaction: &Transaction<'_>) -> Result<i64, String> {
    transaction
        .query_row(
            "SELECT COALESCE(SUM(length(value)), 0) FROM pagination_records",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("could not measure global pagination cache usage: {error}"))
}

pub(crate) fn evict_oldest_pagination_basis(
    transaction: &Transaction<'_>,
    only_snapshot_id: Option<&str>,
    target_snapshot_id: &str,
    target_basis_hash: &str,
) -> Result<bool, String> {
    let oldest = match only_snapshot_id {
        Some(snapshot_id) => transaction
            .query_row(
                "SELECT snapshot_id, basis_hash FROM pagination_bases
                 WHERE snapshot_id = ?1 AND basis_hash <> ?2
                 ORDER BY access_order, basis_hash LIMIT 1",
                params![snapshot_id, target_basis_hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional(),
        None => transaction
            .query_row(
                "SELECT snapshot_id, basis_hash FROM pagination_bases
                 WHERE NOT (snapshot_id = ?1 AND basis_hash = ?2)
                 ORDER BY access_order, snapshot_id, basis_hash LIMIT 1",
                params![target_snapshot_id, target_basis_hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional(),
    }
    .map_err(|error| format!("could not select pagination cache eviction: {error}"))?;
    let Some((snapshot_id, basis_hash)) = oldest else {
        return Ok(false);
    };
    let removed = transaction
        .execute(
            "DELETE FROM pagination_bases WHERE snapshot_id = ?1 AND basis_hash = ?2",
            params![snapshot_id, basis_hash],
        )
        .map_err(|error| format!("could not evict pagination cache basis: {error}"))?;
    Ok(removed == 1)
}

/// `SQLite` application ID `ELPG` for the pagination cache database.
const PAGINATION_APPLICATION_ID: u32 = 0x454c_5047;

/// The pagination cache lives in its own file. It is derived data: Navigator can
/// recompute every record from a snapshot, so it is never migrated and never
/// authoritative. Keeping it out of `library.v2.sqlite` keeps the churn of paging
/// — one write per basis touch, up to 64 KiB per record — out of the write-ahead
/// log that carries annotations and reading positions, and lets the cache run at
/// `synchronous = NORMAL`, where a lost write costs a recomputation rather than a
/// reader's note.
const PAGINATION_DATABASE: &str = "pagination.sqlite";

const PAGINATION_SCHEMA: &str = "
    PRAGMA application_id = 0x454c5047;
    PRAGMA user_version = 1;

    CREATE TABLE pagination_bases (
        snapshot_id TEXT NOT NULL CHECK(length(snapshot_id) = 32),
        basis_hash TEXT NOT NULL CHECK(
            length(basis_hash) = 64 AND basis_hash NOT GLOB '*[^0-9a-f]*'
        ),
        access_order INTEGER NOT NULL CHECK(access_order >= 1),
        PRIMARY KEY(snapshot_id, basis_hash)
    ) STRICT;

    CREATE INDEX pagination_basis_access_order ON pagination_bases(access_order);

    CREATE TABLE pagination_records (
        snapshot_id TEXT NOT NULL,
        basis_hash TEXT NOT NULL,
        record_hash TEXT NOT NULL CHECK(
            length(record_hash) = 64 AND record_hash NOT GLOB '*[^0-9a-f]*'
        ),
        value BLOB NOT NULL CHECK(length(value) <= 65536),
        PRIMARY KEY(snapshot_id, basis_hash, record_hash),
        FOREIGN KEY(snapshot_id, basis_hash)
            REFERENCES pagination_bases(snapshot_id, basis_hash) ON DELETE CASCADE
    ) STRICT;
";

/// Opens the pagination cache, recreating it whenever it is not recognisably this
/// version of the cache.
///
/// There is no migration ledger: the cache holds nothing that cannot be derived
/// again, so replacing an unrecognised file is both cheaper and safer than
/// carrying a second schema history.
pub(super) fn open_pagination_cache(app_data: &Path) -> Result<Connection, String> {
    let path = app_data.join(PAGINATION_DATABASE);
    if path.is_file() && !pagination_cache_is_current(&path) {
        for suffix in ["", "-wal", "-shm"] {
            let companion = path.with_file_name(format!("{PAGINATION_DATABASE}{suffix}"));
            if companion.exists() {
                fs::remove_file(&companion)
                    .map_err(|error| format!("could not discard the pagination cache: {error}"))?;
            }
        }
        tracing::info!("pagination cache discarded: it was not this cache version");
    }
    let connection = Connection::open(&path)
        .map_err(|error| format!("could not open the pagination cache: {error}"))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| format!("could not configure the pagination cache: {error}"))?;
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("could not read the pagination cache version: {error}"))?;
    if version == 0 {
        connection
            .execute_batch(PAGINATION_SCHEMA)
            .map_err(|error| format!("could not create the pagination cache: {error}"))?;
    }
    Ok(connection)
}

/// Whether the file is recognisably this version of the cache.
///
/// A file that cannot even be read as a database is not current, it is rubble:
/// `Connection::open` succeeds on a truncated or garbage file and the first pragma
/// is what fails. Reporting that as an error would refuse to launch over
/// disposable data, which is the opposite of what the cache is for.
fn pagination_cache_is_current(path: &Path) -> bool {
    let Ok(connection) = Connection::open(path) else {
        return false;
    };
    let identity = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, u32>(0))
        .and_then(|application_id| {
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .map(|version| (application_id, version))
        });
    matches!(identity, Ok((PAGINATION_APPLICATION_ID, 1)))
}

impl Storage {
    /// Drops cached pages for snapshots that are no longer current.
    ///
    /// The cache cannot reference the library across files, so the invariant the
    /// old foreign key enforced is kept here instead: at startup and after a
    /// deletion, anything the library no longer vouches for goes. An orphan is
    /// unreachable in the meantime, because every read authorizes its snapshot
    /// against the library first.
    pub(crate) fn retain_pagination_snapshots(&mut self) -> Result<(), String> {
        let live = self.live_snapshot_ids()?;
        let cached = {
            let mut statement = self
                .pagination
                .prepare("SELECT DISTINCT snapshot_id FROM pagination_bases")
                .map_err(|error| format!("could not list cached pagination: {error}"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| format!("could not list cached pagination: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("could not list cached pagination: {error}"))?
        };
        for snapshot_id in cached {
            if live.contains(&snapshot_id) {
                continue;
            }
            self.pagination
                .execute(
                    "DELETE FROM pagination_bases WHERE snapshot_id = ?1",
                    [&snapshot_id],
                )
                .map_err(|error| format!("could not drop cached pagination: {error}"))?;
        }
        Ok(())
    }
}
