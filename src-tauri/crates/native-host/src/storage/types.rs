//! What the store hands back: its row projections, its preference and policy
//! values, and the small logic that belongs to them.

use super::{
    ActiveContentDefault, ActiveContentOverride, AnnotationProjection, AnnotationRecord,
    EffectiveActiveContentPolicy, EffectivePublicationPolicy, EffectiveRemoteResourcesPolicy,
    GlobalPublicationPolicy, GlobalReaderPreferences, InstalledBlob,
    MAX_NARRATION_SKIPPED_TYPE_BYTES, MAX_NARRATION_SKIPPED_TYPES, NARRATION_RATES,
    PolicyRelevance, PublicAnnotationProjection, PublicAnnotationRecord, PublicationPolicy,
    PublicationPolicyRelevance, PublicationReaderPreferences, ReaderPreferences,
    RemoteResourcesDefault, RemoteResourcesOverride, default_narration_rate,
};
use crate::media_overlay::MediaOverlayDescription;
use crate::{ReadingPosition, ResponseManifest};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone)]
pub struct PublicationSummary {
    pub publication_id: String,
    pub title: String,
    /// The direction the title reads, when the publication declared one. Index aligned
    /// `creator_directions` does the same for each creator. The text stays plain so
    /// sorting and search do not have to unwrap it.
    pub title_direction: Option<String>,
    pub creators: Vec<String>,
    pub creator_directions: Vec<Option<String>>,
    pub language: Option<String>,
    pub reading_direction: Option<String>,
    pub imported_at: String,
    pub last_read_at: Option<String>,
    pub has_cover: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicationMetadata {
    pub(super) title_direction: Option<String>,
    pub(super) creators: Vec<String>,
    pub(super) creator_directions: Vec<Option<String>>,
    pub(super) language: Option<String>,
    pub(super) reading_direction: Option<String>,
}

impl PublicationMetadata {
    pub(super) fn creators_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.creators)
            .map_err(|error| format!("could not serialize publication creators: {error}"))
    }

    pub(super) fn creator_directions_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.creator_directions)
            .map_err(|error| format!("could not serialize publication creator directions: {error}"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InstallationOriginConfig {
    pub foreground_port: u16,
    pub reader_port: Option<u16>,
}

pub(crate) enum PublicationSelection {
    Ready(Box<SelectedPublication>),
    Warm {
        snapshot_id: String,
        browser_origin_id: String,
        reading_position: Option<ReadingPosition>,
        position_revision: u32,
        annotations: AnnotationProjection,
    },
    NeedsDerivation(InstalledBlob),
    NotFound,
    SourceUnavailable,
    Revoked,
}

/// One stored timeline, held as the exact response bytes so serving never parses or clones.
#[derive(Debug, Clone)]
pub(crate) struct MediaOverlayTimelineFile {
    pub path: PathBuf,
    pub length: u64,
}

#[derive(Debug, Clone)]
pub struct MediaSidecar {
    pub path: PathBuf,
    pub length: u64,
    /// Response media type when the sidecar is not a copy of the resource itself.
    pub media_type: Option<String>,
}

pub(crate) struct SelectedPublication {
    pub blob_path: PathBuf,
    pub snapshot_id: String,
    pub browser_origin_id: String,
    pub facts: Value,
    pub responses: ResponseManifest,
    pub media_overlay: MediaOverlayDescription,
    pub media_overlay_capability: crate::media_overlay::MediaOverlayCapability,
    pub media_overlay_timeline_files: BTreeMap<u32, MediaOverlayTimelineFile>,
    pub media_sidecars: BTreeMap<String, MediaSidecar>,
    pub policy_relevance: PublicationPolicyRelevance,
    pub reading_position: Option<ReadingPosition>,
    pub position_revision: u32,
    pub annotations: AnnotationProjection,
}

impl Serialize for PublicAnnotationProjection<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::{SerializeSeq, SerializeStruct};

        struct PublicRecords<'a>(&'a [AnnotationRecord]);

        impl Serialize for PublicRecords<'_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let mut records = serializer.serialize_seq(Some(self.0.len()))?;
                for record in self.0 {
                    records.serialize_element(&PublicAnnotationRecord(record))?;
                }
                records.end()
            }
        }

        let mut projection = serializer.serialize_struct("AnnotationProjection", 2)?;
        projection.serialize_field("revision", &self.0.revision)?;
        projection.serialize_field("records", &PublicRecords(&self.0.records))?;
        projection.end()
    }
}

