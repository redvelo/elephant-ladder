use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use epub_stack::package::{Package, metadata::DcElement};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

mod annotation;
mod archive;
mod deletion;
mod dictionary;
mod journal;
mod library;
mod media;
mod migrations;
mod origin;
mod pagination;
mod policy;
mod position;
mod preferences;
mod reading_activity;
mod reconcile;
mod snapshot;
mod types;
#[cfg(test)]
pub(crate) use types::StoredReadingPosition;
pub(crate) use types::{
    InstallationOriginConfig, MAX_PAGINATION_BASES_PER_SNAPSHOT, MAX_PAGINATION_BULK_BYTES,
    MAX_PAGINATION_BULK_OPERATIONS, MAX_PAGINATION_GLOBAL_BYTES, MAX_PAGINATION_RECORD_BYTES,
    MAX_PAGINATION_SNAPSHOT_BYTES, MediaOverlayTimelineFile, MediaSidecar, PublicationMetadata,
    PublicationSelection, PublicationSummary, SaveReadingPositionOutcome, SelectedPublication,
    compose_preferences,
};

pub(crate) use annotation::{
    AnnotationCreatorRecord, AnnotationExchangeSource, AnnotationMutation, AnnotationProjection,
    AnnotationRecord, PublicAnnotationProjection, PublicAnnotationRecord, SaveAnnotationError,
    SaveAnnotationOutcome, format_annotation_timestamp, parse_annotation_timestamp,
};
pub(crate) use archive::{
    InstalledBlob, InstalledCover, ReserveImportOutcome, StoredCover, validate_sha256_hash,
};
pub(crate) use deletion::{BeginPublicationDeletionOutcome, ResetPublicationWebsiteDataOutcome};
pub(crate) use journal::{Journal, JournalError, JournalMutation, ReadingSummary};
pub(crate) use library::is_epub_core_image_mime;
#[cfg(test)]
use library::publication_metadata_from_persisted_value;
use library::{decode_creator_directions_summary, decode_creators_summary, publication_metadata};
#[cfg(test)]
use media::test_absent_media_overlay_json;
use media::{
    PreparedTimeline, install_media_overlay_timelines_in_transaction,
    install_media_sidecars_in_transaction, load_stored_media_overlay,
    prepare_media_overlay_timelines,
};
pub(crate) use migrations::migrate;
pub(crate) use origin::{
    active_browser_origin_id, allocate_browser_origin, is_origin_id, validate_browser_origin_state,
};
#[cfg(test)]
pub(crate) use pagination::PaginationCacheReadOutcome;
pub(crate) use pagination::{
    PaginationCacheMutation, PaginationCacheMutationOutcome, PaginationCacheRecordsReadOutcome,
};
pub use policy::{
    ActiveContentDefault, EffectiveActiveContentPolicy, EffectiveRemoteResourcesPolicy,
    PolicyRelevance, PublicationPolicyOverrides, PublicationPolicyRevisions,
    RemoteResourcesDefault,
};
pub(crate) use policy::{
    ActiveContentOverride, RemoteResourcesOverride, SavePublicationPolicyOutcome,
};
pub use policy::{
    EffectivePublicationPolicy, GlobalPublicationPolicy, PublicationPolicy,
    PublicationPolicyRelevance, StoredPublicationPolicy,
};
use position::decode_reading_position;
pub(crate) use preferences::{
    AppSettings, DeviceSettings, GlobalReaderPreferences, PublicationReaderPreferences,
    SavePreferencesOutcome, load_global_preferences,
};
use preferences::{
    MAX_NARRATION_SKIPPED_TYPE_BYTES, MAX_NARRATION_SKIPPED_TYPES, NARRATION_RATES,
    default_narration_rate,
};
pub use preferences::{PreferenceRevisions, ReaderPreferences, StoredPreferences};
pub(crate) use reading_activity::ActivityRecord;
pub use reconcile::fsync_directory;
pub(crate) use reconcile::{
    consume_import_reservation, is_blake3_hash, reconcile_archive_storage,
    reconcile_media_sidecar_storage, reconcile_media_timeline_storage, remove_file_if_present,
    remove_unreferenced_covers, remove_unreferenced_media_sidecars, sync_directory, verify_blob,
};
pub(crate) use snapshot::{ImportOutcome, InstalledImport, PolicyAwareReadingProducts};

