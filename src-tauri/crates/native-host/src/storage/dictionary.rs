use crate::http::auth::is_hex_id;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use elephant_ladder_dictionary_pack::PackId;
use rusqlite::{Connection, TransactionBehavior, params};

use super::Storage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DictionaryPackType {
    Corpus,
    Audio,
}

impl DictionaryPackType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Corpus => "corpus",
            Self::Audio => "audio",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "corpus" => Ok(Self::Corpus),
            "audio" => Ok(Self::Audio),
            _ => Err("stored dictionary pack type is invalid".to_owned()),
        }
    }
}

/// Where a language's pronunciations play from, which is the only choice an install takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DictionaryAudioChoice {
    /// Recordings stream from Wikimedia Commons; only their index is installed.
    Streamed,
    /// Recordings are installed with the language and play without a network.
    Stored,
}

impl DictionaryAudioChoice {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Streamed => "streamed",
            Self::Stored => "stored",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "streamed" => Ok(Self::Streamed),
            "stored" => Ok(Self::Stored),
            _ => Err("dictionary audio choice is invalid".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DictionaryViewType {
    Monolingual,
    Bilingual,
}

impl DictionaryViewType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Monolingual => "monolingual",
            Self::Bilingual => "bilingual",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "monolingual" => Ok(Self::Monolingual),
            "bilingual" => Ok(Self::Bilingual),
            _ => Err("stored dictionary view type is invalid".to_owned()),
        }
    }
}

/// An installed pack revision whose directory the host may open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DictionaryInstallation {
    pub pack_id: String,
    pub pack_revision: String,
    pub pack_type: DictionaryPackType,
    pub corpus_language: String,
    pub manifest_sha256: String,
    pub installed_bytes: u64,
    pub directory: PathBuf,
}

pub(crate) struct NewDictionaryInstallation<'a> {
    pub pack_id: &'a str,
    pub pack_revision: &'a str,
    pub pack_type: DictionaryPackType,
    pub corpus_language: &'a str,
    pub manifest_sha256: &'a str,
    pub installed_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DictionaryViewRecord {
    pub view_id: String,
    pub pack_id: String,
    pub view_type: DictionaryViewType,
    pub target_language: Option<String>,
    pub enabled: bool,
}

pub(crate) struct NewDictionaryView<'a> {
    pub view_id: &'a str,
    pub view_type: DictionaryViewType,
    pub target_language: Option<&'a str>,
}

/// How far one acquisition has progressed. Terminal phases keep the row for reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DictionaryOperationPhase {
    Catalog,
    Preflight,
    Downloading,
    Verifying,
    Installing,
    Completed,
    Cancelled,
    Failed,
    /// The process ended before the operation did; its staging directory is kept.
    Interrupted,
}

impl DictionaryOperationPhase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Preflight => "preflight",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Installing => "installing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "catalog" => Ok(Self::Catalog),
            "preflight" => Ok(Self::Preflight),
            "downloading" => Ok(Self::Downloading),
            "verifying" => Ok(Self::Verifying),
            "installing" => Ok(Self::Installing),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err("stored dictionary operation phase is invalid".to_owned()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DictionaryOperation {
    pub operation_id: String,
    pub corpus_language: String,
    pub audio: DictionaryAudioChoice,
    pub phase: DictionaryOperationPhase,
    pub bytes_completed: u64,
    pub bytes_total: u64,
    pub failure: Option<String>,
}

/// The directory one operation downloads into, derived only from a validated operation ID.
pub(crate) fn staging_directory(
    dictionaries: &Path,
    operation_id: &str,
) -> Result<PathBuf, String> {
    if operation_id.len() != 32
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("dictionary operation identifier is invalid".to_owned());
    }
    Ok(dictionaries.join("staging").join(operation_id))
}

/// How much of a validated 64-hex revision names its directory.
///
/// The full revision already appears twice in every asset file name, and repeating it in
/// the directory put installed paths past the Windows `MAX_PATH` limit; a prefix keeps
/// the directory distinct while the registry retains the full revision.
const REVISION_DIRECTORY_HEX: usize = 16;

/// The directory name of one validated pack revision.
pub(crate) fn revision_directory(pack_revision: &str) -> &str {
    &pack_revision[..REVISION_DIRECTORY_HEX.min(pack_revision.len())]
}

