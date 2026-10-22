//! Startup reconciliation and repair.
//!
//! Startup removes abandoned staging files, releases expired reservations,
//! checks committed blob presence, identity and size, resumes deletion and blob
//! cleanup, and recomputes storage accounting. It deliberately does not hash
//! every unchanged archive: full verification happens before same-digest reuse
//! and after a relevant filesystem change or read failure. Unknown files are
//! never silently adopted as publications.

use crate::storage::fs;
use rusqlite::{Connection, Transaction, params};
use std::{fs::File, io::Read, path::Path};

pub(crate) fn consume_import_reservation(
    transaction: &Transaction<'_>,
    reservation_id: Option<&str>,
) -> Result<(), String> {
    let Some(reservation_id) = reservation_id else {
        return Ok(());
    };
    let removed = transaction
        .execute(
            "DELETE FROM import_reservations WHERE reservation_id = ?1",
            [reservation_id],
        )
        .map_err(|error| format!("could not consume import reservation: {error}"))?;
    if removed != 1 {
        return Err("import reservation is unavailable".to_owned());
    }
    Ok(())
}

/// One committed blob as the library records it.
struct KnownBlob {
    hash: String,
    storage_name: String,
    size_bytes: u64,
    health_state: String,
    lifecycle_state: String,
}

impl KnownBlob {
    /// The locations this blob's file may legitimately occupy, most likely first.
    ///
    /// Lifecycle says where it belongs — a trashed blob has been moved out of
    /// `blobs/` deliberately — but a blob is mid-move whenever the host stopped
    /// between renaming the file and committing the new lifecycle, so both places
    /// count. Health is a statement about the bytes existing at the right size, not
    /// about which directory currently holds them. The two columns stay orthogonal,
    /// and every combination of them is meaningful, which is why no constraint
    /// couples them.
    fn locations<'a>(&self, blobs: &'a Path, trash: &'a Path) -> [&'a Path; 2] {
        if self.lifecycle_state == "trashed" {
            [trash, blobs]
        } else {
            [blobs, trash]
        }
    }

    /// Whether the name is usable as a path component at all.
    fn addressable(&self) -> bool {
        self.hash == self.storage_name && is_blake3_hash(&self.hash)
    }
}