pub(crate) use dictionary::{
    DictionaryAudioChoice, DictionaryInstallation, DictionaryOperationPhase, DictionaryPackType,
    DictionaryViewRecord, DictionaryViewType, NewDictionaryInstallation, NewDictionaryView,
    installed_directory as dictionary_installed_directory,
    revision_directory as dictionary_revision_directory,
    staging_directory as dictionary_staging_directory,
};

use crate::ReadingPosition;
use crate::media_overlay::MediaOverlayDescription;
use crate::reading::validate_reading_position;

pub struct Storage {
    pub(super) connection: Connection,
    /// Derived page geometry, in its own file so its churn never shares a
    /// write-ahead log with annotations or reading positions.
    pub(super) pagination: Connection,
    pub(super) _host_lock: File,
    pub(super) blobs: PathBuf,
    pub(super) covers: PathBuf,
    pub(super) media_sidecars: PathBuf,
    pub(super) media_timelines: PathBuf,
    pub(super) staging: PathBuf,
    pub(super) trash: PathBuf,
    pub(super) dictionaries: PathBuf,
    pub(super) archive_quota_bytes: Option<u64>,
}

const IMPORT_RESERVATION_LIFETIME: &str = "+1 hour";

/// `library.sqlite` held the pre-release schema, whose ledger was collapsed
/// rather than migrated. It is never opened again.
pub(crate) const LIBRARY_DATABASE: &str = "library.v2.sqlite";

/// `SQLite` application ID `ELLB`, stamped into the database header by the
/// schema baseline so a library database identifies itself the way a dictionary
/// pack does.
const LIBRARY_APPLICATION_ID: u32 = 0x454c_4c42;

/// Refuses a file that is not this application's library before any migration
/// touches it.
///
/// A `Storage::open` failure aborts the process, and on Android the database is
/// app-private, so the one diagnostic a user can produce is a log line. It has to
/// say which file was wrong and why rather than surface as a constraint error
/// several layers further in.
fn validate_library_identity(connection: &Connection, database: &Path) -> Result<(), String> {
    let application_id: u32 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .map_err(|error| format!("could not read the library application ID: {error}"))?;
    // An unstamped header belongs to a database the baseline has not run on yet:
    // setting `journal_mode` already wrote pages, so page count cannot tell a new
    // database from an old one. A foreign file that is also unstamped is caught
    // after migration, by the schema comparison.
    if application_id != 0 && application_id != LIBRARY_APPLICATION_ID {
        return Err(format!(
            "{} is not an Elephant Ladder library: application ID {application_id:#010x} \
             does not match {LIBRARY_APPLICATION_ID:#010x}",
            database.display()
        ));
    }
    Ok(())
}

/// How long a relaunch waits for a departing host to let go of the storage lock.
const HOST_LOCK_WAIT: Duration = Duration::from_secs(2);
const HOST_LOCK_RETRY: Duration = Duration::from_millis(50);

/// Takes the exclusive app-storage lock, tolerating a predecessor still exiting.
///
/// `try_lock_exclusive` is non-blocking, so quitting and immediately relaunching
/// races the previous process's shutdown: the lock is released when its file
/// descriptor closes, and until then a new host sees the same `EAGAIN` that a
/// genuinely running second instance produces. Failing on the first attempt makes
/// those two indistinguishable and refuses to start over a condition that clears
/// itself in milliseconds. Waiting a bounded moment tells them apart — a departing
/// host lets go, a running one does not — and a real second instance still gets the
/// same refusal, just later.
fn acquire_host_lock(app_data: &Path) -> Result<File, String> {
    let path = app_data.join("native-host.lock");
    let deadline = std::time::Instant::now() + HOST_LOCK_WAIT;
    loop {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("could not open native host lock: {error}"))?;
        let Err(error) = file.try_lock_exclusive() else {
            return Ok(file);
        };
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "app storage is already owned by another host: {error}"
            ));
        }
        drop(file);
        std::thread::sleep(HOST_LOCK_RETRY);
    }
}

