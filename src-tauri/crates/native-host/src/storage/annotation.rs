//! Annotation persistence and its public projection.
//!
//! Annotations are publication-scoped application data held outside authored
//! EPUB bytes. Navigator creates targets and mutation requests; the host derives
//! publication identity from the authenticated live session and commits both the
//! mutation and the new complete projection in one revision compare-and-swap
//! transaction. A conflict returns the authoritative projection unchanged.

use super::{Storage, decode_creators_summary, random_uuid};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

impl Storage {
    pub(crate) fn load_annotation_exchange_source(
        &self,
        publication_id: &str,
    ) -> Result<Option<AnnotationExchangeSource>, String> {
        let Some(blob) = self.load_publication_blob(publication_id)? else {
            return Ok(None);
        };
        let (title, creators) = self
            .connection
            .query_row(
                "SELECT title_summary, creators_summary FROM publications
                 WHERE publication_id = ?1",
                [publication_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(|error| format!("could not load publication summary: {error}"))?;
        Ok(Some(AnnotationExchangeSource {
            blob_hash: blob.hash,
            blob_path: blob.path,
            title,
            creators: decode_creators_summary(&creators)?,
            annotations: self.load_annotations(publication_id)?,
        }))
    }

    /// Replaces a projection with records derived from its current state in one transaction.
    pub(crate) fn update_annotation_records<T>(
        &mut self,
        publication_id: &str,
        reading_order_resources: &[Option<String>],
        update: impl FnOnce(&[AnnotationRecord]) -> (Option<Vec<AnnotationRecord>>, T),
    ) -> Result<(AnnotationProjection, T), SaveAnnotationError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| SaveAnnotationError::Storage)?;
        let mut projection = load_annotation_projection(&transaction, publication_id)
            .map_err(|_| SaveAnnotationError::Storage)?;
        let (records, output) = update(&projection.records);
        let Some(records) = records else {
            return Ok((projection, output));
        };
        validate_annotation_records(&records).map_err(|_| SaveAnnotationError::Invalid)?;
        let candidate = AnnotationProjection {
            revision: projection.revision,
            records,
        };
        candidate
            .validate_resource_hints(reading_order_resources)
            .map_err(|_| SaveAnnotationError::Invalid)?;
        projection.records = candidate.records;
        let record_count =
            u32::try_from(projection.records.len()).map_err(|_| SaveAnnotationError::Invalid)?;
        projection.revision = projection
            .revision
            .checked_add(1)
            .ok_or(SaveAnnotationError::Invalid)?
            .max(record_count);
        replace_annotations(&transaction, publication_id, &projection.records)
            .map_err(|_| SaveAnnotationError::Storage)?;
        write_projection_revision(&transaction, publication_id, projection.revision)
            .map_err(|_| SaveAnnotationError::Storage)?;
        transaction
            .commit()
            .map_err(|_| SaveAnnotationError::Storage)?;
        Ok((projection, output))
    }

    pub(crate) fn load_annotations(
        &self,
        publication_id: &str,
    ) -> Result<AnnotationProjection, String> {
        load_annotation_projection(&self.connection, publication_id)
    }