pub(crate) fn reconcile_archive_storage(
    connection: &mut Connection,
    blobs: &Path,
    covers: &Path,
    staging: &Path,
    trash: &Path,
) -> Result<(), String> {
    // Imports are not resumable, so every reservation and staging artifact is abandoned
    // when a new host process opens the library.
    for entry in fs::read_dir(staging)
        .map_err(|error| format!("could not inspect archive staging directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("could not inspect staging entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("could not inspect staging entry type: {error}"))?;
        if !file_type.is_file() {
            return Err(format!(
                "unexpected non-file archive staging entry: {}",
                entry.path().display()
            ));
        }
        fs::remove_file(entry.path())
            .map_err(|error| format!("could not remove abandoned staging file: {error}"))?;
    }
    sync_directory(staging)?;
    connection
        .execute("DELETE FROM import_reservations", [])
        .map_err(|error| format!("could not release abandoned import reservations: {error}"))?;

    let known_blobs = {
        let mut statement = connection
            .prepare(
                "SELECT blob_hash, storage_name, size_bytes, health_state, lifecycle_state
                 FROM archive_blobs",
            )
            .map_err(|error| format!("could not prepare archive reconciliation: {error}"))?;
        statement
            .query_map([], |row| {
                Ok(KnownBlob {
                    hash: row.get(0)?,
                    storage_name: row.get(1)?,
                    size_bytes: row.get(2)?,
                    health_state: row.get(3)?,
                    lifecycle_state: row.get(4)?,
                })
            })
            .map_err(|error| format!("could not query archive reconciliation: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read archive reconciliation: {error}"))?
    };
    let known_names = known_blobs
        .iter()
        .filter(|blob| blob.addressable())
        .map(|blob| blob.storage_name.as_str())
        .collect::<std::collections::HashSet<_>>();
    for entry in fs::read_dir(blobs)
        .map_err(|error| format!("could not inspect archive blob directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect archive blob entry: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "archive blob name is not UTF-8".to_owned())?;
        if !known_names.contains(name) {
            let file_type = entry
                .file_type()
                .map_err(|error| format!("could not inspect archive blob type: {error}"))?;
            if !file_type.is_file() {
                return Err(format!(
                    "unexpected non-file archive blob entry: {}",
                    entry.path().display()
                ));
            }
            fs::remove_file(entry.path())
                .map_err(|error| format!("could not remove unreferenced archive blob: {error}"))?;
        }
    }
    sync_directory(blobs)?;

    // A file in the trash directory that no row claims is swept the same way an
    // unknown blob is: the library is the only thing that decides what an archive
    // file is, in either location.
    for entry in fs::read_dir(trash)
        .map_err(|error| format!("could not inspect archive trash directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect archive trash entry: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "archive trash name is not UTF-8".to_owned())?;
        if !known_names.contains(name) {
            let file_type = entry
                .file_type()
                .map_err(|error| format!("could not inspect archive trash type: {error}"))?;
            if !file_type.is_file() {
                return Err(format!(
                    "unexpected non-file archive trash entry: {}",
                    entry.path().display()
                ));
            }
            fs::remove_file(entry.path())
                .map_err(|error| format!("could not remove unreferenced trashed blob: {error}"))?;
        }
    }
    sync_directory(trash)?;

    for blob in known_blobs {
        let reconciled_health = if !blob.addressable() {
            "corrupt"
        } else {
            let mut observed = "missing";
            for directory in blob.locations(blobs, trash) {
                match fs::metadata(directory.join(&blob.storage_name)) {
                    Ok(metadata) if metadata.is_file() && metadata.len() == blob.size_bytes => {
                        observed = blob.health_state.as_str();
                        break;
                    }
                    Ok(_) => observed = "corrupt",
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("could not inspect committed archive blob: {error}"));
                    }
                }
            }
            observed
        };
        if reconciled_health != blob.health_state {
            connection
                .execute(
                    "UPDATE archive_blobs SET health_state = ?2 WHERE blob_hash = ?1",
                    params![blob.hash, reconciled_health],
                )
                .map_err(|error| format!("could not update archive blob health: {error}"))?;
        }
    }

    remove_unreferenced_covers(connection, covers)?;
    Ok(())
}

pub(crate) fn remove_unreferenced_covers(
    connection: &Connection,
    covers: &Path,
) -> Result<(), String> {
    let referenced_covers = {
        let mut statement = connection
            .prepare("SELECT DISTINCT cover_hash FROM publication_snapshots WHERE cover_hash IS NOT NULL")
            .map_err(|error| format!("could not prepare cover reconciliation: {error}"))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query cover reconciliation: {error}"))?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|error| format!("could not read cover reconciliation: {error}"))?
    };
    for entry in fs::read_dir(covers)
        .map_err(|error| format!("could not inspect cover directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("could not inspect cover entry: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "cover artifact name is not UTF-8".to_owned())?;
        if !referenced_covers.contains(name) {
            let file_type = entry
                .file_type()
                .map_err(|error| format!("could not inspect cover artifact type: {error}"))?;
            if !file_type.is_file() {
                return Err(format!(
                    "unexpected non-file cover entry: {}",
                    entry.path().display()
                ));
            }
            fs::remove_file(entry.path()).map_err(|error| {
                format!("could not remove unreferenced cover artifact: {error}")
            })?;
        }
    }
    sync_directory(covers)?;
    Ok(())
}