fn open_library_connection(database: &Path) -> Result<Connection, String> {
    let connection = Connection::open(database)
        .map_err(|error| format!("could not open library database: {error}"))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|error| format!("could not configure library database: {error}"))?;
    Ok(connection)
}

/// Whether a ledger step is going to run, which is also when reading every page
/// is worth its cost.
fn library_upgrade_pending(connection: &Connection) -> Result<bool, String> {
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("could not read library schema version: {error}"))?;
    Ok(version != migrations::CURRENT_SCHEMA)
}

/// Reads every page and returns `SQLite`'s report when the file is damaged.
///
/// This is the one startup check whose cost grows with the library, so it runs
/// only when an upgrade is pending rather than on every launch. Corruption in a
/// database that is already at the current version therefore surfaces later, as a
/// failing query, rather than here.
fn library_integrity_failure(connection: &Connection) -> Result<Option<String>, String> {
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| format!("could not check the library database: {error}"))?;
    Ok((integrity != "ok").then_some(integrity))
}

/// Moves a damaged library aside, keeping its bytes rather than deleting them,
/// and takes the archives with it.
///
/// The archives have to move too. The replacement library claims no blob, so
/// `reconcile_archive_storage` would sweep every imported publication on this very
/// startup; renaming the directory puts them out of its reach and beside the
/// quarantined database, where they can be imported again.
///
/// The name carries the time so a second failure cannot overwrite the evidence of
/// the first. The write-ahead log and shared-memory files go with it, because a
/// stray `-wal` beside a fresh database would be applied to it.
fn quarantine_library(database: &Path, blobs: &Path) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("could not name the quarantined library: {error}"))?
        .as_secs();
    let mut target = database.with_file_name(format!("{LIBRARY_DATABASE}.unopenable-{stamp}"));
    let mut attempt = 1;
    while target.exists() {
        target =
            database.with_file_name(format!("{LIBRARY_DATABASE}.unopenable-{stamp}-{attempt}"));
        attempt += 1;
    }
    for suffix in ["", "-wal", "-shm"] {
        let from = database.with_file_name(format!("{LIBRARY_DATABASE}{suffix}"));
        if !from.exists() {
            continue;
        }
        let to = PathBuf::from(format!("{}{suffix}", target.display()));
        fs::rename(&from, &to)
            .map_err(|error| format!("could not set aside the damaged library: {error}"))?;
    }
    if blobs.is_dir() {
        let kept = blobs.with_file_name(format!("unopenable-{stamp}-blobs"));
        if !kept.exists() {
            fs::rename(blobs, &kept)
                .map_err(|error| format!("could not set aside the orphaned archives: {error}"))?;
        }
        fs::create_dir_all(blobs)
            .map_err(|error| format!("could not recreate {}: {error}", blobs.display()))?;
    }
    Ok(target)
}

/// Checks the migrated database against the schema the ledger produces.
///
/// The object set catches a database that carries the right version stamp but the
/// wrong shape, which no `user_version` comparison can see. It is the cheap half
/// of the old combined check: building the reference schema costs under a
/// millisecond and reads nothing from disk.
fn validate_library_schema(connection: &Connection, database: &Path) -> Result<(), String> {
    let expected = Connection::open_in_memory()
        .map_err(|error| format!("could not build the reference library schema: {error}"))?;
    migrate(&expected)?;
    if schema_objects(connection)? != schema_objects(&expected)? {
        return Err(format!(
            "{} does not have the schema of library version {}",
            database.display(),
            migrations::CURRENT_SCHEMA
        ));
    }
    Ok(())
}