impl Serialize for PublicAnnotationRecord<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        match self.0 {
            AnnotationRecord::Highlight {
                id,
                target_json,
                occurrence,
                excerpt,
                section_label,
                ..
            } => {
                let mut record = serializer.serialize_struct("Highlight", 6)?;
                record.serialize_field("type", "highlight")?;
                record.serialize_field("id", id)?;
                record.serialize_field("targetJson", target_json)?;
                record.serialize_field("occurrence", occurrence)?;
                record.serialize_field("excerpt", excerpt)?;
                record.serialize_field("sectionLabel", section_label)?;
                record.end()
            }
            AnnotationRecord::Note {
                id,
                target_json,
                occurrence,
                excerpt,
                section_label,
                text,
                ..
            } => {
                let mut record = serializer.serialize_struct("Note", 7)?;
                record.serialize_field("type", "note")?;
                record.serialize_field("id", id)?;
                record.serialize_field("targetJson", target_json)?;
                record.serialize_field("occurrence", occurrence)?;
                record.serialize_field("excerpt", excerpt)?;
                record.serialize_field("sectionLabel", section_label)?;
                record.serialize_field("text", text)?;
                record.end()
            }
            AnnotationRecord::Bookmark {
                id,
                target_json,
                occurrence,
                section_label,
                label,
                ..
            } => {
                let mut record = serializer.serialize_struct("Bookmark", 6)?;
                record.serialize_field("type", "bookmark")?;
                record.serialize_field("id", id)?;
                record.serialize_field("targetJson", target_json)?;
                record.serialize_field("occurrence", occurrence)?;
                record.serialize_field("sectionLabel", section_label)?;
                record.serialize_field("label", label)?;
                record.end()
            }
        }
    }
}

#[cfg(test)]
pub(crate) struct StoredReadingPosition {
    pub position_json: String,
    pub revision: u32,
}

pub(crate) enum SaveReadingPositionOutcome {
    Saved { revision: u32 },
    RevisionMismatch { current_revision: u32 },
    RevisionExhausted { current_revision: u32 },
}

pub(crate) const MAX_PAGINATION_RECORD_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PAGINATION_BULK_OPERATIONS: usize = 32;
pub(crate) const MAX_PAGINATION_BULK_BYTES: usize =
    MAX_PAGINATION_BULK_OPERATIONS * MAX_PAGINATION_RECORD_BYTES;
pub(crate) const MAX_PAGINATION_SNAPSHOT_BYTES: i64 = 4 * 1024 * 1024;
pub(crate) const MAX_PAGINATION_BASES_PER_SNAPSHOT: i64 = 3;
pub(crate) const MAX_PAGINATION_GLOBAL_BYTES: i64 = 64 * 1024 * 1024;

impl Default for ReaderPreferences {
    fn default() -> Self {
        Self {
            color_scheme: "system".to_owned(),
            presentation_mode: "auto".to_owned(),
            pagination_columns: "auto".to_owned(),
            publication_theme: "original".to_owned(),
            publication_typeface: "reader-serif".to_owned(),
            font_scale: "125".to_owned(),
            line_height: "original".to_owned(),
            letter_spacing: "0".to_owned(),
            word_spacing: "0".to_owned(),
            paragraph_spacing: "original".to_owned(),
            text_indent: "original".to_owned(),
            measure: "wide".to_owned(),
            alignment: "original".to_owned(),
            hyphenation: "original".to_owned(),
            keyboard_mode: "standard".to_owned(),
            touch_zones: true,
            click_zones: false,
            narration_rate: default_narration_rate(),
            narration_skipped_types: Vec::new(),
        }
    }
}

/// Whether a stored string is one of a kernel enum's serialized values.
///
/// The reader's presentation vocabulary belongs to the Navigator kernel, so the host asks the
/// kernel rather than keeping a second copy of every value. A variant added upstream widens
/// this automatically, which is what a hand-written copy of the list failed to do: a face the
/// reader could choose stayed a face the host refused, and the reader saw a failed save.
fn is_kernel_value<T: serde::de::DeserializeOwned>(value: &str) -> bool {
    serde_json::from_value::<T>(serde_json::Value::String(value.to_owned())).is_ok()
}

