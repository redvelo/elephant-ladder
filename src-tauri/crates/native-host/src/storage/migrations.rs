//! The schema migration ledger.
//!
//! Version 1 is the whole schema. The pre-release ledger that built it one
//! `ALTER TABLE` at a time was collapsed before the first release, while there
//! was still no installed data to carry forward; `library.sqlite` belongs to
//! that era and is never opened again.
//!
//! From here the ledger is appended to and never reordered. Each step assumes
//! every earlier step has run, and a step that has shipped is frozen: it may use
//! SQL and helpers copied into this module, never live application code, so its
//! meaning cannot drift when a shared helper changes.
//!
//! `step` owns the transaction, the `user_version` stamp and the version
//! counter, so no step can advance the schema on disk without advancing it in
//! memory, and a gap in the numbering is an error rather than a silently skipped
//! step. Every database runs the whole ledger from 0, including a brand new one:
//! a second path is a second schema, and the two drift.
//!
//! Migrations run at startup, before the reading WebView is created, so no
//! request can observe a half-migrated database.

use rusqlite::{Connection, Transaction, TransactionBehavior};

pub(crate) const CURRENT_SCHEMA: u32 = 1;

pub(crate) fn migrate(connection: &Connection) -> Result<(), String> {
    let mut version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(|error| format!("could not read library schema version: {error}"))?;
    if version > CURRENT_SCHEMA {
        return Err(format!(
            "library schema version {version} is newer than supported version {CURRENT_SCHEMA}"
        ));
    }
    step(connection, &mut version, 1, |transaction| {
        transaction
            .execute_batch(BASELINE)
            .map_err(|error| format!("could not create the library schema: {error}"))
    })?;
    Ok(())
}