fn schema_objects(connection: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT name, COALESCE(sql, '') FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|error| format!("could not read a library schema: {error}"))?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            ))
        })
        .map_err(|error| format!("could not query a library schema: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not collect a library schema: {error}"))?;
    Ok(objects)
}

/// Moves pre-release EPUB archives out of the reconciled tree exactly once.
///
/// The collapsed schema starts empty, and `reconcile_archive_storage` deletes
/// every blob without an `archive_blobs` row, so the first launch would destroy
/// imported archives before anyone could copy them out — and on Android the
/// directory is app-private, so there is no manual rescue. Nothing else is
/// preserved: library records, positions, annotations, preferences and installed
/// dictionary packs are all discarded with the old database.
///
/// Remove this once every development machine and the BOOX have launched once.
fn rescue_pre_release_archives(
    app_data: &Path,
    database: &Path,
    blobs: &Path,
) -> Result<(), String> {
    let rescued = app_data.join("archives").join("pre-release-blobs");
    if database.exists()
        || rescued.exists()
        || !app_data.join("library.sqlite").exists()
        || !blobs.exists()
    {
        return Ok(());
    }
    fs::rename(blobs, &rescued)
        .map_err(|error| format!("could not set aside pre-release archives: {error}"))?;
    fs::create_dir_all(blobs)
        .map_err(|error| format!("could not recreate {}: {error}", blobs.display()))?;
    tracing::warn!(
        rescued = %rescued.display(),
        "pre-release library discarded; imported archives set aside for re-import"
    );
    Ok(())
}

impl Storage {
    pub fn open(app_data: &Path) -> Result<Self, String> {
        Self::open_inner(app_data, app_data, None)
    }

    /// Opens storage with bulk, re-downloadable data under its own root.
    ///
    /// Dictionaries are multi-gigabyte and machine-local; on Windows the application data
    /// directory is the roaming profile, which synchronizing domains copy around, so the
    /// host keeps them under the local data directory instead. On Linux and Android both
    /// directories resolve to the same path.
    pub fn open_with_bulk_data(app_data: &Path, bulk_data: &Path) -> Result<Self, String> {
        Self::open_inner(app_data, bulk_data, None)
    }

    fn open_with_archive_quota(
        app_data: &Path,
        archive_quota_bytes: Option<u64>,
    ) -> Result<Self, String> {
        Self::open_inner(app_data, app_data, archive_quota_bytes)
    }