/// The directory of one installed pack revision, derived only from validated identity.
pub(crate) fn installed_directory(
    dictionaries: &Path,
    pack_id: &str,
    pack_revision: &str,
) -> Result<PathBuf, String> {
    PackId::from_str(pack_id).map_err(|error| error.to_string())?;
    if !is_revision(pack_revision) {
        return Err("dictionary pack revision is invalid".to_owned());
    }
    Ok(dictionaries
        .join("installed")
        .join(pack_id)
        .join(revision_directory(pack_revision)))
}

fn is_revision(value: &str) -> bool {
    is_hex_id(value)
}

impl Storage {
    pub(crate) fn dictionaries_directory(&self) -> &Path {
        &self.dictionaries
    }

    /// Installed pack revisions in pack order.
    pub(crate) fn dictionary_installations(&self) -> Result<Vec<DictionaryInstallation>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT pack_id, pack_revision, pack_type, corpus_language, manifest_sha256, \
                 installed_bytes FROM dictionary_installations WHERE state = 'installed' \
                 ORDER BY pack_id",
            )
            .map_err(|error| format!("could not prepare dictionary installations: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|error| format!("could not query dictionary installations: {error}"))?;
        rows.map(|row| {
            let (pack_id, pack_revision, pack_type, corpus_language, manifest_sha256, bytes) =
                row.map_err(|error| format!("could not read dictionary installation: {error}"))?;
            Ok(DictionaryInstallation {
                directory: installed_directory(&self.dictionaries, &pack_id, &pack_revision)?,
                pack_type: DictionaryPackType::parse(&pack_type)?,
                installed_bytes: u64::try_from(bytes)
                    .map_err(|_| "stored dictionary size is invalid".to_owned())?,
                pack_id,
                pack_revision,
                corpus_language,
                manifest_sha256,
            })
        })
        .collect()
    }

    /// Views in display order.
    pub(crate) fn dictionary_views(&self) -> Result<Vec<DictionaryViewRecord>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT view_id, pack_id, view_type, target_language, enabled \
                 FROM dictionary_views ORDER BY display_order",
            )
            .map_err(|error| format!("could not prepare dictionary views: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })
            .map_err(|error| format!("could not query dictionary views: {error}"))?;
        rows.map(|row| {
            let (view_id, pack_id, view_type, target_language, enabled) =
                row.map_err(|error| format!("could not read dictionary view: {error}"))?;
            Ok(DictionaryViewRecord {
                view_id,
                pack_id,
                view_type: DictionaryViewType::parse(&view_type)?,
                target_language,
                enabled,
            })
        })
        .collect()
    }

    /// Commits an installed revision whose files are already in its installed directory.
    ///
    /// A previously installed revision of the same pack is marked for deletion. New views
    /// are appended in order; existing views keep their order and enablement.
    pub(crate) fn register_dictionary_installation(
        &mut self,
        installation: &NewDictionaryInstallation<'_>,
        views: &[NewDictionaryView<'_>],
    ) -> Result<(), String> {
        let directory = installed_directory(
            &self.dictionaries,
            installation.pack_id,
            installation.pack_revision,
        )?;
        if !directory.is_dir() {
            return Err("installed dictionary directory does not exist".to_owned());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin dictionary registration: {error}"))?;
        transaction
            .execute(
                "UPDATE dictionary_installations SET state = 'deleting' \
                 WHERE pack_id = ?1 AND pack_revision <> ?2 AND state = 'installed'",
                params![installation.pack_id, installation.pack_revision],
            )
            .map_err(|error| format!("could not retire dictionary revision: {error}"))?;
        transaction
            .execute(
                "INSERT INTO dictionary_installations (pack_id, pack_revision, pack_type, \
                 corpus_language, manifest_sha256, state, installed_bytes) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'installed', ?6) \
                 ON CONFLICT (pack_id, pack_revision) DO UPDATE SET state = 'installed'",
                params![
                    installation.pack_id,
                    installation.pack_revision,
                    installation.pack_type.as_str(),
                    installation.corpus_language,
                    installation.manifest_sha256,
                    i64::try_from(installation.installed_bytes)
                        .map_err(|_| "dictionary size is too large".to_owned())?,
                ],
            )
            .map_err(|error| format!("could not register dictionary installation: {error}"))?;
        for view in views {
            transaction
                .execute(
                    "INSERT INTO dictionary_views (view_id, pack_id, view_type, target_language, \
                     enabled, display_order) \
                     VALUES (?1, ?2, ?3, ?4, 1, \
                     (SELECT coalesce(max(display_order) + 1, 0) FROM dictionary_views)) \
                     ON CONFLICT (view_id) DO NOTHING",
                    params![
                        view.view_id,
                        installation.pack_id,
                        view.view_type.as_str(),
                        view.target_language
                    ],
                )
                .map_err(|error| format!("could not register dictionary view: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit dictionary registration: {error}"))
    }

    /// Records a new operation, or resumes an interrupted one for the same revision.
    ///
    /// Resuming keeps the earlier operation's staging directory, so an install that was
    /// interrupted by a restart continues from the bytes it already verified.
    pub(crate) fn begin_dictionary_operation(
        &mut self,
        operation_id: &str,
        corpus_language: &str,
        audio: DictionaryAudioChoice,
        catalog_revision: Option<&str>,
    ) -> Result<String, String> {
        staging_directory(&self.dictionaries, operation_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin dictionary operation: {error}"))?;
        let active: i64 = transaction
            .query_row(
                "SELECT count(*) FROM dictionary_operations WHERE phase NOT IN \
                 ('completed', 'cancelled', 'failed', 'interrupted')",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("could not count dictionary operations: {error}"))?;
        if active > 0 {
            return Err("a dictionary operation is already active".to_owned());
        }
        let resumable = transaction
            .query_row(
                "SELECT operation_id FROM dictionary_operations \
                 WHERE corpus_language = ?1 AND audio = ?2 AND phase = 'interrupted' \
                 ORDER BY updated_at DESC LIMIT 1",
                params![corpus_language, audio.as_str()],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .filter(|candidate| {
                staging_directory(&self.dictionaries, candidate)
                    .is_ok_and(|directory| directory.is_dir())
            });
        let operation_id = resumable.as_deref().unwrap_or(operation_id);
        transaction
            .execute(
                "INSERT INTO dictionary_operations (operation_id, operation_type, \
                 corpus_language, audio, catalog_revision, phase, bytes_completed, bytes_total) \
                 VALUES (?1, 'install', ?2, ?3, ?4, 'catalog', 0, 0) \
                 ON CONFLICT (operation_id) DO UPDATE SET phase = 'catalog', failure = NULL, \
                 bytes_completed = 0, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![
                    operation_id,
                    corpus_language,
                    audio.as_str(),
                    catalog_revision
                ],
            )
            .map_err(|error| format!("could not record dictionary operation: {error}"))?;
        let operation_id = operation_id.to_owned();
        transaction
            .commit()
            .map_err(|error| format!("could not commit dictionary operation: {error}"))?;
        Ok(operation_id)
    }

    pub(crate) fn update_dictionary_operation(
        &mut self,
        operation_id: &str,
        phase: DictionaryOperationPhase,
        bytes_completed: u64,
        bytes_total: u64,
        failure: Option<&str>,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE dictionary_operations SET phase = ?2, bytes_completed = ?3, \
                 bytes_total = ?4, failure = ?5, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
                 WHERE operation_id = ?1",
                params![
                    operation_id,
                    phase.as_str(),
                    i64::try_from(bytes_completed)
                        .map_err(|_| "dictionary progress is invalid".to_owned())?,
                    i64::try_from(bytes_total)
                        .map_err(|_| "dictionary size is too large".to_owned())?,
                    failure,
                ],
            )
            .map_err(|error| format!("could not update dictionary operation: {error}"))?;
        Ok(())
    }

    /// Operations newest first, including terminal ones the dialog still reports.
    pub(crate) fn dictionary_operations(&self) -> Result<Vec<DictionaryOperation>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT operation_id, corpus_language, audio, phase, bytes_completed, \
                 bytes_total, failure FROM dictionary_operations ORDER BY started_at DESC",
            )
            .map_err(|error| format!("could not prepare dictionary operations: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|error| format!("could not query dictionary operations: {error}"))?;
        rows.map(|row| {
            let (operation_id, corpus_language, audio, phase, completed, total, failure) =
                row.map_err(|error| format!("could not read dictionary operation: {error}"))?;
            Ok(DictionaryOperation {
                operation_id,
                corpus_language,
                audio: DictionaryAudioChoice::parse(&audio)?,
                phase: DictionaryOperationPhase::parse(&phase)?,
                bytes_completed: u64::try_from(completed)
                    .map_err(|_| "stored dictionary progress is invalid".to_owned())?,
                bytes_total: u64::try_from(total)
                    .map_err(|_| "stored dictionary size is invalid".to_owned())?,
                failure,
            })
        })
        .collect()
    }

    /// Marks a revision for deletion and drops its views, keeping its files until unused.
    pub(crate) fn mark_dictionary_installation_deleting(
        &mut self,
        pack_id: &str,
        pack_revision: &str,
    ) -> Result<bool, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("could not begin dictionary deletion: {error}"))?;
        let marked = transaction
            .execute(
                "UPDATE dictionary_installations SET state = 'deleting' \
                 WHERE pack_id = ?1 AND pack_revision = ?2 AND state = 'installed'",
                params![pack_id, pack_revision],
            )
            .map_err(|error| format!("could not mark dictionary deletion: {error}"))?;
        if marked > 0 {
            let remaining: i64 = transaction
                .query_row(
                    "SELECT count(*) FROM dictionary_installations \
                     WHERE pack_id = ?1 AND state = 'installed'",
                    params![pack_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("could not count dictionary revisions: {error}"))?;
            if remaining == 0 {
                transaction
                    .execute(
                        "DELETE FROM dictionary_views WHERE pack_id = ?1",
                        params![pack_id],
                    )
                    .map_err(|error| format!("could not remove dictionary views: {error}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("could not commit dictionary deletion: {error}"))?;
        Ok(marked > 0)
    }
}

/// Clears staging, removes unregistered installed directories, and completes deletions
/// retired before the previous shutdown. Registered revisions whose files are missing
/// stay registered; the host reports them unavailable.
pub(super) fn reconcile_dictionary_storage(
    connection: &Connection,
    dictionaries: &Path,
) -> Result<(), String> {
    let installed = dictionaries.join("installed");
    for directory in [
        dictionaries,
        &dictionaries.join("staging"),
        &installed,
        &dictionaries.join("trash"),
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    }
    // Earlier releases named an installed revision's directory by the full 64-hex
    // revision; it is a prefix now. One rename per registered pack moves an existing
    // installation to the name the derived paths use. A failed rename aborts the open,
    // because the sweeps below would otherwise read the old directory as unregistered
    // and delete it.
    let registered = {
        let mut statement = connection
            .prepare("SELECT pack_id, pack_revision FROM dictionary_installations")
            .map_err(|error| format!("could not prepare dictionary migration: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("could not query dictionary migration: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read dictionary migration: {error}"))?
    };
    for (pack_id, pack_revision) in registered {
        let Ok(current) = installed_directory(dictionaries, &pack_id, &pack_revision) else {
            continue;
        };
        let legacy = installed.join(&pack_id).join(&pack_revision);
        if legacy != current && legacy.is_dir() && !current.exists() {
            fs::rename(&legacy, &current).map_err(|error| {
                format!(
                    "could not rename the dictionary directory {}: {error}",
                    legacy.display()
                )
            })?;
        }
    }

    // An operation that was running when the process ended keeps its staging directory, so a
    // later install resumes from its verified partial files instead of downloading gigabytes
    // again. Every other staging directory is abandoned work.
    connection
        .execute(
            "UPDATE dictionary_operations SET phase = 'interrupted', \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE phase NOT IN \
             ('completed', 'cancelled', 'failed', 'interrupted')",
            [],
        )
        .map_err(|error| format!("could not interrupt dictionary operations: {error}"))?;
    let resumable = {
        let mut statement = connection
            .prepare("SELECT operation_id FROM dictionary_operations WHERE phase = 'interrupted'")
            .map_err(|error| format!("could not prepare dictionary operations: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query dictionary operations: {error}"))?;
        rows.collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|error| format!("could not read dictionary operations: {error}"))?
    };
    for entry in read_directory(&dictionaries.join("staging"))? {
        let kept = entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| resumable.contains(name));
        if !kept {
            remove_directory(dictionaries, &entry)?;
        }
    }
    for entry in read_directory(&dictionaries.join("trash"))? {
        remove_directory(dictionaries, &entry)?;
    }

    let deleting = {
        let mut statement = connection
            .prepare(
                "SELECT pack_id, pack_revision FROM dictionary_installations \
                 WHERE state = 'deleting'",
            )
            .map_err(|error| format!("could not prepare dictionary reconciliation: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("could not query dictionary reconciliation: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read dictionary reconciliation: {error}"))?
    };
    for (pack_id, pack_revision) in deleting {
        if let Ok(directory) = installed_directory(dictionaries, &pack_id, &pack_revision) {
            remove_directory(dictionaries, &directory)?;
        }
        connection
            .execute(
                "DELETE FROM dictionary_installations WHERE pack_id = ?1 AND pack_revision = ?2",
                params![pack_id, pack_revision],
            )
            .map_err(|error| format!("could not complete dictionary deletion: {error}"))?;
    }

    for pack in read_directory(&installed)? {
        let pack_id = pack
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let revisions = if pack.is_dir() {
            read_directory(&pack)?
        } else {
            Vec::new()
        };
        for revision in revisions {
            let pack_revision = revision
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            // Directories carry the revision prefix, so registration is matched on it.
            let registered: bool = connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM dictionary_installations \
                     WHERE pack_id = ?1 AND substr(pack_revision, 1, ?3) = ?2)",
                    params![pack_id, pack_revision, REVISION_DIRECTORY_HEX as i64],
                    |row| row.get(0),
                )
                .map_err(|error| format!("could not check dictionary directory: {error}"))?;
            if !registered {
                remove_directory(dictionaries, &revision)?;
            }
        }
        if read_directory(&pack).map_or(true, |entries| entries.is_empty()) {
            remove_directory(dictionaries, &pack)?;
        }
    }
    Ok(())
}

fn read_directory(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
    entries.sort();
    Ok(entries)
}

/// Removes a path inside the dictionaries directory without following symbolic links.
fn remove_directory(dictionaries: &Path, path: &Path) -> Result<(), String> {
    if !path.starts_with(dictionaries) || path == dictionaries {
        return Err("refusing to remove a path outside dictionary storage".to_owned());
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
    };
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| format!("could not remove {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation<'a>(pack_id: &'a str, revision: &'a str) -> NewDictionaryInstallation<'a> {
        NewDictionaryInstallation {
            pack_id,
            pack_revision: revision,
            pack_type: DictionaryPackType::Corpus,
            corpus_language: "fr",
            manifest_sha256: revision,
            installed_bytes: 10,
        }
    }

    #[test]
    fn startup_renames_full_revision_directories_to_their_prefix_names() {
        let app_data = tempfile::tempdir().unwrap();
        let revision = "4".repeat(64);
        let legacy = {
            let mut storage = Storage::open(app_data.path()).unwrap();
            let current = installed_directory(
                storage.dictionaries_directory(),
                "wiktionary-fr-fr",
                &revision,
            )
            .unwrap();
            fs::create_dir_all(&current).unwrap();
            fs::write(current.join("marker"), b"bytes").unwrap();
            storage
                .register_dictionary_installation(&installation("wiktionary-fr-fr", &revision), &[])
                .unwrap();
            // An installation made by an earlier release sits under its full revision.
            let legacy = storage
                .dictionaries_directory()
                .join("installed/wiktionary-fr-fr")
                .join(&revision);
            fs::rename(&current, &legacy).unwrap();
            legacy
        };

        let storage = Storage::open(app_data.path()).unwrap();
        let current = installed_directory(
            storage.dictionaries_directory(),
            "wiktionary-fr-fr",
            &revision,
        )
        .unwrap();
        assert_eq!(
            current.file_name().unwrap().to_str().unwrap(),
            "4".repeat(16)
        );
        assert!(!legacy.exists());
        assert!(current.join("marker").is_file());
        assert_eq!(storage.dictionary_installations().unwrap().len(), 1);
    }

    #[test]
    fn opening_with_a_bulk_root_moves_dictionary_storage_there() {
        let app_data = tempfile::tempdir().unwrap();
        let bulk = tempfile::tempdir().unwrap();
        let revision = "5".repeat(64);
        {
            let mut storage = Storage::open(app_data.path()).unwrap();
            fs::create_dir_all(
                installed_directory(
                    storage.dictionaries_directory(),
                    "wiktionary-fr-fr",
                    &revision,
                )
                .unwrap(),
            )
            .unwrap();
            storage
                .register_dictionary_installation(&installation("wiktionary-fr-fr", &revision), &[])
                .unwrap();
        }

        // The bulk root itself does not exist yet, as on a first Windows run.
        let bulk_root = bulk.path().join("local-data");
        let storage = Storage::open_with_bulk_data(app_data.path(), &bulk_root).unwrap();
        assert_eq!(storage.dictionaries_directory(), bulk_root.join("dictionaries"));
        assert!(
            installed_directory(
                storage.dictionaries_directory(),
                "wiktionary-fr-fr",
                &revision
            )
            .unwrap()
            .is_dir()
        );
        assert!(!app_data.path().join("dictionaries").exists());
        assert_eq!(storage.dictionary_installations().unwrap().len(), 1);
    }

    #[test]
    fn registration_retires_revisions_and_startup_reconciles_storage() {
        let app_data = tempfile::tempdir().unwrap();
        let first = "1".repeat(64);
        let second = "2".repeat(64);
        let views = [
            NewDictionaryView {
                view_id: "fr",
                view_type: DictionaryViewType::Monolingual,
                target_language: None,
            },
            NewDictionaryView {
                view_id: "fr-en",
                view_type: DictionaryViewType::Bilingual,
                target_language: Some("en"),
            },
        ];
        {
            let mut storage = Storage::open(app_data.path()).unwrap();
            let dictionaries = storage.dictionaries_directory().to_owned();
            for revision in [&first, &second] {
                fs::create_dir_all(
                    installed_directory(&dictionaries, "wiktionary-fr-fr", revision).unwrap(),
                )
                .unwrap();
            }
            storage
                .register_dictionary_installation(&installation("wiktionary-fr-fr", &first), &views)
                .unwrap();
            storage
                .register_dictionary_installation(
                    &installation("wiktionary-fr-fr", &second),
                    &views,
                )
                .unwrap();
            let installed = storage.dictionary_installations().unwrap();
            assert_eq!(installed.len(), 1);
            assert_eq!(installed[0].pack_revision, second);
            assert_eq!(
                storage
                    .dictionary_views()
                    .unwrap()
                    .iter()
                    .map(|view| view.view_id.as_str())
                    .collect::<Vec<_>>(),
                ["fr", "fr-en"]
            );
            assert!(
                storage
                    .register_dictionary_installation(
                        &installation("wiktionary-de-de", &first),
                        &[]
                    )
                    .is_err()
            );
            assert!(installed_directory(&dictionaries, "../escape", &first).is_err());
            assert!(installed_directory(&dictionaries, "wiktionary-fr-fr", "../../x").is_err());

            fs::create_dir_all(
                dictionaries
                    .join("installed/wiktionary-it-it")
                    .join("3".repeat(64)),
            )
            .unwrap();
            fs::write(dictionaries.join("staging/partial.part"), b"partial").unwrap();
            fs::create_dir_all(dictionaries.join("trash/old")).unwrap();
            fs::create_dir_all(
                installed_directory(&dictionaries, "wiktionary-es-es", &first).unwrap(),
            )
            .unwrap();
            storage
                .register_dictionary_installation(&installation("wiktionary-es-es", &first), &[])
                .unwrap();
            fs::remove_dir_all(
                installed_directory(&dictionaries, "wiktionary-es-es", &first).unwrap(),
            )
            .unwrap();
        }

        let storage = Storage::open(app_data.path()).unwrap();
        let dictionaries = storage.dictionaries_directory();
        assert!(
            !installed_directory(dictionaries, "wiktionary-fr-fr", &first)
                .unwrap()
                .exists()
        );
        assert!(
            installed_directory(dictionaries, "wiktionary-fr-fr", &second)
                .unwrap()
                .is_dir()
        );
        assert!(!dictionaries.join("installed/wiktionary-it-it").exists());
        assert!(
            fs::read_dir(dictionaries.join("staging"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(
            fs::read_dir(dictionaries.join("trash"))
                .unwrap()
                .next()
                .is_none()
        );
        let installed = storage.dictionary_installations().unwrap();
        assert_eq!(
            installed
                .iter()
                .map(|installation| (
                    installation.pack_id.as_str(),
                    installation.pack_revision.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("wiktionary-es-es", first.as_str()),
                ("wiktionary-fr-fr", second.as_str())
            ]
        );
    }
}