impl ReaderPreferences {
    pub(crate) fn validate(&self) -> Result<(), String> {
        use epub_navigator::kernel::{
            FontScale, Hyphenation, LetterSpacing, LineHeight, PaginationColumns, ParagraphSpacing,
            PresentationMode, PublicationTheme, PublicationTypeface, ReadingMeasure, TextAlignment,
            TextIndent, WordSpacing,
        };
        // These three are not kernel-owned: the interface scheme admits `system`, which the
        // reader resolves, and keyboard mode and narration rate are not presentation at all.
        let valid = matches!(self.color_scheme.as_str(), "system" | "light" | "dark")
            && matches!(self.keyboard_mode.as_str(), "off" | "standard" | "vim")
            && is_kernel_value::<PresentationMode>(&self.presentation_mode)
            && NARRATION_RATES.contains(&self.narration_rate.as_str())
            && is_kernel_value::<PaginationColumns>(&self.pagination_columns)
            && is_kernel_value::<PublicationTheme>(&self.publication_theme)
            && is_kernel_value::<PublicationTypeface>(&self.publication_typeface)
            && is_kernel_value::<FontScale>(&self.font_scale)
            && is_kernel_value::<LineHeight>(&self.line_height)
            && is_kernel_value::<LetterSpacing>(&self.letter_spacing)
            && is_kernel_value::<WordSpacing>(&self.word_spacing)
            && is_kernel_value::<ParagraphSpacing>(&self.paragraph_spacing)
            && is_kernel_value::<TextIndent>(&self.text_indent)
            && is_kernel_value::<ReadingMeasure>(&self.measure)
            && is_kernel_value::<TextAlignment>(&self.alignment)
            && is_kernel_value::<Hyphenation>(&self.hyphenation)
            && self.narration_skipped_types.len() <= MAX_NARRATION_SKIPPED_TYPES
            && self
                .narration_skipped_types
                .iter()
                .enumerate()
                .all(|(index, token)| {
                    !token.is_empty()
                        && token.len() <= MAX_NARRATION_SKIPPED_TYPE_BYTES
                        && !token.chars().any(char::is_whitespace)
                        && !self.narration_skipped_types[..index].contains(token)
                });
        valid
            .then_some(())
            .ok_or_else(|| "reader preferences are invalid".to_owned())
    }

    pub(super) fn split(
        &self,
    ) -> Result<(GlobalReaderPreferences, PublicationReaderPreferences), String> {
        self.validate()?;
        Ok((
            GlobalReaderPreferences {
                version: 1,
                reading_defaults: None,
                side_buttons_turn_pages: None,
                color_scheme: self.color_scheme.clone(),
                publication_theme: self.publication_theme.clone(),
                publication_typeface: self.publication_typeface.clone(),
                font_scale: self.font_scale.clone(),
                line_height: self.line_height.clone(),
                letter_spacing: self.letter_spacing.clone(),
                word_spacing: self.word_spacing.clone(),
                paragraph_spacing: self.paragraph_spacing.clone(),
                text_indent: self.text_indent.clone(),
                measure: self.measure.clone(),
                alignment: self.alignment.clone(),
                hyphenation: self.hyphenation.clone(),
                keyboard_mode: self.keyboard_mode.clone(),
                touch_zones: self.touch_zones,
                click_zones: self.click_zones,
                narration_rate: self.narration_rate.clone(),
                narration_skipped_types: self.narration_skipped_types.clone(),
            },
            PublicationReaderPreferences {
                version: 1,
                presentation_mode: self.presentation_mode.clone(),
                pagination_columns: self.pagination_columns.clone(),
            },
        ))
    }
}

impl Default for GlobalReaderPreferences {
    fn default() -> Self {
        let (global, _) = ReaderPreferences::default()
            .split()
            .expect("default reader preferences must be valid");
        global
    }
}

impl Default for PublicationReaderPreferences {
    fn default() -> Self {
        let (_, publication) = ReaderPreferences::default()
            .split()
            .expect("default reader preferences must be valid");
        publication
    }
}

