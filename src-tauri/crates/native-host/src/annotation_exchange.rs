use epub_stack::EpubPath;
use epub_stack::annotation::{
    Annotation, AnnotationAbout, AnnotationBody, AnnotationCreator, AnnotationCreatorType,
    AnnotationMotivation, AnnotationSelector, AnnotationSet, AnnotationTarget, FragmentConformsTo,
    JsonObject, LocalizableText,
};
use epub_stack::resource::{AuthoredHref, resolve_publication_href};
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::storage::{
    AnnotationCreatorRecord, AnnotationRecord, format_annotation_timestamp, format_uuid,
    parse_annotation_timestamp,
};

const MAX_EXCERPT_CODE_POINTS: usize = 512;
const MAX_EXCERPT_BYTES: usize = 2_048;
const MAX_LABEL_CODE_POINTS: usize = 256;
const MAX_LABEL_BYTES: usize = 1_024;

pub(crate) struct ExchangePublication<'a> {
    pub unique_identifier: Option<&'a str>,
    pub blob_hash: &'a str,
    pub title: &'a str,
    pub creators: &'a [String],
    pub package_path: &'a EpubPath,
    pub reading_order_resources: &'a [Option<String>],
}

impl ExchangePublication<'_> {
    fn identifiers(&self) -> Vec<String> {
        publication_identifiers(self.unique_identifier, self.blob_hash)
    }
}

pub(crate) fn publication_identifiers(
    unique_identifier: Option<&str>,
    blob_hash: &str,
) -> Vec<String> {
    unique_identifier
        .map(str::trim)
        .filter(|identifier| !identifier.is_empty())
        .map(str::to_owned)
        .into_iter()
        .chain(std::iter::once(format!("urn:blake3:{blob_hash}")))
        .collect()
}

#[derive(Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportReport {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub skipped: Vec<SkippedAnnotation>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkippedAnnotation {
    pub index: usize,
    pub reason: SkipReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SkipReason {
    Invalid,
    SourceNotInReadingOrder,
    EmptyNote,
    UnsupportedBody,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ImportError {
    Malformed,
    PublicationMismatch,
}

pub(crate) fn export_annotation_set(
    publication: &ExchangePublication<'_>,
    records: &[AnnotationRecord],
    set_id: [u8; 16],
    generated: OffsetDateTime,
) -> Result<String, String> {
    let mut ordered = records.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|record| (record.occurrence(), document_text_start(record)));
    let items = ordered
        .into_iter()
        .map(export_annotation)
        .collect::<Result<Vec<_>, _>>()?;
    let title = (!publication.title.is_empty())
        .then(|| LocalizableText::Plain(publication.title.to_owned()));
    let about = AnnotationAbout::builder()
        .identifiers(publication.identifiers())
        .format("application/epub+zip".to_owned())
        .maybe_title(title)
        .creators(
            publication
                .creators
                .iter()
                .cloned()
                .map(LocalizableText::Plain)
                .collect(),
        )
        .build()
        .map_err(|error| error.to_string())?;
    AnnotationSet::builder()
        .id(format!("urn:uuid:{}", format_uuid(set_id, 4)))
        .about(about)
        .items(items)
        .generated(generated)
        .build()
        .map_err(|error| error.to_string())?
        .to_json_string()
        .map_err(|error| error.to_string())
}

