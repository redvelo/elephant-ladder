use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use crate::search::{SearchDocumentState, SearchSnapshot, validate_search_snapshot};

pub(crate) const SEARCH_HIGHLIGHT_START: &str = "\u{1e}";
pub(crate) const SEARCH_HIGHLIGHT_END: &str = "\u{1f}";
const MAX_SEARCH_QUERY_OFFSET: u32 = 1_000_000;
const INDEX_DIRECTORY: &str = "search-indexes";

#[derive(Clone, Copy, Debug)]
pub(crate) struct SearchStorageRequest<'a> {
    pub snapshot_id: &'a str,
    pub expected_revision: &'a str,
    pub expression: &'a str,
    pub occurrence: Option<u32>,
    pub resource_path: Option<&'a str>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchStorageRow {
    pub entry_id: String,
    pub resource_path: String,
    pub fragment: Option<String>,
    pub occurrence: u32,
    pub snippet: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchCoverageState {
    Complete,
    Partial,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchStorageCoverage {
    pub state: SearchCoverageState,
    pub searched_occurrences: u32,
    pub total_occurrences: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchStorageReady {
    pub rows: Vec<SearchStorageRow>,
    pub coverage: SearchStorageCoverage,
    pub has_more: bool,
    pub diagnostics: Option<SearchStorageDiagnostics>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SearchStorageDiagnostics {
    pub corpus_entries: u32,
    pub linked_entries: u32,
    pub indexed_entries: u32,
    pub matching_entries: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SearchStorageOutcome {
    Ready(SearchStorageReady),
    IndexUnavailable,
    ScopeUnavailable,
}

/// One SQLite file per publication snapshot under `search-indexes/`, written off the library
/// database's lock and disposable: a missing file is rebuilt in the background on the next open.
pub(crate) struct SearchIndexStore {
    directory: PathBuf,
}

impl SearchIndexStore {
    pub fn open(app_data: &Path) -> Result<Self, String> {
        let directory = app_data.join(INDEX_DIRECTORY);
        fs::create_dir_all(&directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        Ok(Self { directory })
    }

    fn path(&self, snapshot_id: &str) -> Result<PathBuf, String> {
        if snapshot_id.len() != 32 || !snapshot_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("search index snapshot ID is invalid".to_owned());
        }
        Ok(self.directory.join(format!("{snapshot_id}.sqlite")))
    }

    pub fn exists(&self, snapshot_id: &str, revision: &str) -> Result<bool, String> {
        let path = self.path(snapshot_id)?;
        if !path.is_file() {
            return Ok(false);
        }
        let connection = open_read_only(&path)?;
        index_revision(&connection, snapshot_id).map(|stored| stored.as_deref() == Some(revision))
    }

    pub fn write(
        &self,
        snapshot_id: &str,
        revision: &str,
        search: &SearchSnapshot,
    ) -> Result<(), String> {
        if revision.is_empty() || revision.len() > 64 {
            return Err("search revision is invalid".to_owned());
        }
        validate_search_snapshot(search)?;
        let path = self.path(snapshot_id)?;
        let staging = self.directory.join(format!("{snapshot_id}.sqlite.tmp"));
        let _ = fs::remove_file(&staging);
        let result = (|| {
            let mut connection = Connection::open(&staging)
                .map_err(|error| format!("could not create search index: {error}"))?;
            connection
                .execute_batch(
                    "PRAGMA journal_mode = OFF;
                     PRAGMA synchronous = OFF;
                     PRAGMA foreign_keys = ON;",
                )
                .map_err(|error| format!("could not configure search index: {error}"))?;
            connection
                .execute_batch(INDEX_SCHEMA)
                .map_err(|error| format!("could not create search index schema: {error}"))?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| format!("could not begin search index: {error}"))?;
            materialize(&transaction, snapshot_id, revision, search)?;
            transaction
                .commit()
                .map_err(|error| format!("could not commit search index: {error}"))?;
            drop(connection);
            fs::rename(&staging, &path)
                .map_err(|error| format!("could not install search index: {error}"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&staging);
        }
        result
    }

    pub fn remove(&self, snapshot_id: &str) -> Result<(), String> {
        let path = self.path(snapshot_id)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove search index: {error}")),
        }
    }

    /// Removes every index and staging file whose snapshot is not in `live`.
    pub fn retain(&self, live: &HashSet<String>) -> Result<(), String> {
        let entries = fs::read_dir(&self.directory)
            .map_err(|error| format!("could not list search indexes: {error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("could not list search indexes: {error}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let snapshot_id = name
                .strip_suffix(".sqlite")
                .or_else(|| name.strip_suffix(".sqlite.tmp"));
            let keep = snapshot_id.is_some_and(|id| name.ends_with(".sqlite") && live.contains(id));
            if !keep {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    pub fn search(
        &self,
        request: SearchStorageRequest<'_>,
    ) -> Result<SearchStorageOutcome, String> {
        let SearchStorageRequest {
            snapshot_id,
            expected_revision,
            expression,
            occurrence,
            resource_path,
            offset,
            limit,
        } = request;
        if expression.is_empty()
            || expression.len() > 16_384
            || resource_path.is_some_and(|path| path.is_empty() || path.len() > 16_384)
            || limit == 0
            || limit > 100
            || offset > MAX_SEARCH_QUERY_OFFSET
        {
            return Err("invalid search query bounds".to_owned());
        }
        let path = self.path(snapshot_id)?;
        if !path.is_file() {
            return Ok(SearchStorageOutcome::IndexUnavailable);
        }
        let connection = open_read_only(&path)?;
        if index_revision(&connection, snapshot_id)?.as_deref() != Some(expected_revision) {
            return Ok(SearchStorageOutcome::IndexUnavailable);
        }

        if occurrence.is_some() || resource_path.is_some() {
            let available = connection
                .query_row(
                    "SELECT 1 FROM search_document_occurrences
                     WHERE snapshot_id = ?1
                       AND (?2 IS NULL OR occurrence_index = ?2)
                       AND (?3 IS NULL OR resource_path = ?3) LIMIT 1",
                    params![snapshot_id, occurrence, resource_path],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("could not validate publication search scope: {error}"))?
                .is_some();
            if !available {
                return Ok(SearchStorageOutcome::ScopeUnavailable);
            }
        }

        let (total_occurrences, searched_occurrences) = connection
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(d.state IN ('complete', 'inapplicable')), 0)
                 FROM search_document_occurrences o
                 JOIN search_documents d
                   ON d.snapshot_id = o.snapshot_id AND d.resource_path = o.resource_path
                 WHERE o.snapshot_id = ?1
                   AND (?2 IS NULL OR o.occurrence_index = ?2)
                   AND (?3 IS NULL OR o.resource_path = ?3)",
                params![snapshot_id, occurrence, resource_path],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
            )
            .map_err(|error| format!("could not read publication search coverage: {error}"))?;

        let snapshot_expression = snapshot_expression(snapshot_id);
        let fetch_limit = limit + 1;
        let mut statement = connection
            .prepare(
                "SELECT e.entry_id, e.resource_path, e.fragment, o.occurrence_index,
                        snippet(search_entries_fts, 1, ?5, ?6, '...', 24)
                 FROM search_entries_fts
                 JOIN search_entries e ON e.entry_key = search_entries_fts.rowid
                 JOIN search_entry_occurrences o ON o.entry_key = e.entry_key
                 WHERE search_entries_fts MATCH ?1
                   AND search_entries_fts MATCH ?2
                   AND e.snapshot_id = ?3
                   AND (?7 IS NULL OR o.occurrence_index = ?7)
                   AND (?8 IS NULL OR e.resource_path = ?8)
                 ORDER BY o.occurrence_index, e.entry_order
                 LIMIT ?4 OFFSET ?9",
            )
            .map_err(|error| format!("could not prepare publication search: {error}"))?;
        let mut rows = statement
            .query(params![
                expression,
                snapshot_expression,
                snapshot_id,
                fetch_limit,
                SEARCH_HIGHLIGHT_START,
                SEARCH_HIGHLIGHT_END,
                occurrence,
                resource_path,
                offset
            ])
            .map_err(|error| format!("could not execute publication search: {error}"))?;
        let mut results = Vec::with_capacity(limit as usize + 1);
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("could not read publication search result: {error}"))?
        {
            results.push(SearchStorageRow {
                entry_id: row
                    .get(0)
                    .map_err(|error| format!("invalid search entry ID: {error}"))?,
                resource_path: row
                    .get(1)
                    .map_err(|error| format!("invalid search resource path: {error}"))?,
                fragment: row
                    .get(2)
                    .map_err(|error| format!("invalid search fragment: {error}"))?,
                occurrence: row
                    .get(3)
                    .map_err(|error| format!("invalid search occurrence: {error}"))?,
                snippet: row
                    .get(4)
                    .map_err(|error| format!("invalid search snippet: {error}"))?,
            });
        }
        let has_more = results.len() > limit as usize;
        results.truncate(limit as usize);
        let diagnostics = (offset == 0 && results.is_empty())
            .then(|| diagnostics(&connection, snapshot_id, expression))
            .transpose()?;
        Ok(SearchStorageOutcome::Ready(SearchStorageReady {
            rows: results,
            coverage: SearchStorageCoverage {
                state: if searched_occurrences == total_occurrences {
                    SearchCoverageState::Complete
                } else {
                    SearchCoverageState::Partial
                },
                searched_occurrences,
                total_occurrences,
            },
            has_more,
            diagnostics,
        }))
    }
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("could not open search index: {error}"))?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|error| format!("could not configure search index: {error}"))?;
    Ok(connection)
}

fn index_revision(connection: &Connection, snapshot_id: &str) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT revision FROM search_indexes WHERE snapshot_id = ?1",
            [snapshot_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("could not read the search index revision: {error}"))
}

fn snapshot_expression(snapshot_id: &str) -> String {
    format!("snapshot_id : \"{}\"", snapshot_id.replace('"', "\"\""))
}

fn materialize(
    transaction: &rusqlite::Transaction<'_>,
    snapshot_id: &str,
    revision: &str,
    search: &SearchSnapshot,
) -> Result<(), String> {
    let effective_reading_order_resources = &search.effective_reading_order_resources;
    transaction
        .execute(
            "INSERT INTO search_indexes (snapshot_id, revision) VALUES (?1, ?2)",
            params![snapshot_id, revision],
        )
        .map_err(|error| format!("could not create publication search index: {error}"))?;

    for (document_order, (resource_path, state)) in search.documents.iter().enumerate() {
        let state = match state {
            SearchDocumentState::Complete => "complete",
            SearchDocumentState::Partial => "partial",
            SearchDocumentState::Unavailable => "unavailable",
            SearchDocumentState::Inapplicable => "inapplicable",
        };
        transaction
            .execute(
                "INSERT INTO search_documents
                    (snapshot_id, resource_path, document_order, state)
                 VALUES (?1, ?2, ?3, ?4)",
                params![snapshot_id, resource_path, document_order, state],
            )
            .map_err(|error| format!("could not store search document: {error}"))?;
    }

    for (occurrence, resource_path) in effective_reading_order_resources.iter().enumerate() {
        let Some(resource_path) = resource_path else {
            continue;
        };
        transaction
            .execute(
                "INSERT INTO search_document_occurrences
                    (snapshot_id, occurrence_index, resource_path)
                 VALUES (?1, ?2, ?3)",
                params![snapshot_id, occurrence, resource_path],
            )
            .map_err(|error| format!("could not store search document occurrence: {error}"))?;
    }

    for (entry_order, entry) in search.entries.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO search_entries
                    (snapshot_id, entry_order, entry_id, resource_path, fragment, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    snapshot_id,
                    entry_order,
                    entry.id,
                    entry.resource_path,
                    entry.fragment,
                    entry.text
                ],
            )
            .map_err(|error| format!("could not store search entry: {error}"))?;
        let entry_key = transaction.last_insert_rowid();
        for occurrence in &entry.reading_order_occurrence_indexes {
            let mapped_resource = effective_reading_order_resources
                .get(*occurrence)
                .and_then(Option::as_deref)
                .ok_or_else(|| "search entry refers to an unavailable occurrence".to_owned())?;
            if mapped_resource != entry.resource_path {
                return Err("search entry occurrence has a different resource".to_owned());
            }
            transaction
                .execute(
                    "INSERT INTO search_entry_occurrences
                        (entry_key, snapshot_id, resource_path, occurrence_index)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![entry_key, snapshot_id, entry.resource_path, occurrence],
                )
                .map_err(|error| format!("could not store search entry occurrence: {error}"))?;
        }
    }
    Ok(())
}