/// Deletes timeline files no live snapshot references, the same way sidecars are reconciled.
pub(crate) fn reconcile_media_timeline_storage(
    connection: &mut Connection,
    media_timelines: &Path,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM media_overlay_timeline_files
             WHERE snapshot_id NOT IN (SELECT snapshot_id FROM publication_snapshots)",
            [],
        )
        .map_err(|error| format!("could not reconcile orphaned media timeline rows: {error}"))?;
    let referenced = {
        let mut statement = connection
            .prepare("SELECT DISTINCT storage_name FROM media_overlay_timeline_files")
            .map_err(|error| format!("could not prepare media timeline reconciliation: {error}"))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query media timeline reconciliation: {error}"))?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|error| format!("could not read media timeline reconciliation: {error}"))?
    };
    for entry in fs::read_dir(media_timelines)
        .map_err(|error| format!("could not inspect media timeline directory: {error}"))?
    {
        let entry = entry
            .map_err(|error| format!("could not inspect media timeline directory: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !referenced.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

pub(crate) fn reconcile_media_sidecar_storage(
    connection: &mut Connection,
    media_sidecars: &Path,
) -> Result<(), String> {
    // A sidecar row whose owning snapshot no longer exists is derived cache data that outlived
    // its snapshot; the foreign key normally cascades this away, but startup reconciliation
    // stays defensive against any row that predates the constraint or a partial write.
    connection
        .execute(
            "DELETE FROM media_overlay_sidecars
             WHERE snapshot_id NOT IN (SELECT snapshot_id FROM publication_snapshots)",
            [],
        )
        .map_err(|error| format!("could not reconcile orphaned media sidecar rows: {error}"))?;
    remove_unreferenced_media_sidecars(connection, media_sidecars)
}

pub(crate) fn remove_unreferenced_media_sidecars(
    connection: &Connection,
    media_sidecars: &Path,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM media_sidecar_transcodes
             WHERE storage_name NOT IN (SELECT storage_name FROM media_overlay_sidecars)",
            [],
        )
        .map_err(|error| format!("could not reconcile orphaned media transcode rows: {error}"))?;

    let referenced_sidecars = {
        let mut statement = connection
            .prepare("SELECT DISTINCT storage_name FROM media_overlay_sidecars")
            .map_err(|error| format!("could not prepare media sidecar reconciliation: {error}"))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("could not query media sidecar reconciliation: {error}"))?
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|error| format!("could not read media sidecar reconciliation: {error}"))?
    };
    for entry in fs::read_dir(media_sidecars)
        .map_err(|error| format!("could not inspect media sidecar directory: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("could not inspect media sidecar entry: {error}"))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| "media sidecar artifact name is not UTF-8".to_owned())?;
        if !referenced_sidecars.contains(name) {
            let file_type = entry.file_type().map_err(|error| {
                format!("could not inspect media sidecar artifact type: {error}")
            })?;
            if !file_type.is_file() {
                return Err(format!(
                    "unexpected non-file media sidecar entry: {}",
                    entry.path().display()
                ));
            }
            fs::remove_file(entry.path()).map_err(|error| {
                format!("could not remove unreferenced media sidecar artifact: {error}")
            })?;
        }
    }
    sync_directory(media_sidecars)?;
    Ok(())
}

/// A committed blob is named by its BLAKE3 digest, and [`verify_blob`] recomputes BLAKE3
/// against that name, so the algorithm is part of this contract rather than a label.
pub(crate) fn is_blake3_hash(value: &str) -> bool {
    value.len() == blake3::OUT_LEN * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(crate) fn verify_blob(
    path: &Path,
    expected_hash: &str,
    expected_size: u64,
) -> Result<(), String> {
    let mut file =
        File::open(path).map_err(|error| format!("could not open archive blob: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect archive blob: {error}"))?;
    if metadata.len() != expected_size {
        return Err("existing archive blob has the wrong size".to_owned());
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not verify archive blob: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hasher.finalize().to_hex().as_str() != expected_hash {
        return Err("existing archive blob failed BLAKE3 verification".to_owned());
    }
    Ok(())
}

pub(crate) fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove {}: {error}", path.display())),
    }
}

/// Unix persists a rename or a create by syncing the directory holding it. Windows has no
/// equivalent operation and will not open a directory as a file at all, so there is nothing
/// to flush there.
#[cfg(unix)]
pub fn fsync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
pub fn fsync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<(), String> {
    fsync_directory(path).map_err(|error| format!("could not sync archive directory: {error}"))
}