fn export_annotation(record: &AnnotationRecord) -> Result<Annotation, String> {
    let (motivation, excerpt, section_label, body_text) = match record {
        AnnotationRecord::Highlight {
            excerpt,
            section_label,
            ..
        } => (
            AnnotationMotivation::Highlighting,
            excerpt.as_deref(),
            section_label.as_deref(),
            None,
        ),
        AnnotationRecord::Note {
            excerpt,
            section_label,
            text,
            ..
        } => (
            AnnotationMotivation::Commenting,
            excerpt.as_deref(),
            section_label.as_deref(),
            Some(text.as_str()),
        ),
        AnnotationRecord::Bookmark {
            section_label,
            label,
            ..
        } => (
            AnnotationMotivation::Bookmarking,
            None,
            section_label.as_deref(),
            label.as_deref(),
        ),
    };
    let stored =
        AnnotationTarget::parse_json(record.target_json()).map_err(|error| error.to_string())?;
    let target = AnnotationTarget::builder()
        .source(stored.source().unwrap_or_default().to_owned())
        .selectors(stored.selectors().to_vec())
        .maybe_meta(target_meta(section_label, excerpt))
        .build()
        .map_err(|error| error.to_string())?;
    let created = parse_annotation_timestamp(record.created())
        .ok_or_else(|| "annotation created timestamp is invalid".to_owned())?;
    let modified = parse_annotation_timestamp(record.modified())
        .ok_or_else(|| "annotation modified timestamp is invalid".to_owned())?;
    let creator = record
        .creator()
        .map(|creator| {
            AnnotationCreator::builder()
                .id(creator.id.clone())
                .creator_type(match creator.creator_type.as_str() {
                    "Organization" => AnnotationCreatorType::Organization,
                    "Software" => AnnotationCreatorType::Software,
                    _ => AnnotationCreatorType::Person,
                })
                .maybe_name(creator.name.clone().map(LocalizableText::Plain))
                .build()
        })
        .transpose()
        .map_err(|error| error.to_string())?;
    let body = body_text
        .map(AnnotationBody::text)
        .transpose()
        .map_err(|error| error.to_string())?;
    Annotation::builder()
        .id(format!("urn:uuid:{}", record.id()))
        .created(created)
        .modified(modified)
        .motivation(motivation)
        .target(target)
        .maybe_creator(creator)
        .maybe_body(body)
        .build()
        .map_err(|error| error.to_string())
}

fn document_text_start(record: &AnnotationRecord) -> u64 {
    AnnotationTarget::parse_json(record.target_json())
        .ok()
        .and_then(|target| {
            target
                .selectors()
                .iter()
                .find_map(|selector| match selector {
                    AnnotationSelector::TextPosition(position) => position.start(),
                    _ => None,
                })
        })
        .unwrap_or(u64::MAX)
}

/// Follows the Readium annotation `meta` shape, since EPUB Annotations 1.0 leaves it undefined.
fn target_meta(section_label: Option<&str>, excerpt: Option<&str>) -> Option<JsonObject> {
    let mut meta = JsonObject::new();
    if let Some(label) = section_label {
        meta.insert("headings".to_owned(), json!([{ "level": 1, "txt": label }]));
    }
    if let Some(excerpt) = excerpt {
        meta.insert("text".to_owned(), json!({ "highlight": excerpt }));
    }
    (!meta.is_empty()).then_some(meta)
}

pub(crate) fn parse_annotation_set(
    input: &str,
    publication: &ExchangePublication<'_>,
    allow_publication_mismatch: bool,
) -> Result<AnnotationSet, ImportError> {
    let set = AnnotationSet::parse_json(input).map_err(|_| ImportError::Malformed)?;
    if !allow_publication_mismatch {
        let declared = set.about().map(AnnotationAbout::identifiers).unwrap_or(&[]);
        let known = publication.identifiers();
        if !declared.is_empty() && !declared.iter().any(|value| known.contains(value)) {
            return Err(ImportError::PublicationMismatch);
        }
    }
    Ok(set)
}

pub(crate) fn merge_annotation_set(
    set: &AnnotationSet,
    publication: &ExchangePublication<'_>,
    existing: &[AnnotationRecord],
) -> (Option<Vec<AnnotationRecord>>, ImportReport) {
    let mut records = existing.to_vec();
    let mut report = ImportReport::default();
    for (index, annotation) in set.items().iter().enumerate() {
        let incoming = match import_annotation(annotation, publication) {
            Ok(record) => record,
            Err(reason) => {
                report.skipped.push(SkippedAnnotation { index, reason });
                continue;
            }
        };
        match records
            .iter_mut()
            .find(|record| record.id() == incoming.id())
        {
            Some(current) => {
                let newer = parse_annotation_timestamp(incoming.modified())
                    > parse_annotation_timestamp(current.modified());
                if newer {
                    *current = incoming;
                    report.updated += 1;
                } else {
                    report.unchanged += 1;
                }
            }
            None => {
                records.push(incoming);
                report.added += 1;
            }
        }
    }
    let changed = report.added > 0 || report.updated > 0;
    (changed.then_some(records), report)
}

