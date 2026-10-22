//! Reading the library: what a publication summary says, which snapshot an
//! open selects, and where its archive and cover bytes are.

use super::{
    DcElement, InstalledBlob, Package, PolicyRelevance, PublicationMetadata,
    PublicationPolicyRelevance, PublicationSelection, PublicationSummary, SelectedPublication,
    Storage, StoredCover, decode_reading_position, is_blake3_hash, is_origin_id,
    load_stored_media_overlay,
};
use crate::snapshot::validate_stored_reading_products;
use crate::{MAX_FACTS_SNAPSHOT_BYTES, MAX_RESPONSE_MANIFEST_BYTES, ResponseManifest};
use epub_stack::semantics::TextDirection;
use rusqlite::OptionalExtension;
use serde_json::Value;

impl Storage {
    pub fn load_publication_description(
        &self,
        publication_id: &str,
    ) -> Result<Option<Option<String>>, String> {
        let row: Option<Option<String>> = self.connection.query_row(
            "SELECT json_extract(s.facts_json, '$.package.metadata.description')
             FROM publications p LEFT JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
             WHERE p.publication_id = ?1 AND p.deletion_state = 'live'",
            [publication_id], |row| row.get(0),
        ).optional().map_err(|error| error.to_string())?;
        row.map(|json| {
            let descriptions: Value = match json {
                Some(json) => serde_json::from_str(&json).map_err(|error| error.to_string())?,
                None => return Ok(None),
            };
            Ok(descriptions
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("content").and_then(Value::as_str))
                        .map(str::trim)
                        .find(|text| !text.is_empty())
                })
                .map(|text| text.chars().take(16_000).collect()))
        })
        .transpose()
    }

    pub fn list_publications(&self) -> Result<Vec<PublicationSummary>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT p.publication_id, p.title_summary, p.creators_summary,
                         p.language_summary, p.reading_direction_summary, p.imported_at,
                        r.updated_at,
                        (s.cover_hash IS NOT NULL AND s.cover_size_bytes IS NOT NULL
                         AND s.cover_mime IS NOT NULL),
                        p.title_direction_summary, p.creator_directions_summary
                 FROM publications p
                 LEFT JOIN reading_positions r ON r.publication_id = p.publication_id
                 LEFT JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                  WHERE p.deletion_state = 'live'
                 ORDER BY r.updated_at IS NULL, r.updated_at DESC, p.imported_at DESC,
                          p.publication_id",
            )
            .map_err(|error| format!("could not prepare publication list: {error}"))?;
        let publications = statement
            .query_map([], |row| {
                Ok(PublicationSummary {
                    publication_id: row.get(0)?,
                    title: row.get(1)?,
                    creators: decode_creators_summary(&row.get::<_, String>(2)?).map_err(
                        |error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    error,
                                )),
                            )
                        },
                    )?,
                    language: row.get(3)?,
                    reading_direction: row.get(4)?,
                    imported_at: row.get(5)?,
                    last_read_at: row.get(6)?,
                    has_cover: row.get::<_, i64>(7)? != 0,
                    title_direction: row.get(8)?,
                    creator_directions: decode_creator_directions_summary(
                        &row.get::<_, String>(9)?,
                    )
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                        )
                    })?,
                })
            })
            .map_err(|error| format!("could not query publications: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read publications: {error}"))?;
        Ok(publications)
    }

    pub(crate) fn select_publication(
        &self,
        publication_id: &str,
        reading_revision: &str,
        warm_snapshot_id: Option<&str>,
    ) -> Result<PublicationSelection, String> {
        let row = self
            .connection
            .query_row(
                "SELECT p.deletion_state, p.current_snapshot_id,
                         s.reading_revision, s.revoked_at, b.blob_hash, b.storage_name,
                         b.size_bytes, b.health_state, b.lifecycle_state,
                          s.facts_json, s.responses_json,
                          s.active_content_relevance, s.remote_resources_relevance,
                          r.position_json, r.position_revision,
                         o.origin_id, s.media_overlay_json,
                         s.media_overlay_capability_json
                  FROM publications p
                  LEFT JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                  LEFT JOIN archive_blobs b ON b.blob_hash = s.blob_hash
                  LEFT JOIN reading_positions r ON r.publication_id = p.publication_id
                  LEFT JOIN browser_origin_allocations o
                    ON o.publication_id = p.publication_id AND o.lifecycle_state = 'active'
                  WHERE p.publication_id = ?1",
                [publication_id],
                |row| {
                    let snapshot_id = row.get::<_, Option<String>>(1)?;
                    let load_products = snapshot_id.as_deref() != warm_snapshot_id;
                    Ok((
                        row.get::<_, String>(0)?,
                        snapshot_id,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<u64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        load_products.then(|| row.get::<_, String>(9)).transpose()?,
                        load_products
                            .then(|| row.get::<_, String>(10))
                            .transpose()?,
                        load_products
                            .then(|| row.get::<_, String>(11))
                            .transpose()?,
                        load_products
                            .then(|| row.get::<_, String>(12))
                            .transpose()?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, Option<i64>>(14)?,
                        row.get::<_, Option<String>>(15)?,
                        load_products
                            .then(|| row.get::<_, Option<String>>(16))
                            .transpose()?
                            .flatten(),
                        load_products
                            .then(|| row.get::<_, Option<String>>(17))
                            .transpose()?
                            .flatten(),
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not select publication: {error}"))?;
        let Some((
            deletion_state,
            snapshot_id,
            stored_reading_revision,
            revoked_at,
            blob_hash,
            storage_name,
            size,
            health_state,
            lifecycle_state,
            facts_json,
            responses_json,
            active_content_relevance,
            remote_resources_relevance,
            position_json,
            position_revision,
            browser_origin_id,
            media_overlay_json,
            media_overlay_capability_json,
        )) = row
        else {
            return Ok(PublicationSelection::NotFound);
        };
        if deletion_state != "live" || revoked_at.is_some() {
            return Ok(PublicationSelection::Revoked);
        }
        let browser_origin_id = browser_origin_id
            .filter(|origin_id| is_origin_id(origin_id))
            .ok_or_else(|| "publication has no valid active browser origin".to_owned())?;
        let Some(snapshot_id) = snapshot_id else {
            return Ok(match self.load_publication_blob(publication_id)? {
                Some(blob) => PublicationSelection::NeedsDerivation(blob),
                None => PublicationSelection::SourceUnavailable,
            });
        };
        if stored_reading_revision.as_deref() != Some(reading_revision) {
            return Ok(match self.load_publication_blob(publication_id)? {
                Some(blob) => PublicationSelection::NeedsDerivation(blob),
                None => PublicationSelection::SourceUnavailable,
            });
        }
        let (Some(_blob_hash), Some(storage_name), Some(_size)) = (blob_hash, storage_name, size)
        else {
            return Ok(PublicationSelection::SourceUnavailable);
        };
        let blob_path = self.blobs.join(storage_name);
        if health_state.as_deref() != Some("available")
            || lifecycle_state.as_deref() != Some("live")
            || !blob_path.is_file()
        {
            return Ok(PublicationSelection::SourceUnavailable);
        }
        let (reading_position, position_revision) =
            decode_reading_position(position_json, position_revision)?;
        let annotations = self.load_annotations(publication_id)?;
        if warm_snapshot_id == Some(snapshot_id.as_str()) {
            return Ok(PublicationSelection::Warm {
                snapshot_id,
                browser_origin_id,
                reading_position,
                position_revision,
                annotations,
            });
        }
        let facts_json =
            facts_json.ok_or_else(|| "stored publication facts are missing".to_owned())?;
        let responses_json =
            responses_json.ok_or_else(|| "stored response manifest is missing".to_owned())?;
        let media_overlay_json = media_overlay_json
            .ok_or_else(|| "stored media overlay description is missing".to_owned())?;
        let media_overlay_capability_json = media_overlay_capability_json
            .ok_or_else(|| "stored media overlay timelines are missing".to_owned())?;
        let policy_relevance = PublicationPolicyRelevance {
            active_content: PolicyRelevance::parse(
                active_content_relevance
                    .as_deref()
                    .ok_or_else(|| "stored active-content relevance is missing".to_owned())?,
            )?,
            remote_resources: PolicyRelevance::parse(
                remote_resources_relevance
                    .as_deref()
                    .ok_or_else(|| "stored remote-resource relevance is missing".to_owned())?,
            )?,
        };
        if facts_json.len() > MAX_FACTS_SNAPSHOT_BYTES
            || responses_json.len() > MAX_RESPONSE_MANIFEST_BYTES
        {
            return Err("stored publication products exceed their bounds".to_owned());
        }
        let facts: Value = serde_json::from_str(&facts_json)
            .map_err(|error| format!("stored publication facts are invalid JSON: {error}"))?;
        let responses: ResponseManifest = serde_json::from_str(&responses_json)
            .map_err(|error| format!("stored response manifest is invalid: {error}"))?;
        let (media_overlay, media_overlay_capability) =
            load_stored_media_overlay(&media_overlay_json, &media_overlay_capability_json);
        validate_stored_reading_products(&facts, &responses)?;
        let Some(media_sidecars) = self.load_media_sidecars(&snapshot_id)? else {
            return Ok(match self.load_publication_blob(publication_id)? {
                Some(blob) => PublicationSelection::NeedsDerivation(blob),
                None => PublicationSelection::SourceUnavailable,
            });
        };
        let Some(media_overlay_timeline_files) = self.load_media_overlay_timelines(&snapshot_id)?
        else {
            return Ok(match self.load_publication_blob(publication_id)? {
                Some(blob) => PublicationSelection::NeedsDerivation(blob),
                None => PublicationSelection::SourceUnavailable,
            });
        };
        Ok(PublicationSelection::Ready(Box::new(SelectedPublication {
            blob_path,
            snapshot_id,
            browser_origin_id,
            facts,
            responses,
            media_overlay,
            media_overlay_capability,
            media_overlay_timeline_files,
            media_sidecars,
            policy_relevance,
            reading_position,
            position_revision,
            annotations,
        })))
    }

    pub(crate) fn load_publication_blob(
        &self,
        publication_id: &str,
    ) -> Result<Option<InstalledBlob>, String> {
        let stored = self
            .connection
            .query_row(
                "SELECT b.blob_hash, b.storage_name, b.size_bytes
                 FROM publications p
                 JOIN archive_blobs b ON b.blob_hash = p.imported_blob_hash
                 WHERE p.publication_id = ?1 AND p.deletion_state = 'live'
                   AND b.health_state = 'available' AND b.lifecycle_state = 'live'",
                [publication_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not load publication source: {error}"))?;
        let Some((hash, storage_name, size)) = stored else {
            return Ok(None);
        };
        let path = self.blobs.join(storage_name);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(InstalledBlob {
            hash,
            path,
            size,
            reservation_id: None,
            expected_publication_id: Some(publication_id.to_owned()),
        }))
    }

    pub(crate) fn load_publication_cover(
        &self,
        publication_id: &str,
    ) -> Result<Option<StoredCover>, String> {
        let descriptor = self
            .connection
            .query_row(
                "SELECT s.cover_hash, s.cover_size_bytes, s.cover_mime
                 FROM publications p
                 JOIN publication_snapshots s ON s.snapshot_id = p.current_snapshot_id
                 WHERE p.publication_id = ?1 AND p.deletion_state = 'live'
                   AND s.revoked_at IS NULL",
                [publication_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<u64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("could not load publication cover: {error}"))?;
        let Some((hash, size, mime)) = descriptor else {
            return Ok(None);
        };
        let descriptor = match (hash, size, mime) {
            (Some(hash), Some(size), Some(mime)) => (hash, size, mime),
            (None, None, None) => return Ok(None),
            _ => return Err("stored cover descriptor is incomplete".to_owned()),
        };
        let (hash, size, mime) = descriptor;
        if !is_blake3_hash(&hash) || !is_epub_core_image_mime(&mime) {
            return Err("stored cover descriptor is invalid".to_owned());
        }
        let path = self.covers.join(hash);
        let metadata = path
            .metadata()
            .map_err(|error| format!("stored cover artifact is unavailable: {error}"))?;
        if !metadata.is_file() || metadata.len() != size {
            return Err("stored cover artifact has the wrong size".to_owned());
        }
        Ok(Some(StoredCover { path, size, mime }))
    }
}

/// The direction a metadata element reads, as EPUB resolves it: the element's own `dir`
/// when it has one, otherwise the package's, otherwise unstated.
///
/// Unstated is not `auto`. A reading system may render it however it likes, and leaving
/// the attribute off is what the corpus expects, so it stays `None` rather than becoming
/// a value.
pub(super) fn resolved_direction(
    element: Option<TextDirection>,
    package: Option<TextDirection>,
) -> Option<String> {
    element
        .or(package)
        .map(|direction| direction.as_str().to_owned())
}

pub(super) fn publication_metadata(package: &Package) -> PublicationMetadata {
    let package_direction = package.dir();
    let title_direction = resolved_direction(
        package
            .metadata()
            .elements(DcElement::Title)
            .first()
            .and_then(|title| title.dir()),
        package_direction,
    );
    let named_creators: Vec<_> = package
        .metadata()
        .elements(DcElement::Creator)
        .iter()
        .filter(|creator| {
            creator
                .content()
                .is_some_and(|content| !content.as_str().trim().is_empty())
        })
        .collect();
    let creator_directions = named_creators
        .iter()
        .map(|creator| resolved_direction(creator.dir(), package_direction))
        .collect();
    let creators = named_creators
        .iter()
        .filter_map(|creator| creator.content())
        .map(|creator| creator.as_str().trim())
        .map(|creator| creator.chars().take(512).collect())
        .collect();
    let language = package
        .metadata()
        .elements(DcElement::Language)
        .first()
        .and_then(|language| language.content())
        .map(|language| language.as_str().trim())
        .filter(|language| !language.is_empty())
        .map(|language| language.chars().take(128).collect());
    let reading_direction = package
        .spine()
        .page_progression_direction()
        .map(|direction| direction.to_string());
    PublicationMetadata {
        title_direction,
        creators,
        creator_directions,
        language,
        reading_direction,
    }
}

#[cfg(test)]
pub(super) fn publication_metadata_from_persisted_value(facts: &Value) -> PublicationMetadata {
    let package_direction = facts
        .get("package")
        .and_then(|package| package.get("dir"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let named_creators: Vec<&Value> = facts
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("creator"))
        .and_then(Value::as_array)
        .map(|creators| {
            creators
                .iter()
                .filter(|creator| {
                    creator
                        .get("content")
                        .and_then(Value::as_str)
                        .is_some_and(|content| !content.trim().is_empty())
                })
                .collect()
        })
        .unwrap_or_default();
    let title_direction = facts
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("title"))
        .and_then(Value::as_array)
        .and_then(|titles| titles.first())
        .and_then(|title| title.get("dir"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| package_direction.clone());
    let creator_directions = named_creators
        .iter()
        .map(|creator| {
            creator
                .get("dir")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| package_direction.clone())
        })
        .collect();
    let creators = named_creators
        .iter()
        .filter_map(|creator| creator.get("content").and_then(Value::as_str))
        .map(str::trim)
        .map(|creator| creator.chars().take(512).collect())
        .collect();
    let language = facts
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get("language"))
        .and_then(Value::as_array)
        .and_then(|languages| languages.first())
        .and_then(|language| language.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .map(|language| language.chars().take(128).collect());
    let reading_direction = facts
        .get("package")
        .and_then(|package| package.get("spine"))
        .and_then(|spine| spine.get("pageProgressionDirection"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    PublicationMetadata {
        title_direction,
        creators,
        creator_directions,
        language,
        reading_direction,
    }
}

/// Index aligned with the creators. A row written before directions were recorded has an
/// empty array, and a creator with no entry simply has no declared direction.
pub(super) fn decode_creator_directions_summary(
    value: &str,
) -> Result<Vec<Option<String>>, String> {
    serde_json::from_str(value)
        .map_err(|error| format!("stored creator directions are not readable: {error}"))
}

pub(super) fn decode_creators_summary(value: &str) -> Result<Vec<String>, String> {
    serde_json::from_str(value)
        .map_err(|error| format!("publication creators summary is invalid: {error}"))
}

pub(crate) fn is_epub_core_image_mime(value: &str) -> bool {
    matches!(
        value,
        "image/gif"
            | "image/jpeg"
            | "image/png"
            | "image/svg+xml"
            | "image/webp"
            | "image/avif"
            | "image/jxl"
    )
}