fn diagnostics(
    connection: &Connection,
    snapshot_id: &str,
    expression: &str,
) -> Result<SearchStorageDiagnostics, String> {
    let snapshot_expression = snapshot_expression(snapshot_id);
    let count = |sql: &str, parameters: &[&dyn rusqlite::ToSql]| {
        connection
            .query_row(sql, parameters, |row| row.get::<_, u32>(0))
            .map_err(|error| format!("could not diagnose empty publication search: {error}"))
    };
    Ok(SearchStorageDiagnostics {
        corpus_entries: count(
            "SELECT COUNT(*) FROM search_entries WHERE snapshot_id = ?1",
            &[&snapshot_id],
        )?,
        linked_entries: count(
            "SELECT COUNT(DISTINCT entry_key) FROM search_entry_occurrences WHERE snapshot_id = ?1",
            &[&snapshot_id],
        )?,
        indexed_entries: count(
            "SELECT COUNT(*) FROM search_entries_fts WHERE search_entries_fts MATCH ?1",
            &[&snapshot_expression],
        )?,
        matching_entries: count(
            "SELECT COUNT(*) FROM search_entries_fts
             WHERE search_entries_fts MATCH ?1 AND search_entries_fts MATCH ?2",
            &[&expression, &snapshot_expression],
        )?,
    })
}