    fn open_inner(
        app_data: &Path,
        bulk_data: &Path,
        archive_quota_bytes: Option<u64>,
    ) -> Result<Self, String> {
        let archives = app_data.join("archives");
        let blobs = archives.join("blobs");
        let covers = app_data.join("covers");
        let media_sidecars = app_data.join("media-sidecars");
        let media_timelines = app_data.join("media-timelines");
        let staging = archives.join("staging");
        let trash = archives.join("trash");
        for directory in [
            app_data,
            archives.as_path(),
            &blobs,
            &covers,
            &media_sidecars,
            &media_timelines,
            &staging,
            &trash,
        ] {
            fs::create_dir_all(directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        }

        let host_lock = acquire_host_lock(app_data)?;
        let database = app_data.join(LIBRARY_DATABASE);
        rescue_pre_release_archives(app_data, &database, &blobs)?;
        let mut connection = open_library_connection(&database)?;
        validate_library_identity(&connection, &database)?;
        // An upgrade is about to rewrite pages, so the file is read first. Migrating
        // corrupt pages cannot make them readable, and refusing would leave the
        // reader with an application that will not start at all, so a corrupt
        // library is set aside and replaced. The archives are untouched, so the
        // publications can be imported again.
        if library_upgrade_pending(&connection)?
            && let Some(report) = library_integrity_failure(&connection)?
        {
            drop(connection);
            let quarantined = quarantine_library(&database, &blobs)?;
            tracing::error!(
                event = "library_quarantined",
                reason = %report,
                moved_to = %quarantined.display(),
                "library database failed its integrity check and was replaced"
            );
            connection = open_library_connection(&database)?;
        }
        migrate(&connection)?;
        validate_library_schema(&connection, &database)?;
        validate_browser_origin_state(&connection)?;
        reconcile_archive_storage(&mut connection, &blobs, &covers, &staging, &trash)?;
        reconcile_media_sidecar_storage(&mut connection, &media_sidecars)?;
        reconcile_media_timeline_storage(&mut connection, &media_timelines)?;
        let dictionaries = bulk_data.join("dictionaries");
        // The bulk root is not the application data directory on Windows and may not
        // exist yet; the legacy move below renames into it.
        fs::create_dir_all(bulk_data)
            .map_err(|error| format!("could not create {}: {error}", bulk_data.display()))?;
        // Dictionaries predate the bulk-data root and lived under the application data
        // directory. Both roots are on the same volume, so one rename moves everything;
        // leaving the old tree behind would strand gigabytes the registry still names.
        let legacy_dictionaries = app_data.join("dictionaries");
        if legacy_dictionaries != dictionaries
            && legacy_dictionaries.is_dir()
            && !dictionaries.exists()
        {
            fs::rename(&legacy_dictionaries, &dictionaries).map_err(|error| {
                format!(
                    "could not move dictionary storage to {}: {error}",
                    dictionaries.display()
                )
            })?;
        }
        dictionary::reconcile_dictionary_storage(&connection, &dictionaries)?;

        let mut storage = Self {
            connection,
            pagination: pagination::open_pagination_cache(app_data)?,
            _host_lock: host_lock,
            blobs,
            covers,
            media_sidecars,
            media_timelines,
            staging,
            trash,
            dictionaries,
            archive_quota_bytes,
        };
        storage.retain_pagination_snapshots()?;
        storage.resume_pending_deletions()?;
        Ok(storage)
    }

    #[cfg(test)]
    fn open_for_test(app_data: &Path, archive_quota_bytes: u64) -> Result<Self, String> {
        Self::open_with_archive_quota(app_data, Some(archive_quota_bytes))
    }

    pub(crate) fn installation_origin_config(
        &self,
    ) -> Result<Option<InstallationOriginConfig>, String> {
        self.connection
            .query_row(
                "SELECT foreground_port, reader_port
                 FROM installation_origin_config WHERE singleton_key = 1",
                [],
                |row| {
                    Ok(InstallationOriginConfig {
                        foreground_port: row.get(0)?,
                        reader_port: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("could not load installation origin configuration: {error}"))
    }

    pub fn initialize_origin_ports(
        &mut self,
        foreground_port: u16,
        reader_port: u16,
    ) -> Result<(), String> {
        if foreground_port == 0 || reader_port == 0 || foreground_port == reader_port {
            return Err("installation origin ports are invalid".to_owned());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin origin port initialization: {error}"))?;
        let existing = transaction
            .query_row(
                "SELECT foreground_port, reader_port
                 FROM installation_origin_config WHERE singleton_key = 1",
                [],
                |row| Ok((row.get::<_, u16>(0)?, row.get::<_, Option<u16>>(1)?)),
            )
            .optional()
            .map_err(|error| format!("could not inspect origin port configuration: {error}"))?;
        if let Some((existing_foreground, existing_reader)) = existing {
            if existing_foreground != foreground_port {
                return Err("foreground publication port cannot be rotated".to_owned());
            }
            if existing_reader.is_some_and(|existing| existing != reader_port) {
                return Err("reader port cannot be rotated".to_owned());
            }
            if existing_reader.is_none() {
                transaction
                    .execute(
                        "UPDATE installation_origin_config SET reader_port = ?1
                         WHERE singleton_key = 1",
                        [reader_port],
                    )
                    .map_err(|error| format!("could not persist reader port: {error}"))?;
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO installation_origin_config
                        (singleton_key, scheme_version, foreground_port, reader_port)
                     VALUES (1, 1, ?1, ?2)",
                    params![foreground_port, reader_port],
                )
                .map_err(|error| format!("could not persist origin ports: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit origin ports: {error}"))
    }

    pub fn load_global_preferences(&self) -> Result<StoredPreferences, String> {
        let (global, global_revision) = load_global_preferences(&self.connection)?;
        let defaults = global.reading_defaults.clone().unwrap_or_default();
        Ok(StoredPreferences {
            preferences: compose_preferences(global, defaults)?,
            revisions: PreferenceRevisions {
                global: global_revision,
                publication: 0,
            },
        })
    }

    pub fn checkpoint(&self) {
        let _ = self
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

pub(super) fn random_uuid() -> Result<String, String> {
    let mut value = [0_u8; 16];
    getrandom::fill(&mut value).map_err(|error| error.to_string())?;
    Ok(format_uuid(value, 4))
}

pub(crate) fn format_uuid(mut value: [u8; 16], version: u8) -> String {
    value[6] = (value[6] & 0x0f) | (version << 4);
    value[8] = (value[8] & 0x3f) | 0x80;
    let hex: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value).map_err(|error| error.to_string())?;
    Ok(value.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests;

/// Shared handle to the single process-wide SQLite store.
///
/// All EPUB and SQLite work is synchronous, so every access runs on a blocking
/// worker rather than an async executor thread (see the production plan,
/// "Concurrency And Limits"). `with` owns that rule, together with the one
/// mapping from a poisoned lock or a failed worker to an operational fault, so
/// no handler repeats it.
#[derive(Clone)]
pub struct StorageHandle(Arc<Mutex<Storage>>);

/// Why a storage access could not produce a value. `Failed` carries the store's
/// own message for logging; it is never sent to a client.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StorageFault {
    #[error("native storage lock poisoned")]
    Poisoned,
    #[error("storage worker failed")]
    Worker,
    #[error("{0}")]
    Failed(String),
}

impl StorageHandle {
    pub(crate) fn new(storage: Storage) -> Self {
        Self(Arc::new(Mutex::new(storage)))
    }

    /// Runs one synchronous storage operation on a blocking worker.
    ///
    /// `operation` names the access for diagnostics. A fault is logged here, once,
    /// and never travels to a client: callers map `Err` to their own response.
    pub(crate) async fn with<T, F>(
        &self,
        operation: &'static str,
        work: F,
    ) -> Result<T, StorageFault>
    where
        F: FnOnce(&mut Storage) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let storage = self.0.clone();
        let outcome = match tokio::task::spawn_blocking(move || {
            let mut guard = storage.lock().map_err(|_| StorageFault::Poisoned)?;
            work(&mut guard).map_err(StorageFault::Failed)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(StorageFault::Worker),
        };
        if let Err(fault) = &outcome {
            let error_kind = match fault {
                StorageFault::Poisoned => "lock_poisoned",
                StorageFault::Worker => "worker",
                StorageFault::Failed(_) => "storage",
            };
            // The message stays in the log: it can name authored paths, so it
            // never travels to a client.
            tracing::error!(
                event = "storage_operation_failed",
                operation,
                error_kind,
                detail = %fault
            );
        }
        outcome
    }

    /// Locks the store from inside a blocking worker, mapping poisoning to the
    /// store's own error type. This is the single spelling of that mapping.
    pub(crate) fn locked(&self) -> Result<std::sync::MutexGuard<'_, Storage>, String> {
        self.0
            .lock()
            .map_err(|_| "native storage lock poisoned".to_owned())
    }

    /// Direct lock, for the synchronous paths that already hold a worker thread.
    pub(crate) fn lock(&self) -> std::sync::LockResult<std::sync::MutexGuard<'_, Storage>> {
        self.0.lock()
    }
}