    pub(crate) fn compare_and_swap_annotations(
        &mut self,
        publication_id: &str,
        mutation: AnnotationMutation,
        reading_order_resources: &[Option<String>],
    ) -> Result<SaveAnnotationOutcome, SaveAnnotationError> {
        validate_annotation_mutation(&mutation).map_err(|_| SaveAnnotationError::Invalid)?;
        let request_id = mutation.request_id().to_owned();
        let base_revision = mutation.based_on_revision();
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| SaveAnnotationError::Storage)?;
        let publication_exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM publications
                 WHERE publication_id = ?1 AND deletion_state = 'live')",
                [publication_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| SaveAnnotationError::Storage)?;
        if !publication_exists {
            return Err(SaveAnnotationError::Storage);
        }
        let mut projection = load_annotation_projection(&transaction, publication_id)
            .map_err(|_| SaveAnnotationError::Storage)?;
        if projection.revision != base_revision {
            return Ok(SaveAnnotationOutcome::RevisionMismatch {
                request_id,
                projection,
            });
        }
        if projection.revision == u32::MAX {
            return Ok(SaveAnnotationOutcome::RevisionExhausted {
                request_id,
                projection,
            });
        }
        let target_occurrence = mutation.occurrence_for_target_change().or_else(|| {
            mutation.retarget_record_id().and_then(|record_id| {
                projection
                    .records
                    .iter()
                    .find(|record| record.id() == record_id)
                    .map(AnnotationRecord::occurrence)
            })
        });
        let resource_hint = target_occurrence
            .and_then(|occurrence| usize::try_from(occurrence).ok())
            .and_then(|occurrence| reading_order_resources.get(occurrence))
            .cloned()
            .flatten();
        let now = annotation_timestamp_now().map_err(|_| SaveAnnotationError::Storage)?;
        let removes_record = matches!(mutation, AnnotationMutation::DeleteAnnotation { .. });
        let record_id =
            apply_annotation_mutation(&mut projection.records, mutation, resource_hint, &now)
                .map_err(|_| SaveAnnotationError::Invalid)?;
        validate_annotation_records(&projection.records)
            .map_err(|_| SaveAnnotationError::Invalid)?;
        projection.revision += 1;
        // One mutation touches one row: the whole projection is never rewritten.
        if removes_record {
            delete_annotation(&transaction, publication_id, &record_id)
                .map_err(|_| SaveAnnotationError::Storage)?;
        } else {
            let record = projection
                .records
                .iter()
                .find(|record| record.id() == record_id)
                .ok_or(SaveAnnotationError::Storage)?;
            write_annotation(&transaction, publication_id, record)
                .map_err(|_| SaveAnnotationError::Storage)?;
        }
        write_projection_revision(&transaction, publication_id, projection.revision)
            .map_err(|_| SaveAnnotationError::Storage)?;
        transaction
            .commit()
            .map_err(|_| SaveAnnotationError::Storage)?;
        Ok(SaveAnnotationOutcome::Saved {
            request_id,
            record_id,
            projection,
        })
    }
}

pub(crate) struct AnnotationExchangeSource {
    pub blob_hash: String,
    pub blob_path: PathBuf,
    pub title: String,
    pub creators: Vec<String>,
    pub annotations: AnnotationProjection,
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotationProjection {
    pub revision: u32,
    pub records: Vec<AnnotationRecord>,
}

pub(crate) struct PublicAnnotationProjection<'a>(pub(crate) &'a AnnotationProjection);

pub(crate) struct PublicAnnotationRecord<'a>(pub(crate) &'a AnnotationRecord);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum AnnotationRecord {
    Highlight {
        id: String,
        #[serde(rename = "targetJson")]
        target_json: String,
        occurrence: u32,
        excerpt: Option<String>,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
        created: String,
        modified: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creator: Option<AnnotationCreatorRecord>,
        #[serde(rename = "resourceHint")]
        resource_hint: String,
    },
    Note {
        id: String,
        #[serde(rename = "targetJson")]
        target_json: String,
        occurrence: u32,
        excerpt: Option<String>,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
        text: String,
        created: String,
        modified: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creator: Option<AnnotationCreatorRecord>,
        #[serde(rename = "resourceHint")]
        resource_hint: String,
    },
    Bookmark {
        id: String,
        #[serde(rename = "targetJson")]
        target_json: String,
        occurrence: u32,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
        label: Option<String>,
        created: String,
        modified: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creator: Option<AnnotationCreatorRecord>,
        #[serde(rename = "resourceHint")]
        resource_hint: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnnotationCreatorRecord {
    pub id: String,
    #[serde(rename = "type")]
    pub creator_type: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum AnnotationMutation {
    CreateBookmark {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "locationRevision")]
        location_revision: u32,
        occurrence: u32,
        #[serde(rename = "targetJson")]
        target_json: String,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
        label: Option<String>,
    },
    CreateHighlight {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "selectionRevision")]
        selection_revision: u32,
        occurrence: u32,
        #[serde(rename = "targetJson")]
        target_json: String,
        excerpt: Option<String>,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
    },
    CreateNote {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "selectionRevision")]
        selection_revision: u32,
        occurrence: u32,
        #[serde(rename = "targetJson")]
        target_json: String,
        excerpt: Option<String>,
        #[serde(rename = "sectionLabel")]
        section_label: Option<String>,
        text: String,
    },
    UpdateNote {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "recordId")]
        record_id: String,
        text: String,
    },
    ConvertHighlightToNote {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "recordId")]
        record_id: String,
        text: String,
    },
    DeleteAnnotation {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "recordId")]
        record_id: String,
    },
    RetargetAnnotation {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "basedOnRevision")]
        based_on_revision: u32,
        #[serde(rename = "recordId")]
        record_id: String,
        #[serde(rename = "targetJson")]
        target_json: String,
        excerpt: String,
    },
}