pub(crate) fn compose_preferences(
    global: GlobalReaderPreferences,
    publication: PublicationReaderPreferences,
) -> Result<ReaderPreferences, String> {
    if global.version != 1 || publication.version != 1 {
        return Err("reader preference version is unsupported".to_owned());
    }
    let preferences = ReaderPreferences {
        color_scheme: global.color_scheme,
        presentation_mode: publication.presentation_mode,
        pagination_columns: publication.pagination_columns,
        publication_theme: global.publication_theme,
        publication_typeface: global.publication_typeface,
        font_scale: global.font_scale,
        line_height: global.line_height,
        letter_spacing: global.letter_spacing,
        word_spacing: global.word_spacing,
        paragraph_spacing: global.paragraph_spacing,
        text_indent: global.text_indent,
        measure: global.measure,
        alignment: global.alignment,
        hyphenation: global.hyphenation,
        keyboard_mode: global.keyboard_mode,
        touch_zones: global.touch_zones,
        click_zones: global.click_zones,
        narration_rate: global.narration_rate,
        narration_skipped_types: global.narration_skipped_types,
    };
    preferences.validate()?;
    Ok(preferences)
}

impl Default for GlobalPublicationPolicy {
    fn default() -> Self {
        Self {
            active_content: ActiveContentDefault::Ask,
            remote_resources: RemoteResourcesDefault::Ask,
        }
    }
}

impl PublicationPolicy {
    pub(crate) fn effective(&self) -> EffectivePublicationPolicy {
        let active_content = match self.publication.active_content {
            Some(ActiveContentOverride::Enabled) => EffectiveActiveContentPolicy::Enabled,
            Some(ActiveContentOverride::Disabled) => EffectiveActiveContentPolicy::Disabled,
            None => match self.global.active_content {
                ActiveContentDefault::Automatic => EffectiveActiveContentPolicy::Enabled,
                ActiveContentDefault::Ask => EffectiveActiveContentPolicy::Ask,
                ActiveContentDefault::Disabled => EffectiveActiveContentPolicy::Disabled,
            },
        };
        let remote_resources = match self.publication.remote_resources {
            Some(RemoteResourcesOverride::Allowed) => EffectiveRemoteResourcesPolicy::Allowed,
            Some(RemoteResourcesOverride::Blocked) => EffectiveRemoteResourcesPolicy::Blocked,
            None => match self.global.remote_resources {
                RemoteResourcesDefault::Allowed => EffectiveRemoteResourcesPolicy::Allowed,
                RemoteResourcesDefault::Ask => EffectiveRemoteResourcesPolicy::Ask,
                RemoteResourcesDefault::Blocked => EffectiveRemoteResourcesPolicy::Blocked,
            },
        };
        EffectivePublicationPolicy {
            active_content,
            remote_resources,
        }
    }
}

impl ActiveContentDefault {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Ask => "ask",
            Self::Disabled => "disabled",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "automatic" => Ok(Self::Automatic),
            "ask" => Ok(Self::Ask),
            "disabled" => Ok(Self::Disabled),
            _ => Err("stored global active-content policy is invalid".to_owned()),
        }
    }
}

impl RemoteResourcesDefault {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Ask => "ask",
            Self::Blocked => "blocked",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "allowed" => Ok(Self::Allowed),
            "ask" => Ok(Self::Ask),
            "blocked" => Ok(Self::Blocked),
            _ => Err("stored global remote-resource policy is invalid".to_owned()),
        }
    }
}

impl ActiveContentOverride {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            _ => Err("stored publication active-content policy is invalid".to_owned()),
        }
    }
}

impl RemoteResourcesOverride {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Blocked => "blocked",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "allowed" => Ok(Self::Allowed),
            "blocked" => Ok(Self::Blocked),
            _ => Err("stored publication remote-resource policy is invalid".to_owned()),
        }
    }
}

impl PolicyRelevance {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::NotDetected => "not-detected",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "detected" => Ok(Self::Detected),
            "not-detected" => Ok(Self::NotDetected),
            "unknown" => Ok(Self::Unknown),
            _ => Err("stored publication policy relevance is invalid".to_owned()),
        }
    }
}