const INDEX_SCHEMA: &str = "
    CREATE TABLE search_indexes (
        snapshot_id TEXT PRIMARY KEY CHECK(length(snapshot_id) = 32),
        revision TEXT NOT NULL CHECK(length(revision) BETWEEN 1 AND 64)
    ) STRICT;
    CREATE TABLE search_documents (
        snapshot_id TEXT NOT NULL REFERENCES search_indexes(snapshot_id) ON DELETE CASCADE,
        resource_path TEXT NOT NULL CHECK(
            length(CAST(resource_path AS BLOB)) BETWEEN 1 AND 16384
        ),
        document_order INTEGER NOT NULL CHECK(document_order >= 0),
        state TEXT NOT NULL CHECK(
            state IN ('complete', 'partial', 'unavailable', 'inapplicable')
        ),
        PRIMARY KEY(snapshot_id, resource_path),
        UNIQUE(snapshot_id, document_order)
    ) STRICT;
    CREATE TABLE search_document_occurrences (
        snapshot_id TEXT NOT NULL,
        occurrence_index INTEGER NOT NULL CHECK(occurrence_index >= 0),
        resource_path TEXT NOT NULL,
        PRIMARY KEY(snapshot_id, occurrence_index),
        FOREIGN KEY(snapshot_id, resource_path)
            REFERENCES search_documents(snapshot_id, resource_path) ON DELETE CASCADE
    ) STRICT;
    CREATE INDEX search_document_occurrences_resource
        ON search_document_occurrences(snapshot_id, resource_path, occurrence_index);
    CREATE TABLE search_entries (
        entry_key INTEGER PRIMARY KEY,
        snapshot_id TEXT NOT NULL REFERENCES search_indexes(snapshot_id) ON DELETE CASCADE,
        entry_order INTEGER NOT NULL CHECK(entry_order >= 0),
        entry_id TEXT NOT NULL CHECK(
            length(CAST(entry_id AS BLOB)) BETWEEN 1 AND 16384
        ),
        resource_path TEXT NOT NULL,
        fragment TEXT CHECK(
            fragment IS NULL OR length(CAST(fragment AS BLOB)) <= 16384
        ),
        text TEXT NOT NULL CHECK(
            length(CAST(text AS BLOB)) BETWEEN 1 AND 1048576
        ),
        UNIQUE(snapshot_id, entry_order),
        UNIQUE(snapshot_id, entry_id),
        UNIQUE(entry_key, snapshot_id, resource_path),
        FOREIGN KEY(snapshot_id, resource_path)
            REFERENCES search_documents(snapshot_id, resource_path) ON DELETE CASCADE
    ) STRICT;
    CREATE TABLE search_entry_occurrences (
        entry_key INTEGER NOT NULL,
        snapshot_id TEXT NOT NULL,
        resource_path TEXT NOT NULL,
        occurrence_index INTEGER NOT NULL,
        PRIMARY KEY(entry_key, occurrence_index),
        FOREIGN KEY(entry_key, snapshot_id, resource_path)
            REFERENCES search_entries(entry_key, snapshot_id, resource_path) ON DELETE CASCADE,
        FOREIGN KEY(snapshot_id, occurrence_index)
            REFERENCES search_document_occurrences(snapshot_id, occurrence_index) ON DELETE CASCADE
    ) STRICT;
    CREATE INDEX search_entry_occurrences_order
        ON search_entry_occurrences(snapshot_id, occurrence_index, entry_key);
    CREATE VIRTUAL TABLE search_entries_fts USING fts5(
        snapshot_id, text,
        content = 'search_entries', content_rowid = 'entry_key',
        tokenize = 'unicode61'
    );
    CREATE TRIGGER search_entries_fts_insert
    AFTER INSERT ON search_entries BEGIN
        INSERT INTO search_entries_fts(rowid, snapshot_id, text)
        VALUES (new.entry_key, new.snapshot_id, new.text);
    END;