pub(crate) enum SaveAnnotationOutcome {
    Saved {
        request_id: String,
        record_id: String,
        projection: AnnotationProjection,
    },
    RevisionMismatch {
        request_id: String,
        projection: AnnotationProjection,
    },
    RevisionExhausted {
        request_id: String,
        projection: AnnotationProjection,
    },
}

#[derive(Debug)]
pub(crate) enum SaveAnnotationError {
    Invalid,
    Storage,
}

pub(crate) fn validate_annotation_records(records: &[AnnotationRecord]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for record in records {
        if !ids.insert(record.id()) {
            return Err("annotation projection has duplicate record IDs".to_owned());
        }
        validate_annotation_id(record.id())?;
        validate_annotation_timestamps(record.created(), record.modified())?;
        if let Some(creator) = record.creator() {
            validate_annotation_creator(creator)?;
        }
        let (target, section_label, excerpt, note, label, resource_hint) = match record {
            AnnotationRecord::Highlight {
                target_json,
                section_label,
                excerpt,
                resource_hint,
                ..
            } => (
                target_json,
                section_label,
                Some(excerpt),
                None,
                None,
                resource_hint,
            ),
            AnnotationRecord::Note {
                target_json,
                section_label,
                excerpt,
                text,
                resource_hint,
                ..
            } => (
                target_json,
                section_label,
                Some(excerpt),
                Some(text),
                None,
                resource_hint,
            ),
            AnnotationRecord::Bookmark {
                target_json,
                section_label,
                label,
                resource_hint,
                ..
            } => (
                target_json,
                section_label,
                None,
                None,
                Some(label),
                resource_hint,
            ),
        };
        validate_annotation_target(target)?;
        validate_canonical_resource_hint(resource_hint)?;
        validate_nullable_annotation_text(
            section_label,
            MAX_ANNOTATION_LABEL_CODE_POINTS,
            MAX_ANNOTATION_LABEL_BYTES,
        )?;
        if let Some(excerpt) = excerpt {
            validate_nullable_annotation_text(
                excerpt,
                MAX_ANNOTATION_EXCERPT_CODE_POINTS,
                MAX_ANNOTATION_EXCERPT_BYTES,
            )?;
        }
        if let Some(label) = label {
            validate_nullable_annotation_text(
                label,
                MAX_ANNOTATION_LABEL_CODE_POINTS,
                MAX_ANNOTATION_LABEL_BYTES,
            )?;
        }
        if let Some(note) = note {
            validate_annotation_note(note)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_annotation_timestamps(created: &str, modified: &str) -> Result<(), String> {
    let (Some(created), Some(modified)) = (
        parse_annotation_timestamp(created),
        parse_annotation_timestamp(modified),
    ) else {
        return Err("annotation timestamp is invalid".to_owned());
    };
    if modified < created {
        return Err("annotation modified timestamp precedes created".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_annotation_creator(creator: &AnnotationCreatorRecord) -> Result<(), String> {
    if creator.id.is_empty()
        || creator.id.len() > MAX_ANNOTATION_LABEL_BYTES
        || url::Url::parse(&creator.id).is_err()
        || !matches!(
            creator.creator_type.as_str(),
            "Person" | "Organization" | "Software"
        )
    {
        return Err("annotation creator is invalid".to_owned());
    }
    validate_nullable_annotation_text(
        &creator.name,
        MAX_ANNOTATION_LABEL_CODE_POINTS,
        MAX_ANNOTATION_LABEL_BYTES,
    )
}

pub(crate) fn validate_annotation_target(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_ANNOTATION_TARGET_BYTES
        || !serde_json::from_str::<Value>(value).is_ok_and(|target| target.is_object())
    {
        return Err("annotation target is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_annotation_note(value: &str) -> Result<(), String> {
    if value.chars().count() == 0
        || value.chars().count() > MAX_ANNOTATION_NOTE_CODE_POINTS
        || value.len() > MAX_ANNOTATION_NOTE_BYTES
        || !value.chars().any(|character| !character.is_whitespace())
    {
        return Err("annotation note is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_nullable_annotation_text(
    value: &Option<String>,
    max_code_points: usize,
    max_bytes: usize,
) -> Result<(), String> {
    match value {
        Some(value) => validate_annotation_text(value, max_code_points, max_bytes),
        None => Ok(()),
    }
}

pub(crate) fn validate_annotation_text(
    value: &str,
    max_code_points: usize,
    max_bytes: usize,
) -> Result<(), String> {
    if value.chars().count() > max_code_points || value.len() > max_bytes {
        return Err("annotation text field is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_annotation_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_ANNOTATION_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    {
        return Err("annotation ID is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn validate_canonical_resource_hint(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.contains('?')
        || value.contains('#')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
        || value
            .chars()
            .any(|character| character <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&character))
    {
        return Err("annotation resource hint is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn annotation_timestamp_now() -> Result<String, String> {
    format_annotation_timestamp(time::OffsetDateTime::now_utc())
}

/// RFC 3339 UTC with exactly three fractional digits, the same shape the schema
/// writes with `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`.
///
/// The width is fixed on purpose. `Rfc3339` formatting drops trailing zeros, so it
/// emits `.5Z` for one instant and `.501Z` for a later one, and comparing those as
/// strings orders the later one first. Annotation timestamps are compared as
/// strings by the schema and by anything that exchanges or merges them, so a
/// variable width makes `modified` sort before its own `created`.
pub(crate) fn format_annotation_timestamp(value: time::OffsetDateTime) -> Result<String, String> {
    let value = value.to_offset(time::UtcOffset::UTC);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        value.year(),
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond(),
    ))
}

pub(crate) fn parse_annotation_timestamp(value: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}

/// Loads a publication's annotations in projection order.
///
/// The projection revision lives on `annotation_projections` and the records live
/// in `annotations`, one row each, so a mutation rewrites one row rather than
/// every annotation the reader owns.
pub(super) fn load_annotation_projection(
    connection: &Connection,
    publication_id: &str,
) -> Result<AnnotationProjection, String> {
    let revision: Option<u32> = connection
        .query_row(
            "SELECT projection_revision FROM annotation_projections WHERE publication_id = ?1",
            [publication_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("could not load annotation revision: {error}"))?
        .map(u32::try_from)
        .transpose()
        .map_err(|_| "stored annotation projection revision is invalid".to_owned())?;
    let mut statement = connection
        .prepare(
            "SELECT annotation_id, record_type, occurrence, target_json, resource_hint,
                    section_label, excerpt, text, label, creator_json, created, modified
             FROM annotations WHERE publication_id = ?1 ORDER BY record_order",
        )
        .map_err(|error| format!("could not prepare annotation load: {error}"))?;
    let records = statement
        .query_map([publication_id], |row| {
            Ok(StoredAnnotationRow {
                id: row.get(0)?,
                record_type: row.get(1)?,
                occurrence: row.get(2)?,
                target_json: row.get(3)?,
                resource_hint: row.get(4)?,
                section_label: row.get(5)?,
                excerpt: row.get(6)?,
                text: row.get(7)?,
                label: row.get(8)?,
                creator_json: row.get(9)?,
                created: row.get(10)?,
                modified: row.get(11)?,
            })
        })
        .map_err(|error| format!("could not query annotations: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read annotations: {error}"))?
        .into_iter()
        .map(StoredAnnotationRow::into_record)
        .collect::<Result<Vec<_>, String>>()?;
    let revision = match revision {
        None => {
            if !records.is_empty() {
                return Err("stored annotations have no projection revision".to_owned());
            }
            0
        }
        Some(0) => return Err("stored annotation projection revision is invalid".to_owned()),
        Some(revision) => {
            if revision < u32::try_from(records.len()).unwrap_or(u32::MAX) {
                return Err("stored annotation projection revision is invalid".to_owned());
            }
            revision
        }
    };
    validate_annotation_records(&records)
        .map_err(|error| format!("stored annotations are invalid: {error}"))?;
    Ok(AnnotationProjection { revision, records })
}

struct StoredAnnotationRow {
    id: String,
    record_type: String,
    occurrence: u32,
    target_json: String,
    resource_hint: String,
    section_label: Option<String>,
    excerpt: Option<String>,
    text: Option<String>,
    label: Option<String>,
    creator_json: Option<String>,
    created: String,
    modified: String,
}

impl StoredAnnotationRow {
    fn into_record(self) -> Result<AnnotationRecord, String> {
        let creator = self
            .creator_json
            .map(|json| serde_json::from_str::<AnnotationCreatorRecord>(&json))
            .transpose()
            .map_err(|error| format!("stored annotation creator is invalid: {error}"))?;
        Ok(match self.record_type.as_str() {
            "highlight" => AnnotationRecord::Highlight {
                id: self.id,
                target_json: self.target_json,
                occurrence: self.occurrence,
                excerpt: self.excerpt,
                section_label: self.section_label,
                created: self.created,
                modified: self.modified,
                creator,
                resource_hint: self.resource_hint,
            },
            "note" => AnnotationRecord::Note {
                id: self.id,
                target_json: self.target_json,
                occurrence: self.occurrence,
                excerpt: self.excerpt,
                section_label: self.section_label,
                text: self
                    .text
                    .ok_or_else(|| "stored annotation note has no text".to_owned())?,
                created: self.created,
                modified: self.modified,
                creator,
                resource_hint: self.resource_hint,
            },
            "bookmark" => AnnotationRecord::Bookmark {
                id: self.id,
                target_json: self.target_json,
                occurrence: self.occurrence,
                section_label: self.section_label,
                label: self.label,
                created: self.created,
                modified: self.modified,
                creator,
                resource_hint: self.resource_hint,
            },
            other => return Err(format!("stored annotation type `{other}` is unknown")),
        })
    }
}

/// The column values one annotation row holds.
struct AnnotationColumns<'a> {
    id: &'a str,
    record_type: &'a str,
    occurrence: u32,
    target_json: &'a str,
    resource_hint: &'a str,
    section_label: Option<&'a str>,
    excerpt: Option<&'a str>,
    text: Option<&'a str>,
    label: Option<&'a str>,
    creator_json: Option<String>,
    created: &'a str,
    modified: &'a str,
}

impl<'a> AnnotationColumns<'a> {
    fn of(record: &'a AnnotationRecord) -> Result<Self, String> {
        let creator_json = record
            .creator()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| format!("could not encode annotation creator: {error}"))?;
        let (record_type, section_label, excerpt, text, label) = match record {
            AnnotationRecord::Highlight {
                excerpt,
                section_label,
                ..
            } => (
                "highlight",
                section_label.as_deref(),
                excerpt.as_deref(),
                None,
                None,
            ),
            AnnotationRecord::Note {
                excerpt,
                section_label,
                text,
                ..
            } => (
                "note",
                section_label.as_deref(),
                excerpt.as_deref(),
                Some(text.as_str()),
                None,
            ),
            AnnotationRecord::Bookmark {
                section_label,
                label,
                ..
            } => (
                "bookmark",
                section_label.as_deref(),
                None,
                None,
                label.as_deref(),
            ),
        };
        Ok(Self {
            id: record.id(),
            record_type,
            occurrence: record.occurrence(),
            target_json: record.target_json(),
            resource_hint: record.resource_hint(),
            section_label,
            excerpt,
            text,
            label,
            creator_json,
            created: record.created(),
            modified: record.modified(),
        })
    }
}

/// Writes one annotation, appending it in projection order when it is new and
/// leaving its position alone when it already exists.
fn write_annotation(
    connection: &Connection,
    publication_id: &str,
    record: &AnnotationRecord,
) -> Result<(), String> {
    let columns = AnnotationColumns::of(record)?;
    connection
        .execute(
            "INSERT INTO annotations (
                annotation_id, publication_id, record_order, record_type, occurrence,
                target_json, resource_hint, section_label, excerpt, text, label,
                creator_json, created, modified
             ) VALUES (
                ?1, ?2,
                (SELECT COALESCE(MAX(record_order), 0) + 1 FROM annotations
                 WHERE publication_id = ?2),
                ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
             )
             ON CONFLICT(publication_id, annotation_id) DO UPDATE SET
                record_type = excluded.record_type,
                occurrence = excluded.occurrence,
                target_json = excluded.target_json,
                resource_hint = excluded.resource_hint,
                section_label = excluded.section_label,
                excerpt = excluded.excerpt,
                text = excluded.text,
                label = excluded.label,
                creator_json = excluded.creator_json,
                created = excluded.created,
                modified = excluded.modified",
            params![
                columns.id,
                publication_id,
                columns.record_type,
                columns.occurrence,
                columns.target_json,
                columns.resource_hint,
                columns.section_label,
                columns.excerpt,
                columns.text,
                columns.label,
                columns.creator_json,
                columns.created,
                columns.modified,
            ],
        )
        .map_err(|error| format!("could not write annotation: {error}"))?;
    Ok(())
}

fn delete_annotation(
    connection: &Connection,
    publication_id: &str,
    annotation_id: &str,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM annotations WHERE publication_id = ?1 AND annotation_id = ?2",
            params![publication_id, annotation_id],
        )
        .map_err(|error| format!("could not delete annotation: {error}"))?;
    Ok(())
}

/// Replaces every annotation of a publication, used by the exchange paths that
/// substitute a whole set rather than mutating one record.
fn replace_annotations(
    connection: &Connection,
    publication_id: &str,
    records: &[AnnotationRecord],
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM annotations WHERE publication_id = ?1",
            [publication_id],
        )
        .map_err(|error| format!("could not clear annotations: {error}"))?;
    for record in records {
        write_annotation(connection, publication_id, record)?;
    }
    Ok(())
}

fn write_projection_revision(
    connection: &Connection,
    publication_id: &str,
    revision: u32,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO annotation_projections (publication_id, projection_revision)
             VALUES (?1, ?2)
             ON CONFLICT(publication_id) DO UPDATE SET
                projection_revision = excluded.projection_revision,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![publication_id, revision],
        )
        .map_err(|error| format!("could not write annotation revision: {error}"))?;
    Ok(())
}

impl AnnotationMutation {
    pub(crate) fn request_id(&self) -> &str {
        match self {
            Self::CreateBookmark { request_id, .. }
            | Self::CreateHighlight { request_id, .. }
            | Self::CreateNote { request_id, .. }
            | Self::UpdateNote { request_id, .. }
            | Self::ConvertHighlightToNote { request_id, .. }
            | Self::DeleteAnnotation { request_id, .. }
            | Self::RetargetAnnotation { request_id, .. } => request_id,
        }
    }

    pub(crate) fn based_on_revision(&self) -> u32 {
        match self {
            Self::CreateBookmark {
                based_on_revision, ..
            }
            | Self::CreateHighlight {
                based_on_revision, ..
            }
            | Self::CreateNote {
                based_on_revision, ..
            }
            | Self::UpdateNote {
                based_on_revision, ..
            }
            | Self::ConvertHighlightToNote {
                based_on_revision, ..
            }
            | Self::DeleteAnnotation {
                based_on_revision, ..
            }
            | Self::RetargetAnnotation {
                based_on_revision, ..
            } => *based_on_revision,
        }
    }

    pub(crate) fn occurrence_for_target_change(&self) -> Option<u32> {
        match self {
            Self::CreateBookmark { occurrence, .. }
            | Self::CreateHighlight { occurrence, .. }
            | Self::CreateNote { occurrence, .. } => Some(*occurrence),
            _ => None,
        }
    }

    pub(crate) fn retarget_record_id(&self) -> Option<&str> {
        match self {
            Self::RetargetAnnotation { record_id, .. } => Some(record_id),
            _ => None,
        }
    }
}

fn validate_annotation_mutation(mutation: &AnnotationMutation) -> Result<(), String> {
    validate_annotation_id(mutation.request_id())?;
    match mutation {
        AnnotationMutation::CreateBookmark {
            location_revision,
            target_json,
            section_label,
            label,
            ..
        } => {
            let _ = location_revision;
            validate_annotation_target(target_json)?;
            validate_nullable_annotation_text(
                section_label,
                MAX_ANNOTATION_LABEL_CODE_POINTS,
                MAX_ANNOTATION_LABEL_BYTES,
            )?;
            validate_nullable_annotation_text(
                label,
                MAX_ANNOTATION_LABEL_CODE_POINTS,
                MAX_ANNOTATION_LABEL_BYTES,
            )
        }
        AnnotationMutation::CreateHighlight {
            selection_revision,
            target_json,
            excerpt,
            section_label,
            ..
        } => {
            let _ = selection_revision;
            validate_annotation_target(target_json)?;
            validate_nullable_annotation_text(
                excerpt,
                MAX_ANNOTATION_EXCERPT_CODE_POINTS,
                MAX_ANNOTATION_EXCERPT_BYTES,
            )?;
            validate_nullable_annotation_text(
                section_label,
                MAX_ANNOTATION_LABEL_CODE_POINTS,
                MAX_ANNOTATION_LABEL_BYTES,
            )
        }
        AnnotationMutation::CreateNote {
            selection_revision,
            target_json,
            excerpt,
            section_label,
            text,
            ..
        } => {
            let _ = selection_revision;
            validate_annotation_target(target_json)?;
            validate_nullable_annotation_text(
                excerpt,
                MAX_ANNOTATION_EXCERPT_CODE_POINTS,
                MAX_ANNOTATION_EXCERPT_BYTES,
            )?;
            validate_nullable_annotation_text(
                section_label,
                MAX_ANNOTATION_LABEL_CODE_POINTS,
                MAX_ANNOTATION_LABEL_BYTES,
            )?;
            validate_annotation_note(text)
        }
        AnnotationMutation::UpdateNote {
            record_id, text, ..
        }
        | AnnotationMutation::ConvertHighlightToNote {
            record_id, text, ..
        } => {
            validate_annotation_id(record_id)?;
            validate_annotation_note(text)
        }
        AnnotationMutation::DeleteAnnotation { record_id, .. } => validate_annotation_id(record_id),
        AnnotationMutation::RetargetAnnotation {
            record_id,
            target_json,
            excerpt,
            ..
        } => {
            validate_annotation_id(record_id)?;
            validate_annotation_target(target_json)?;
            validate_annotation_text(
                excerpt,
                MAX_ANNOTATION_EXCERPT_CODE_POINTS,
                MAX_ANNOTATION_EXCERPT_BYTES,
            )
        }
    }
}

fn apply_annotation_mutation(
    records: &mut Vec<AnnotationRecord>,
    mutation: AnnotationMutation,
    resource_hint: Option<String>,
    now: &str,
) -> Result<String, String> {
    if matches!(
        mutation,
        AnnotationMutation::CreateBookmark { .. }
            | AnnotationMutation::CreateHighlight { .. }
            | AnnotationMutation::CreateNote { .. }
    ) {
        let id = random_uuid()?;
        if records.iter().any(|record| record.id() == id) {
            return Err("generated annotation record ID is not unique".to_owned());
        }
        let resource_hint = resource_hint
            .ok_or_else(|| "annotation occurrence has no unique local resource".to_owned())?;
        let record = match mutation {
            AnnotationMutation::CreateBookmark {
                occurrence,
                target_json,
                section_label,
                label,
                ..
            } => AnnotationRecord::Bookmark {
                id: id.clone(),
                target_json,
                occurrence,
                section_label,
                label,
                created: now.to_owned(),
                modified: now.to_owned(),
                creator: None,
                resource_hint,
            },
            AnnotationMutation::CreateHighlight {
                occurrence,
                target_json,
                excerpt,
                section_label,
                ..
            } => AnnotationRecord::Highlight {
                id: id.clone(),
                target_json,
                occurrence,
                excerpt,
                section_label,
                created: now.to_owned(),
                modified: now.to_owned(),
                creator: None,
                resource_hint,
            },
            AnnotationMutation::CreateNote {
                occurrence,
                target_json,
                excerpt,
                section_label,
                text,
                ..
            } => AnnotationRecord::Note {
                id: id.clone(),
                target_json,
                occurrence,
                excerpt,
                section_label,
                text,
                created: now.to_owned(),
                modified: now.to_owned(),
                creator: None,
                resource_hint,
            },
            _ => unreachable!(),
        };
        records.push(record);
        return Ok(id);
    }

    let record_id = match &mutation {
        AnnotationMutation::UpdateNote { record_id, .. }
        | AnnotationMutation::ConvertHighlightToNote { record_id, .. }
        | AnnotationMutation::DeleteAnnotation { record_id, .. }
        | AnnotationMutation::RetargetAnnotation { record_id, .. } => record_id.clone(),
        _ => unreachable!(),
    };
    let index = records
        .iter()
        .position(|record| record.id() == record_id)
        .ok_or_else(|| "annotation record does not exist".to_owned())?;
    match mutation {
        AnnotationMutation::UpdateNote { text, .. } => match &mut records[index] {
            AnnotationRecord::Note {
                text: current_text,
                modified,
                ..
            } => {
                *current_text = text;
                now.clone_into(modified);
            }
            _ => return Err("annotation record is not a note".to_owned()),
        },
        AnnotationMutation::ConvertHighlightToNote { text, .. } => {
            let AnnotationRecord::Highlight {
                id,
                target_json,
                occurrence,
                excerpt,
                section_label,
                created,
                creator,
                resource_hint,
                ..
            } = records[index].clone()
            else {
                return Err("annotation record is not a highlight".to_owned());
            };
            records[index] = AnnotationRecord::Note {
                id,
                target_json,
                occurrence,
                excerpt,
                section_label,
                text,
                created,
                modified: now.to_owned(),
                creator,
                resource_hint,
            };
        }
        AnnotationMutation::DeleteAnnotation { .. } => {
            records.remove(index);
        }
        AnnotationMutation::RetargetAnnotation {
            target_json,
            excerpt,
            ..
        } => {
            let resource_hint = resource_hint
                .ok_or_else(|| "annotation occurrence has no unique local resource".to_owned())?;
            match &mut records[index] {
                AnnotationRecord::Highlight {
                    target_json: current_target,
                    excerpt: current_excerpt,
                    modified,
                    resource_hint: current_resource,
                    ..
                }
                | AnnotationRecord::Note {
                    target_json: current_target,
                    excerpt: current_excerpt,
                    modified,
                    resource_hint: current_resource,
                    ..
                } => {
                    *current_target = target_json;
                    *current_excerpt = Some(excerpt);
                    now.clone_into(modified);
                    *current_resource = resource_hint;
                }
                AnnotationRecord::Bookmark { .. } => {
                    return Err("annotation record cannot be retargeted".to_owned());
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(record_id)
}

impl AnnotationRecord {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Highlight { id, .. } | Self::Note { id, .. } | Self::Bookmark { id, .. } => id,
        }
    }

    pub(crate) fn occurrence(&self) -> u32 {
        match self {
            Self::Highlight { occurrence, .. }
            | Self::Note { occurrence, .. }
            | Self::Bookmark { occurrence, .. } => *occurrence,
        }
    }

    pub(crate) fn target_json(&self) -> &str {
        match self {
            Self::Highlight { target_json, .. }
            | Self::Note { target_json, .. }
            | Self::Bookmark { target_json, .. } => target_json,
        }
    }

    pub(crate) fn created(&self) -> &str {
        match self {
            Self::Highlight { created, .. }
            | Self::Note { created, .. }
            | Self::Bookmark { created, .. } => created,
        }
    }

    pub(crate) fn modified(&self) -> &str {
        match self {
            Self::Highlight { modified, .. }
            | Self::Note { modified, .. }
            | Self::Bookmark { modified, .. } => modified,
        }
    }

    pub(crate) fn creator(&self) -> Option<&AnnotationCreatorRecord> {
        match self {
            Self::Highlight { creator, .. }
            | Self::Note { creator, .. }
            | Self::Bookmark { creator, .. } => creator.as_ref(),
        }
    }

    pub(super) fn resource_hint(&self) -> &str {
        match self {
            Self::Highlight { resource_hint, .. }
            | Self::Note { resource_hint, .. }
            | Self::Bookmark { resource_hint, .. } => resource_hint,
        }
    }
}

impl AnnotationProjection {
    pub(crate) fn public(&self) -> PublicAnnotationProjection<'_> {
        PublicAnnotationProjection(self)
    }

    pub(crate) fn validate_resource_hints(
        &self,
        reading_order_resources: &[Option<String>],
    ) -> Result<(), String> {
        for record in &self.records {
            let resource = usize::try_from(record.occurrence())
                .ok()
                .and_then(|occurrence| reading_order_resources.get(occurrence))
                .and_then(Option::as_deref);
            if resource != Some(record.resource_hint()) {
                return Err("annotation resource hint does not match its occurrence".to_owned());
            }
        }
        Ok(())
    }
}

pub(super) const MAX_ANNOTATION_TARGET_BYTES: usize = 1_048_576;

pub(super) const MAX_ANNOTATION_NOTE_CODE_POINTS: usize = 4_096;

pub(super) const MAX_ANNOTATION_NOTE_BYTES: usize = 16_384;

pub(super) const MAX_ANNOTATION_EXCERPT_CODE_POINTS: usize = 512;

pub(super) const MAX_ANNOTATION_EXCERPT_BYTES: usize = 2_048;

pub(super) const MAX_ANNOTATION_LABEL_CODE_POINTS: usize = 256;

pub(super) const MAX_ANNOTATION_LABEL_BYTES: usize = 1_024;

pub(super) const MAX_ANNOTATION_ID_BYTES: usize = 128;
