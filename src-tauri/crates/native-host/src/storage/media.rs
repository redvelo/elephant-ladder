//! Media overlay timelines and the audio sidecars a publication was installed
//! with: where their bytes live and which transcodes can be reused.

use super::{
    MediaOverlayDescription, MediaOverlayTimelineFile, MediaSidecar, Storage, Transaction,
    is_blake3_hash, params, sync_directory, verify_blob,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

impl Storage {
    /// Returns `None` when any committed sidecar file is missing or has the wrong length, so the
    /// snapshot can be derived again from its immutable source.
    /// Returns `None` when any timeline file is missing or truncated, which routes the
    /// publication to re-derivation exactly as a missing sidecar does.
    pub(super) fn load_media_overlay_timelines(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<BTreeMap<u32, MediaOverlayTimelineFile>>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT overlay_index, storage_name, length_bytes
                 FROM media_overlay_timeline_files WHERE snapshot_id = ?1",
            )
            .map_err(|error| format!("could not prepare media timeline query: {error}"))?;
        let rows = statement
            .query_map([snapshot_id], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .map_err(|error| format!("could not query media timelines: {error}"))?;
        let mut timelines = BTreeMap::new();
        for row in rows {
            let (overlay_index, storage_name, length) =
                row.map_err(|error| format!("could not read media timeline: {error}"))?;
            if !is_blake3_hash(&storage_name) {
                return Err("stored media timeline name is invalid".to_owned());
            }
            let path = self.media_timelines.join(storage_name);
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() && metadata.len() == length => {}
                _ => return Ok(None),
            }
            timelines.insert(overlay_index, MediaOverlayTimelineFile { path, length });
        }
        Ok(Some(timelines))
    }

    pub(super) fn load_media_sidecars(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<BTreeMap<String, MediaSidecar>>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT canonical_path, storage_name, length_bytes, media_type
                 FROM media_overlay_sidecars WHERE snapshot_id = ?1",
            )
            .map_err(|error| format!("could not prepare media sidecar query: {error}"))?;
        let rows = statement
            .query_map([snapshot_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| format!("could not query media sidecars: {error}"))?;
        let mut sidecars = BTreeMap::new();
        for row in rows {
            let (canonical_path, storage_name, length, media_type) =
                row.map_err(|error| format!("could not read media sidecar: {error}"))?;
            if !is_blake3_hash(&storage_name) {
                return Err("stored media sidecar name is invalid".to_owned());
            }
            let path = self.media_sidecars.join(storage_name);
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() && metadata.len() == length => {}
                _ => return Ok(None),
            }
            sidecars.insert(
                canonical_path,
                MediaSidecar {
                    path,
                    length,
                    media_type,
                },
            );
        }
        Ok(Some(sidecars))
    }

    /// Transcodes still installed for a live snapshot, keyed by the BLAKE3 of their source audio.
    pub(crate) fn reusable_media_transcodes(
        &self,
        derivation: &str,
    ) -> Result<
        std::collections::HashMap<String, crate::media_overlay::backing::ReusableSidecar>,
        String,
    > {
        let mut statement = self
            .connection
            .prepare(
                "SELECT source_hash, storage_name, length_bytes FROM media_sidecar_transcodes
                 WHERE derivation = ?1
                   AND storage_name IN (SELECT storage_name FROM media_overlay_sidecars)",
            )
            .map_err(|error| format!("could not prepare media transcode query: {error}"))?;
        let rows = statement
            .query_map([derivation], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .map_err(|error| format!("could not query media transcodes: {error}"))?;
        let mut transcodes = std::collections::HashMap::new();
        for row in rows {
            let (source_hash, storage_name, length) =
                row.map_err(|error| format!("could not read media transcode: {error}"))?;
            if !is_blake3_hash(&storage_name) {
                continue;
            }
            match fs::metadata(self.media_sidecars.join(&storage_name)) {
                Ok(metadata) if metadata.is_file() && metadata.len() == length => {}
                _ => continue,
            }
            transcodes.insert(
                source_hash,
                crate::media_overlay::backing::ReusableSidecar {
                    storage_name,
                    length,
                },
            );
        }
        Ok(transcodes)
    }

    pub fn media_sidecars_dir(&self) -> &Path {
        &self.media_sidecars
    }
}

/// Narration products never decide whether a publication opens. Anything oversized, unparsable, or
/// invalid degrades to a representation-limited description carrying that reason to the reader,
/// rather than failing the load and making the publication unopenable.
pub(super) fn load_stored_media_overlay(
    description_json: &str,
    capability_json: &str,
) -> (
    MediaOverlayDescription,
    crate::media_overlay::MediaOverlayCapability,
) {
    let degraded = || {
        (
            crate::media_overlay::representation_limited_description(),
            crate::media_overlay::MediaOverlayCapability::Unavailable {},
        )
    };
    let Ok(description) = serde_json::from_str::<MediaOverlayDescription>(description_json) else {
        return degraded();
    };
    if crate::media_overlay::validate_media_overlay_description(&description).is_err() {
        return degraded();
    }
    let Ok(capability) =
        serde_json::from_str::<crate::media_overlay::MediaOverlayCapability>(capability_json)
    else {
        return degraded();
    };
    if !crate::media_overlay::capability_matches_playback(&capability, &description.playback) {
        return degraded();
    }
    (description, capability)
}