/// Runs one ledger step in its own immediate transaction. The schema change,
/// the `user_version` stamp and the in-memory version all land in one commit,
/// or none of them do.
fn step(
    connection: &Connection,
    version: &mut u32,
    target: u32,
    apply: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
) -> Result<(), String> {
    if *version >= target {
        return Ok(());
    }
    if *version + 1 != target {
        return Err(format!(
            "library schema ledger skips from version {version} to {target}"
        ));
    }
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
        .map_err(|error| format!("could not begin library version {target} migration: {error}"))?;
    apply(&transaction)?;
    transaction
        .pragma_update(None, "user_version", target)
        .map_err(|error| format!("could not set library schema version {target}: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("could not commit library version {target} migration: {error}"))?;
    *version = target;
    Ok(())
}

const BASELINE: &str = "
    PRAGMA application_id = 0x454c4c42;

    CREATE TABLE archive_blobs (
        blob_hash TEXT PRIMARY KEY CHECK(length(blob_hash) = 64),
        storage_name TEXT NOT NULL UNIQUE,
        size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
        health_state TEXT NOT NULL CHECK(
            health_state IN ('available', 'missing', 'corrupt')
        ),
        lifecycle_state TEXT NOT NULL CHECK(
            lifecycle_state IN ('live', 'gc_pending', 'trashed')
        ),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    -- EPUB lets a publication declare which direction its title and each creator name
    -- read, so the directions are denormalized beside the text they belong to; the facts
    -- they come from are too large to parse for a library listing.
    CREATE TABLE publications (
        publication_id TEXT PRIMARY KEY CHECK(length(publication_id) = 32),
        imported_blob_hash TEXT UNIQUE REFERENCES archive_blobs(blob_hash),
        current_snapshot_id TEXT,
        title_summary TEXT NOT NULL,
        title_direction_summary TEXT,
        creators_summary TEXT NOT NULL DEFAULT '[]',
        creator_directions_summary TEXT NOT NULL DEFAULT '[]',
        language_summary TEXT,
        reading_direction_summary TEXT,
        imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        deletion_state TEXT NOT NULL CHECK(
            deletion_state IN ('live', 'deleting', 'delete_failed')
        ),
        FOREIGN KEY(current_snapshot_id) REFERENCES publication_snapshots(snapshot_id)
    ) STRICT;

    -- `publication_id` deliberately does not cascade: deleting a publication runs
    -- through the `deletion_operations` state machine, which detaches blobs and
    -- browser origins in order, so a plain row delete must fail rather than
    -- silently discard a snapshot the machine still has work for.
    CREATE TABLE publication_snapshots (
        snapshot_id TEXT PRIMARY KEY CHECK(length(snapshot_id) = 32),
        publication_id TEXT NOT NULL REFERENCES publications(publication_id),
        blob_hash TEXT NOT NULL REFERENCES archive_blobs(blob_hash),
        reading_revision TEXT NOT NULL CHECK(length(reading_revision) BETWEEN 1 AND 64),
        facts_json TEXT NOT NULL CHECK(length(facts_json) <= 25165824),
        responses_json TEXT NOT NULL CHECK(length(responses_json) <= 25165824),
        active_content_relevance TEXT NOT NULL DEFAULT 'unknown' CHECK(
            active_content_relevance IN ('detected', 'not-detected', 'unknown')
        ),
        remote_resources_relevance TEXT NOT NULL DEFAULT 'unknown' CHECK(
            remote_resources_relevance IN ('detected', 'not-detected', 'unknown')
        ),
        media_overlay_json TEXT CHECK(
            media_overlay_json IS NULL OR length(media_overlay_json) <= 25165824
        ),
        media_overlay_capability_json TEXT CHECK(
            media_overlay_capability_json IS NULL
            OR length(media_overlay_capability_json) <= 25165824
        ),
        cover_hash TEXT CHECK(cover_hash IS NULL OR length(cover_hash) = 64),
        cover_size_bytes INTEGER CHECK(cover_size_bytes IS NULL OR cover_size_bytes >= 0),
        cover_mime TEXT CHECK(cover_mime IS NULL OR cover_mime IN (
            'image/gif', 'image/jpeg', 'image/png', 'image/svg+xml',
            'image/webp', 'image/avif', 'image/jxl'
        )),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        revoked_at TEXT
    ) STRICT;

    CREATE INDEX publication_snapshot_owner
        ON publication_snapshots(publication_id);

    CREATE TABLE reading_positions (
        publication_id TEXT PRIMARY KEY
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        position_json TEXT NOT NULL,
        summary_json TEXT,
        position_revision INTEGER NOT NULL CHECK(
            position_revision >= 1 AND position_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE annotation_projections (
        publication_id TEXT PRIMARY KEY
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        projection_revision INTEGER NOT NULL CHECK(
            projection_revision >= 1 AND projection_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    -- One row per annotation rather than one JSON document per publication: a
    -- mutation writes one row instead of rewriting every annotation the reader
    -- owns, and annotations stay queryable across the library and addressable
    -- individually for exchange.
    -- Keyed by publication as well as id. An annotation id is only unique within
    -- its publication: `local_annotation_id` derives it from the foreign id in an
    -- imported set, so importing one exported set into two publications yields the
    -- same id in both, and a global key would make one overwrite the other.
    CREATE TABLE annotations (
        annotation_id TEXT NOT NULL CHECK(length(annotation_id) BETWEEN 1 AND 128),
        publication_id TEXT NOT NULL
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        record_order INTEGER NOT NULL CHECK(record_order >= 1),
        record_type TEXT NOT NULL CHECK(
            record_type IN ('highlight', 'note', 'bookmark')
        ),
        occurrence INTEGER NOT NULL CHECK(occurrence >= 0 AND occurrence <= 4294967295),
        target_json TEXT NOT NULL CHECK(length(target_json) <= 1048576),
        resource_hint TEXT NOT NULL CHECK(length(resource_hint) <= 16384),
        section_label TEXT,
        excerpt TEXT,
        text TEXT,
        label TEXT,
        creator_json TEXT CHECK(creator_json IS NULL OR json_valid(creator_json)),
        created TEXT NOT NULL,
        modified TEXT NOT NULL CHECK(modified >= created),
        PRIMARY KEY(publication_id, annotation_id),
        UNIQUE(publication_id, record_order),
        CHECK((record_type = 'note') = (text IS NOT NULL)),
        CHECK(excerpt IS NULL OR record_type IN ('highlight', 'note')),
        CHECK(label IS NULL OR record_type = 'bookmark')
    ) STRICT;

    CREATE INDEX annotation_history ON annotations(publication_id, created);

    CREATE TABLE reflections (
        id TEXT PRIMARY KEY,
        publication_id TEXT NOT NULL
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        text TEXT NOT NULL DEFAULT '',
        prompt INTEGER NOT NULL CHECK(prompt BETWEEN 0 AND 4),
        draft INTEGER NOT NULL CHECK(draft IN (0, 1)),
        context_json TEXT,
        position_json TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE UNIQUE INDEX reflection_draft ON reflections(publication_id) WHERE draft = 1;
    CREATE INDEX reflection_history ON reflections(publication_id, created_at DESC);

    CREATE TABLE reading_activity (
        recording_id TEXT NOT NULL,
        day TEXT NOT NULL,
        milliseconds INTEGER NOT NULL CHECK(milliseconds >= 0),
        pages INTEGER NOT NULL CHECK(pages >= 0),
        PRIMARY KEY(recording_id, day)
    ) STRICT;

    CREATE INDEX reading_activity_day ON reading_activity(day);

    CREATE TABLE import_reservations (
        reservation_id TEXT PRIMARY KEY CHECK(
            length(reservation_id) = 32 AND reservation_id NOT GLOB '*[^0-9a-f]*'
        ),
        staging_name TEXT NOT NULL UNIQUE CHECK(
            staging_name = reservation_id || '.part'
        ),
        reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes > 0),
        blob_hash TEXT CHECK(
            blob_hash IS NULL OR (
                length(blob_hash) = 64 AND blob_hash NOT GLOB '*[^0-9a-f]*'
            )
        ),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        expires_at TEXT NOT NULL CHECK(expires_at > created_at)
    ) STRICT;

    CREATE INDEX import_reservation_expiration ON import_reservations(expires_at);

    CREATE TABLE deletion_operations (
        publication_id TEXT PRIMARY KEY
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        phase TEXT NOT NULL CHECK(phase IN ('marked', 'detached')),
        attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
        next_retry_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        last_error TEXT,
        detached_imported_blob_hash TEXT CHECK(
            detached_imported_blob_hash IS NULL
            OR length(detached_imported_blob_hash) = 64
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE blob_cleanup_operations (
        blob_hash TEXT PRIMARY KEY REFERENCES archive_blobs(blob_hash) ON DELETE CASCADE,
        phase TEXT NOT NULL CHECK(phase IN ('pending', 'trashed')),
        attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
        next_retry_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        last_error TEXT,
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE reader_preferences (
        singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
        preferences_json TEXT NOT NULL CHECK(length(preferences_json) <= 4096),
        preferences_revision INTEGER NOT NULL CHECK(
            preferences_revision >= 1 AND preferences_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE publication_reader_preferences (
        publication_id TEXT PRIMARY KEY
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        preferences_json TEXT NOT NULL CHECK(length(preferences_json) <= 1024),
        preferences_revision INTEGER NOT NULL CHECK(
            preferences_revision >= 1 AND preferences_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE publication_policy_defaults (
        singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
        active_content TEXT NOT NULL CHECK(
            active_content IN ('automatic', 'ask', 'disabled')
        ),
        remote_resources TEXT NOT NULL CHECK(
            remote_resources IN ('allowed', 'ask', 'blocked')
        ),
        policy_revision INTEGER NOT NULL CHECK(
            policy_revision >= 1 AND policy_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE publication_policy_overrides (
        publication_id TEXT PRIMARY KEY
            REFERENCES publications(publication_id) ON DELETE CASCADE,
        active_content TEXT CHECK(
            active_content IS NULL OR active_content IN ('enabled', 'disabled')
        ),
        remote_resources TEXT CHECK(
            remote_resources IS NULL OR remote_resources IN ('allowed', 'blocked')
        ),
        policy_revision INTEGER NOT NULL CHECK(
            policy_revision >= 1 AND policy_revision <= 4294967295
        ),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE installation_origin_config (
        singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
        scheme_version INTEGER NOT NULL CHECK(scheme_version = 1),
        foreground_port INTEGER NOT NULL CHECK(foreground_port BETWEEN 1 AND 65535),
        reader_port INTEGER CHECK(reader_port IS NULL OR reader_port BETWEEN 1 AND 65535),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    ) STRICT;

    CREATE TABLE browser_origin_allocations (
        origin_id TEXT PRIMARY KEY CHECK(
            length(origin_id) = 32 AND origin_id NOT GLOB '*[^0-9a-f]*'
        ),
        publication_id TEXT REFERENCES publications(publication_id) ON DELETE SET NULL,
        lifecycle_state TEXT NOT NULL CHECK(
            lifecycle_state IN ('active', 'retired', 'purge_pending', 'purged')
        ),
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        retired_at TEXT,
        purge_attempts INTEGER NOT NULL DEFAULT 0 CHECK(purge_attempts >= 0),
        last_purge_error TEXT,
        last_purge_report_json TEXT,
        CHECK(lifecycle_state != 'active' OR publication_id IS NOT NULL),
        CHECK((lifecycle_state = 'active') = (retired_at IS NULL))
    ) STRICT;

    CREATE UNIQUE INDEX active_browser_origin_owner
        ON browser_origin_allocations(publication_id)
        WHERE lifecycle_state = 'active';

    CREATE TABLE media_overlay_sidecars (
        snapshot_id TEXT NOT NULL
            REFERENCES publication_snapshots(snapshot_id) ON DELETE CASCADE,
        canonical_path TEXT NOT NULL,
        storage_name TEXT NOT NULL CHECK(length(storage_name) = 64),
        length_bytes INTEGER NOT NULL CHECK(length_bytes >= 0),
        media_type TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        PRIMARY KEY (snapshot_id, canonical_path)
    ) STRICT;

    CREATE TABLE media_overlay_timeline_files (
        snapshot_id TEXT NOT NULL
            REFERENCES publication_snapshots(snapshot_id) ON DELETE CASCADE,
        overlay_index INTEGER NOT NULL CHECK(overlay_index >= 0),
        storage_name TEXT NOT NULL CHECK(length(storage_name) = 64),
        length_bytes INTEGER NOT NULL CHECK(length_bytes >= 0),
        PRIMARY KEY (snapshot_id, overlay_index)
    ) STRICT;

    CREATE TABLE media_sidecar_transcodes (
        source_hash TEXT NOT NULL CHECK(length(source_hash) = 64),
        derivation TEXT NOT NULL,
        storage_name TEXT NOT NULL CHECK(length(storage_name) = 64),
        length_bytes INTEGER NOT NULL CHECK(length_bytes >= 0),
        PRIMARY KEY (source_hash, derivation)
    ) STRICT;

    CREATE TABLE dictionary_installations (
        pack_id TEXT NOT NULL CHECK(length(pack_id) BETWEEN 1 AND 64),
        pack_revision TEXT NOT NULL CHECK(length(pack_revision) = 64),
        pack_type TEXT NOT NULL CHECK(pack_type IN ('corpus', 'audio')),
        corpus_language TEXT NOT NULL CHECK(length(corpus_language) BETWEEN 1 AND 35),
        manifest_sha256 TEXT NOT NULL CHECK(length(manifest_sha256) = 64),
        state TEXT NOT NULL CHECK(state IN ('installed', 'deleting')),
        installed_bytes INTEGER NOT NULL CHECK(installed_bytes >= 0),
        PRIMARY KEY (pack_id, pack_revision)
    ) STRICT;

    CREATE UNIQUE INDEX dictionary_installed_revision
        ON dictionary_installations(pack_id) WHERE state = 'installed';

    CREATE TABLE dictionary_views (
        view_id TEXT PRIMARY KEY CHECK(length(view_id) BETWEEN 1 AND 64),
        pack_id TEXT NOT NULL CHECK(length(pack_id) BETWEEN 1 AND 64),
        view_type TEXT NOT NULL CHECK(view_type IN ('monolingual', 'bilingual')),
        target_language TEXT CHECK(
            target_language IS NULL OR length(target_language) BETWEEN 1 AND 35
        ),
        enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
        display_order INTEGER NOT NULL UNIQUE CHECK(display_order >= 0),
        CHECK((view_type = 'bilingual') = (target_language IS NOT NULL))
    ) STRICT;

    -- Installing a language is one download of its corpus and the pronunciations that
    -- come with it, so an operation names the language and where its recordings play
    -- from rather than one pack; a resume after a restart then continues the language
    -- rather than whichever pack was in flight.
    CREATE TABLE dictionary_operations (
        operation_id TEXT PRIMARY KEY CHECK(length(operation_id) = 32),
        operation_type TEXT NOT NULL CHECK(operation_type IN ('install', 'delete')),
        corpus_language TEXT NOT NULL CHECK(length(corpus_language) BETWEEN 1 AND 35),
        audio TEXT NOT NULL CHECK(audio IN ('streamed', 'stored')),
        catalog_revision TEXT,
        phase TEXT NOT NULL CHECK(phase IN (
            'catalog', 'preflight', 'downloading', 'verifying', 'installing',
            'completed', 'cancelled', 'failed', 'interrupted'
        )),
        bytes_completed INTEGER NOT NULL CHECK(bytes_completed >= 0),
        bytes_total INTEGER NOT NULL CHECK(bytes_total >= 0),
        started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        failure TEXT
    ) STRICT;
";

#[cfg(test)]
mod tests {
    use super::CURRENT_SCHEMA;
    use rusqlite::Connection;

    fn user_version(connection: &Connection) -> u32 {
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn schema(connection: &Connection) -> String {
        let mut statement = connection
            .prepare(
                "SELECT type, name, COALESCE(sql, '') FROM sqlite_master
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        rows.map(|row| {
            let (kind, name, sql) = row.unwrap();
            format!(
                "{kind} {name}: {}",
                sql.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    fn migrated_from_zero() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        super::migrate(&connection).unwrap();
        connection
    }

    #[test]
    fn a_fresh_database_reaches_the_current_schema_in_one_pass() {
        assert_eq!(user_version(&migrated_from_zero()), CURRENT_SCHEMA);
    }

    /// Running the ledger again must be a no-op rather than a second set of changes.
    #[test]
    fn migrating_a_current_database_changes_nothing() {
        let connection = migrated_from_zero();
        let before = schema(&connection);
        super::migrate(&connection).unwrap();
        assert_eq!(user_version(&connection), CURRENT_SCHEMA);
        assert_eq!(schema(&connection), before);
    }

    /// A database written by a newer build is refused rather than downgraded in place.
    #[test]
    fn a_future_schema_is_refused() {
        let connection = migrated_from_zero();
        connection
            .pragma_update(None, "user_version", CURRENT_SCHEMA + 1)
            .unwrap();
        let error = super::migrate(&connection).unwrap_err();
        assert!(error.contains("is newer than supported version"), "{error}");
    }

    /// The baseline leaves no broken constraint or unresolved foreign key behind.
    #[test]
    fn the_baseline_is_internally_consistent() {
        let connection = migrated_from_zero();
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let mut statement = connection.prepare("PRAGMA foreign_key_check").unwrap();
        assert_eq!(statement.query_map([], |_| Ok(())).unwrap().count(), 0);
    }

    /// The baseline stamps the header so a library database identifies itself,
    /// which is what `validate_library_identity` checks before migrating.
    #[test]
    fn the_baseline_stamps_the_application_id() {
        let connection = migrated_from_zero();
        let application_id: u32 = connection
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .unwrap();
        assert_eq!(application_id, 0x454c_4c42);
    }

    /// Every stored timestamp is RFC 3339 UTC with milliseconds, so lexicographic
    /// order is chronological order and stored values compare, merge and exchange
    /// without reformatting. `CURRENT_TIMESTAMP` has no zone and only second
    /// resolution, so no column may default to it.
    #[test]
    fn no_timestamp_column_defaults_to_current_timestamp() {
        let connection = migrated_from_zero();
        let mut statement = connection
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table'")
            .unwrap();
        let offenders = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .filter(|(_, sql)| sql.contains("CURRENT_TIMESTAMP"))
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert!(offenders.is_empty(), "{offenders:?}");
    }

    /// The schema a new install ends up with, in full. A diff here means either a
    /// deliberate ledger step or a schema change that forgot one.
    #[test]
    fn the_current_schema_is_exact() {
        insta::assert_snapshot!(schema(&migrated_from_zero()));
    }
}