fn import_annotation(
    annotation: &Annotation,
    publication: &ExchangePublication<'_>,
) -> Result<AnnotationRecord, SkipReason> {
    let id = annotation.id().ok_or(SkipReason::Invalid)?;
    let created = annotation
        .created()
        .and_then(parse_annotation_timestamp)
        .ok_or(SkipReason::Invalid)?;
    let modified = match annotation.modified() {
        Some(value) => parse_annotation_timestamp(value).ok_or(SkipReason::Invalid)?,
        None => created,
    };
    if modified < created {
        return Err(SkipReason::Invalid);
    }
    let parsed = annotation.target().ok_or(SkipReason::Invalid)?;
    let target = AnnotationTarget::builder()
        .source(parsed.source().unwrap_or_default().to_owned())
        .selectors(parsed.selectors().to_vec())
        .build()
        .map_err(|_| SkipReason::Invalid)?;
    let resource = resolve_publication_href(
        &AuthoredHref::new(target.source().unwrap_or_default()),
        publication.package_path,
    )
    .ok_or(SkipReason::SourceNotInReadingOrder)?;
    let occurrence = publication
        .reading_order_resources
        .iter()
        .position(|candidate| candidate.as_deref() == Some(resource.as_str()))
        .and_then(|occurrence| u32::try_from(occurrence).ok())
        .ok_or(SkipReason::SourceNotInReadingOrder)?;
    let target_json = target.to_json_string().map_err(|_| SkipReason::Invalid)?;
    let body_text = match annotation.body() {
        None => None,
        Some(body) => match body.value() {
            Some(LocalizableText::Plain(text)) | Some(LocalizableText::Localized { text, .. }) => {
                Some(text.clone())
            }
            None => return Err(SkipReason::UnsupportedBody),
        },
    };
    let meta = parsed.meta();
    let section_label = meta
        .and_then(|meta| meta.get("headings"))
        .and_then(Value::as_array)
        .and_then(|headings| headings.last())
        .and_then(|heading| heading.get("txt"))
        .and_then(Value::as_str)
        .map(|label| bounded(label, MAX_LABEL_CODE_POINTS, MAX_LABEL_BYTES));
    let excerpt = meta
        .and_then(|meta| meta.get("text"))
        .and_then(|text| text.get("highlight"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| text_fragment_exact(target.selectors()))
        .map(|excerpt| bounded(&excerpt, MAX_EXCERPT_CODE_POINTS, MAX_EXCERPT_BYTES));
    let creator = annotation.creator().and_then(|creator| {
        Some(AnnotationCreatorRecord {
            id: creator.id()?.to_owned(),
            creator_type: creator.creator_type()?.as_str().to_owned(),
            name: creator.name().map(|name| match name {
                LocalizableText::Plain(text) | LocalizableText::Localized { text, .. } => {
                    bounded(text, MAX_LABEL_CODE_POINTS, MAX_LABEL_BYTES)
                }
            }),
        })
    });
    let id = local_annotation_id(id);
    let created = format_annotation_timestamp(created).map_err(|_| SkipReason::Invalid)?;
    let modified = format_annotation_timestamp(modified).map_err(|_| SkipReason::Invalid)?;
    let resource_hint = resource.as_str().to_owned();
    let record = match (annotation.motivation(), body_text) {
        (Some(AnnotationMotivation::Bookmarking), label) => AnnotationRecord::Bookmark {
            id,
            target_json,
            occurrence,
            section_label,
            label: label.map(|label| bounded(&label, MAX_LABEL_CODE_POINTS, MAX_LABEL_BYTES)),
            created,
            modified,
            creator,
            resource_hint,
        },
        (_, Some(text)) => {
            if text.trim().is_empty() {
                return Err(SkipReason::EmptyNote);
            }
            AnnotationRecord::Note {
                id,
                target_json,
                occurrence,
                excerpt,
                section_label,
                text,
                created,
                modified,
                creator,
                resource_hint,
            }
        }
        (_, None) => AnnotationRecord::Highlight {
            id,
            target_json,
            occurrence,
            excerpt,
            section_label,
            created,
            modified,
            creator,
            resource_hint,
        },
    };
    Ok(record)
}

/// Keeps `urn:uuid:` identities and derives a stable UUID for any other annotation IRI, so
/// importing the same file twice updates rather than duplicates.
fn local_annotation_id(id: &str) -> String {
    if let Some(uuid) = id
        .strip_prefix("urn:uuid:")
        .map(str::to_ascii_lowercase)
        .filter(|uuid| is_uuid(uuid))
    {
        return uuid;
    }
    let hash = blake3::hash(id.as_bytes());
    let mut value = [0_u8; 16];
    value.copy_from_slice(&hash.as_bytes()[..16]);
    format_uuid(value, 8)
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn text_fragment_exact(selectors: &[AnnotationSelector]) -> Option<String> {
    selectors.iter().find_map(|selector| {
        let AnnotationSelector::Fragment(fragment) = selector else {
            return None;
        };
        if fragment.conforms_to() != Some(&FragmentConformsTo::TextFragment) {
            return None;
        }
        let components = fragment
            .value()?
            .strip_prefix("text=")?
            .split(',')
            .collect::<Vec<_>>();
        let start = usize::from(components.first()?.ends_with('-'));
        let end = components.len() - usize::from(components.last()?.starts_with('-'));
        let decode = |component: &str| {
            percent_encoding::percent_decode_str(component)
                .decode_utf8()
                .ok()
                .map(|value| value.into_owned())
        };
        match components.get(start..end)? {
            [exact] => decode(exact),
            [first, last] => Some(format!("{} … {}", decode(first)?, decode(last)?)),
            _ => None,
        }
    })
}

fn bounded(value: &str, max_code_points: usize, max_bytes: usize) -> String {
    let mut result = String::new();
    for character in value.chars().take(max_code_points) {
        if result.len() + character.len_utf8() > max_bytes {
            break;
        }
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLICATION_HASH: &str = "ab";

    fn publication(resources: &[Option<String>]) -> ExchangePublication<'_> {
        static PACKAGE: std::sync::OnceLock<EpubPath> = std::sync::OnceLock::new();
        ExchangePublication {
            unique_identifier: Some("urn:isbn:9780000000000"),
            blob_hash: PUBLICATION_HASH,
            title: "Moby-Dick",
            creators: &[],
            package_path: PACKAGE.get_or_init(|| EpubPath::new("EPUB/package.opf").unwrap()),
            reading_order_resources: resources,
        }
    }

    fn resources() -> Vec<Option<String>> {
        vec![None, Some("EPUB/chapter.xhtml".to_owned())]
    }

    fn note(id: &str, modified: &str, text: &str) -> AnnotationRecord {
        AnnotationRecord::Note {
            id: id.to_owned(),
            target_json: AnnotationTarget::parse_json(
                r#"{"source":"chapter.xhtml","selector":[{"type":"FragmentSelector","conformsTo":"http://tools.ietf.org/rfc/rfc3986","value":"p1"}]}"#,
            )
            .unwrap()
            .to_json_string()
            .unwrap(),
            occurrence: 1,
            excerpt: Some("Call me Ishmael.".to_owned()),
            section_label: Some("Loomings".to_owned()),
            text: text.to_owned(),
            created: "2026-01-01T00:00:00.000Z".to_owned(),
            modified: modified.to_owned(),
            creator: None,
            resource_hint: "EPUB/chapter.xhtml".to_owned(),
        }
    }

    fn set_json(records: &[AnnotationRecord], resources: &[Option<String>]) -> String {
        export_annotation_set(
            &publication(resources),
            records,
            [7; 16],
            OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap()
    }

    #[test]
    fn export_then_import_reproduces_records() {
        let resources = resources();
        let id = format_uuid([1; 16], 4);
        let records = vec![
            note(&id, "2026-01-02T00:00:00.000Z", "A note"),
            AnnotationRecord::Bookmark {
                id: format_uuid([2; 16], 4),
                target_json: r#"{"source":"chapter.xhtml"}"#.to_owned(),
                occurrence: 1,
                section_label: None,
                label: Some("Start".to_owned()),
                created: "2026-01-01T00:00:00.000Z".to_owned(),
                modified: "2026-01-01T00:00:00.000Z".to_owned(),
                creator: Some(AnnotationCreatorRecord {
                    id: "https://example.com/reader".to_owned(),
                    creator_type: "Person".to_owned(),
                    name: Some("Reader".to_owned()),
                }),
                resource_hint: "EPUB/chapter.xhtml".to_owned(),
            },
        ];
        let json = set_json(&records, &resources);
        let set = parse_annotation_set(&json, &publication(&resources), false).unwrap();
        let (merged, report) = merge_annotation_set(&set, &publication(&resources), &[]);
        assert_eq!(report.added, 2);
        assert!(report.skipped.is_empty());
        let merged = merged.unwrap();
        assert_eq!(
            serde_json::to_value(&merged).unwrap(),
            serde_json::to_value(&records).unwrap()
        );
    }

    #[test]
    fn merge_keeps_newer_local_records_and_counts_outcomes() {
        let resources = resources();
        let first = format_uuid([1; 16], 4);
        let second = format_uuid([2; 16], 4);
        let third = format_uuid([3; 16], 4);
        let incoming = set_json(
            &[
                note(&first, "2026-01-05T00:00:00.000Z", "newer"),
                note(&second, "2026-01-01T00:00:00.000Z", "older"),
                note(&third, "2026-01-01T00:00:00.000Z", "new"),
            ],
            &resources,
        );
        let existing = vec![
            note(&first, "2026-01-02T00:00:00.000Z", "local"),
            note(&second, "2026-01-03T00:00:00.000Z", "local"),
        ];
        let set = parse_annotation_set(&incoming, &publication(&resources), false).unwrap();
        let (merged, report) = merge_annotation_set(&set, &publication(&resources), &existing);
        assert_eq!((report.added, report.updated, report.unchanged), (1, 1, 1));
        let merged = merged.unwrap();
        assert!(matches!(&merged[0], AnnotationRecord::Note { text, .. } if text == "newer"));
        assert!(matches!(&merged[1], AnnotationRecord::Note { text, .. } if text == "local"));
        assert_eq!(merged.len(), 3);

        let (unchanged, report) = merge_annotation_set(&set, &publication(&resources), &merged);
        assert!(unchanged.is_none());
        assert_eq!(report.unchanged, 3);
    }

    #[test]
    fn import_skips_sources_outside_reading_order_and_checks_publication() {
        let resources = resources();
        let json = set_json(
            &[note(
                &format_uuid([1; 16], 4),
                "2026-01-01T00:00:00.000Z",
                "x",
            )],
            &resources,
        );
        let other = vec![Some("EPUB/other.xhtml".to_owned())];
        let set = parse_annotation_set(&json, &publication(&other), false).unwrap();
        let (merged, report) = merge_annotation_set(&set, &publication(&other), &[]);
        assert!(merged.is_none());
        assert_eq!(
            report.skipped[0].reason,
            SkipReason::SourceNotInReadingOrder
        );

        let mismatched = json.replace("urn:isbn:9780000000000", "urn:isbn:1");
        let mismatched = mismatched.replace("urn:blake3:ab", "urn:blake3:cd");
        assert_eq!(
            parse_annotation_set(&mismatched, &publication(&resources), false).unwrap_err(),
            ImportError::PublicationMismatch
        );
        assert!(parse_annotation_set(&mismatched, &publication(&resources), true).is_ok());
    }

    #[test]
    fn foreign_quote_only_annotations_import_with_stable_ids_and_excerpts() {
        let resources = resources();
        let json = serde_json::json!({
            "@context": "http://www.idpf.org/epub/annotations/v1",
            "type": "AnnotationSet",
            "items": [{
                "id": "koreader:highlight:12",
                "type": "Annotation",
                "motivation": "highlighting",
                "created": "2024-03-04T05:06:07+02:00",
                "target": {
                    "source": "chapter.xhtml",
                    "selector": [{
                        "type": "FragmentSelector",
                        "conformsTo": "https://wicg.github.io/scroll-to-text-fragment/",
                        "value": "text=me-,Ishmael%2E,-Some"
                    }]
                }
            }]
        })
        .to_string();
        let set = parse_annotation_set(&json, &publication(&resources), false).unwrap();
        let (first, _) = merge_annotation_set(&set, &publication(&resources), &[]);
        let first = first.unwrap();
        let AnnotationRecord::Highlight {
            id,
            excerpt,
            created,
            ..
        } = &first[0]
        else {
            panic!("expected highlight");
        };
        assert_eq!(excerpt.as_deref(), Some("Ishmael."));
        assert_eq!(created, "2024-03-04T03:06:07.000Z");
        assert!(is_uuid(id));
        let (again, report) = merge_annotation_set(&set, &publication(&resources), &first);
        assert!(again.is_none());
        assert_eq!(report.unchanged, 1);
    }
}