#[cfg(test)]
pub(super) fn test_absent_media_overlay_json() -> String {
    serde_json::to_string(&crate::media_overlay::absent_media_overlay_description())
        .expect("the absent media overlay description always serializes")
}

/// One timeline serialized to its exact response bytes and content-addressed, so identical
/// re-derivations reuse the same file.
pub(crate) struct PreparedTimeline {
    overlay_index: u32,
    storage_name: String,
    bytes: Vec<u8>,
}

pub(super) fn prepare_media_overlay_timelines(
    timelines: &[crate::media_overlay::MediaOverlayTimeline],
) -> Result<Vec<PreparedTimeline>, String> {
    timelines
        .iter()
        .map(|timeline| {
            let bytes = timeline
                .to_result_json()
                .map_err(|error| format!("could not serialize media overlay timeline: {error}"))?;
            Ok(PreparedTimeline {
                overlay_index: timeline.overlay_index,
                storage_name: blake3::hash(&bytes).to_hex().to_string(),
                bytes,
            })
        })
        .collect()
}

pub(super) fn install_media_overlay_timelines_in_transaction(
    transaction: &Transaction<'_>,
    media_timelines: &Path,
    snapshot_id: &str,
    prepared: Vec<PreparedTimeline>,
) -> Result<(), String> {
    for timeline in prepared {
        let installed = media_timelines.join(&timeline.storage_name);
        if !installed.exists() {
            let staging = media_timelines.join(format!("{}.part", timeline.storage_name));
            fs::write(&staging, &timeline.bytes)
                .map_err(|error| format!("could not stage media overlay timeline: {error}"))?;
            fs::rename(&staging, &installed)
                .map_err(|error| format!("could not commit media overlay timeline: {error}"))?;
        }
        transaction
            .execute(
                "INSERT INTO media_overlay_timeline_files
                    (snapshot_id, overlay_index, storage_name, length_bytes)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(snapshot_id, overlay_index) DO UPDATE SET
                    storage_name = excluded.storage_name,
                    length_bytes = excluded.length_bytes",
                params![
                    snapshot_id,
                    timeline.overlay_index,
                    timeline.storage_name,
                    timeline.bytes.len() as u64
                ],
            )
            .map_err(|error| format!("could not record media overlay timeline: {error}"))?;
    }
    Ok(())
}

pub(super) fn install_media_sidecars_in_transaction(
    transaction: &Transaction<'_>,
    media_sidecars: &Path,
    snapshot_id: &str,
    prepared: Vec<crate::media_overlay::backing::PreparedSidecar>,
) -> Result<(), String> {
    if prepared.is_empty() {
        return Ok(());
    }
    for sidecar in prepared {
        let crate::media_overlay::backing::PreparedSidecarParts {
            canonical_path,
            storage_name,
            length,
            media_type,
            origin,
            source_hash,
            staging_path,
        } = sidecar.into_parts();
        let installed_path = media_sidecars.join(&storage_name);
        match staging_path {
            Some(staging_path) if !installed_path.exists() => {
                fs::rename(&staging_path, &installed_path)
                    .map_err(|error| format!("could not commit media overlay sidecar: {error}"))?;
            }
            staging_path => {
                verify_blob(&installed_path, &storage_name, length)?;
                if let Some(staging_path) = staging_path {
                    let _ = fs::remove_file(&staging_path);
                }
            }
        }
        transaction
            .execute(
                "INSERT INTO media_overlay_sidecars
                    (snapshot_id, canonical_path, storage_name, length_bytes, media_type)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(snapshot_id, canonical_path) DO UPDATE SET
                    storage_name = excluded.storage_name,
                    length_bytes = excluded.length_bytes,
                    media_type = excluded.media_type",
                params![
                    snapshot_id,
                    canonical_path,
                    storage_name,
                    length,
                    media_type
                ],
            )
            .map_err(|error| format!("could not record media overlay sidecar: {error}"))?;
        if origin == crate::media_overlay::backing::SidecarOrigin::Transcoded
            && let Some(source_hash) = source_hash
        {
            transaction
                .execute(
                    "INSERT INTO media_sidecar_transcodes
                            (source_hash, derivation, storage_name, length_bytes)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(source_hash, derivation) DO UPDATE SET
                            storage_name = excluded.storage_name,
                            length_bytes = excluded.length_bytes",
                    params![
                        source_hash,
                        crate::media_overlay::backing::transcode_derivation(),
                        storage_name,
                        length
                    ],
                )
                .map_err(|error| format!("could not record media transcode: {error}"))?;
        }
    }
    sync_directory(media_sidecars)?;
    Ok(())
}