";

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        SEARCH_HIGHLIGHT_END, SEARCH_HIGHLIGHT_START, SearchCoverageState, SearchIndexStore,
        SearchStorageDiagnostics, SearchStorageOutcome, SearchStorageRequest,
    };
    use crate::search::SearchSnapshot;

    const SNAPSHOT: &str = "0123456789abcdef0123456789abcdef";

    fn searchable_snapshot() -> SearchSnapshot {
        serde_json::from_value(serde_json::json!({
            "entries": [
                {
                    "id": "chapter.xhtml#second",
                    "resourcePath": "chapter.xhtml",
                    "fragment": "second",
                    "text": "The second elephant crosses the ladder.",
                    "readingOrderOccurrenceIndexes": [0]
                },
                {
                    "id": "chapter.xhtml#first",
                    "resourcePath": "chapter.xhtml",
                    "fragment": "first",
                    "text": "An elephant waits beside the ladder.",
                    "readingOrderOccurrenceIndexes": [0]
                }
            ],
            "documents": {
                "chapter.xhtml": "complete",
                "image.svg": "inapplicable"
            },
            "effectiveReadingOrderResources": ["chapter.xhtml", "image.svg"]
        }))
        .unwrap()
    }

    fn request<'a>(
        expression: &'a str,
        revision: &'a str,
        occurrence: Option<u32>,
        resource_path: Option<&'a str>,
        limit: u32,
    ) -> SearchStorageRequest<'a> {
        SearchStorageRequest {
            snapshot_id: SNAPSHOT,
            expected_revision: revision,
            expression,
            occurrence,
            resource_path,
            offset: 0,
            limit,
        }
    }

    #[test]
    fn reopens_and_replaces_the_previous_chunk_index() {
        let directory = tempfile::tempdir().unwrap();
        let store = SearchIndexStore::open(directory.path()).unwrap();
        store
            .write(SNAPSHOT, "search-v3", &searchable_snapshot())
            .unwrap();
        drop(store);
        let store = SearchIndexStore::open(directory.path()).unwrap();
        assert!(!store.exists(SNAPSHOT, crate::SEARCH_REVISION).unwrap());
        assert!(matches!(
            store
                .search(request("elephant", crate::SEARCH_REVISION, None, None, 10))
                .unwrap(),
            SearchStorageOutcome::IndexUnavailable
        ));
        let mut snapshot = searchable_snapshot();
        snapshot.entries[0].text = "Hello world again".to_owned();
        store
            .write(SNAPSHOT, crate::SEARCH_REVISION, &snapshot)
            .unwrap();
        assert!(store.exists(SNAPSHOT, crate::SEARCH_REVISION).unwrap());
        let SearchStorageOutcome::Ready(results) = store
            .search(request(
                "\"Hello world\"",
                crate::SEARCH_REVISION,
                None,
                None,
                10,
            ))
            .unwrap()
        else {
            panic!("rebuilt index is unavailable")
        };
        assert_eq!(results.rows.len(), 1);
    }

    #[test]
    fn indexes_and_queries_one_snapshot_file_in_publication_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = SearchIndexStore::open(directory.path()).unwrap();
        assert!(!store.exists(SNAPSHOT, "search-v2").unwrap());
        assert!(matches!(
            store
                .search(request("elephant", "search-v2", None, None, 10))
                .unwrap(),
            SearchStorageOutcome::IndexUnavailable
        ));

        store
            .write(SNAPSHOT, "search-v2", &searchable_snapshot())
            .unwrap();
        assert!(store.exists(SNAPSHOT, "search-v2").unwrap());
        assert!(!store.exists(SNAPSHOT, "search-v1").unwrap());
        assert!(
            directory
                .path()
                .join("search-indexes")
                .join(format!("{SNAPSHOT}.sqlite"))
                .is_file()
        );

        let SearchStorageOutcome::Ready(ready) = store
            .search(request("elephant", "search-v2", None, None, 1))
            .unwrap()
        else {
            panic!("search index must be ready");
        };
        assert_eq!(ready.rows.len(), 1);
        assert_eq!(ready.rows[0].entry_id, "chapter.xhtml#second");
        assert_eq!(ready.rows[0].occurrence, 0);
        assert!(ready.rows[0].snippet.contains(SEARCH_HIGHLIGHT_START));
        assert!(ready.rows[0].snippet.contains(SEARCH_HIGHLIGHT_END));
        assert!(ready.has_more);
        assert_eq!(ready.diagnostics, None);
        assert_eq!(ready.coverage.state, SearchCoverageState::Complete);
        assert_eq!(ready.coverage.searched_occurrences, 2);
        assert_eq!(ready.coverage.total_occurrences, 2);

        let SearchStorageOutcome::Ready(empty) = store
            .search(request("\"rabbit\"*", "search-v2", None, None, 10))
            .unwrap()
        else {
            panic!("search index must be ready");
        };
        assert!(empty.rows.is_empty());
        assert_eq!(
            empty.diagnostics,
            Some(SearchStorageDiagnostics {
                corpus_entries: 2,
                linked_entries: 2,
                indexed_entries: 2,
                matching_entries: 0,
            })
        );

        let SearchStorageOutcome::Ready(scoped) = store
            .search(request(
                "elephant",
                "search-v2",
                Some(1),
                Some("image.svg"),
                10,
            ))
            .unwrap()
        else {
            panic!("inapplicable occurrence scope must still be available");
        };
        assert!(scoped.rows.is_empty());
        assert_eq!(scoped.coverage.searched_occurrences, 1);
        assert!(matches!(
            store
                .search(request(
                    "elephant",
                    "search-v2",
                    Some(0),
                    Some("image.svg"),
                    10
                ))
                .unwrap(),
            SearchStorageOutcome::ScopeUnavailable
        ));
        assert!(matches!(
            store
                .search(request("elephant", "search-v1", None, None, 10))
                .unwrap(),
            SearchStorageOutcome::IndexUnavailable
        ));
    }

    #[test]
    fn rewrites_removes_and_sweeps_snapshot_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = SearchIndexStore::open(directory.path()).unwrap();
        let other = "fedcba9876543210fedcba9876543210";
        store
            .write(SNAPSHOT, "search-v1", &searchable_snapshot())
            .unwrap();
        store
            .write(SNAPSHOT, "search-v2", &searchable_snapshot())
            .unwrap();
        store
            .write(other, "search-v2", &searchable_snapshot())
            .unwrap();
        assert!(store.exists(SNAPSHOT, "search-v2").unwrap());
        std::fs::write(
            directory
                .path()
                .join("search-indexes")
                .join("00000000000000000000000000000000.sqlite.tmp"),
            b"stale",
        )
        .unwrap();

        store.retain(&HashSet::from([SNAPSHOT.to_owned()])).unwrap();
        assert!(store.exists(SNAPSHOT, "search-v2").unwrap());
        assert!(!store.exists(other, "search-v2").unwrap());
        assert_eq!(
            std::fs::read_dir(directory.path().join("search-indexes"))
                .unwrap()
                .count(),
            1
        );

        store.remove(SNAPSHOT).unwrap();
        store.remove(SNAPSHOT).unwrap();
        assert!(!store.exists(SNAPSHOT, "search-v2").unwrap());
        assert!(store.path("not-hex").is_err());
    }

    #[test]
    fn rejects_a_projection_unrelated_to_its_reading_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = SearchIndexStore::open(directory.path()).unwrap();
        let mut search = serde_json::to_value(searchable_snapshot()).unwrap();
        search["entries"][0]["readingOrderOccurrenceIndexes"] = serde_json::json!([1]);
        let search: SearchSnapshot = serde_json::from_value(search).unwrap();
        assert_eq!(
            store.write(SNAPSHOT, "search-v2", &search).unwrap_err(),
            "search entry occurrence has a different effective resource"
        );
        assert!(!store.exists(SNAPSHOT, "search-v2").unwrap());
        assert!(
            !directory
                .path()
                .join("search-indexes")
                .join(format!("{SNAPSHOT}.sqlite.tmp"))
                .exists()
        );
    }
}
