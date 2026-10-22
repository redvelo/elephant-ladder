//! The media-overlay vocabulary: what the host derived, and how available each
//! part of it is. Every value here is serialized into a snapshot, so each
//! availability state is explicit rather than an absent field.

use serde::{Deserialize, Serialize};

use super::validate::MediaOverlayTimelineResultRef;

pub(crate) const MAX_MEDIA_OVERLAY_DECLARATIONS: usize = 100_000;
pub(crate) const MAX_MEDIA_OVERLAY_TIMELINES: usize = 10_000;
pub(crate) const MAX_MEDIA_OVERLAY_ASSOCIATIONS: usize = 100_000;
pub(crate) const MAX_MEDIA_OVERLAY_SEGMENTS: usize = 100_000;
pub(crate) const MAX_MEDIA_OVERLAY_SEMANTIC_DEPTH: usize = 256;
pub(crate) const MAX_MEDIA_OVERLAY_TOKENS_PER_NODE: usize = 256;
pub(crate) const MAX_MEDIA_OVERLAY_COLLECTION_ITEMS: usize = 10_000;
pub(crate) const MAX_MEDIA_OVERLAY_AMBIGUOUS_CANDIDATES: usize = 256;
pub(crate) const MAX_MEDIA_OVERLAY_COVERAGE_ENTRIES: usize = 10_000;
pub(crate) const MAX_MEDIA_OVERLAY_COVERAGE_ISSUES: usize = 10_000;
pub(crate) const MAX_MEDIA_OVERLAY_STRING_BYTES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayDescription {
    pub package: MediaOverlayPackageFacts,
    pub coverage: MediaOverlayCoverage,
    pub playback: MediaOverlayPlayback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayPackageFacts {
    Available {
        declared: bool,
        declarations: Vec<MediaOverlayDeclaration>,
        metadata: MediaOverlayPackageMetadata,
    },
    Unavailable {
        reason: MediaOverlayPackageIssue,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayPackageIssue {
    RepresentationLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayPackageMetadata {
    pub total_durations: Vec<MediaOverlayDurationMetadata>,
    pub item_durations: Vec<MediaOverlayDurationMetadata>,
    pub narrators: Vec<String>,
    pub active_classes: Vec<String>,
    pub playback_active_classes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayDurationMetadata {
    pub authored: String,
    pub clock: SmilClock,
    pub refines: Option<String>,
    pub declaration_indexes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayDeclaration {
    pub declaration_index: u32,
    pub authored_id: MediaOverlayManifestId,
    pub authored_href: Option<String>,
    pub canonical_path: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayManifestId {
    Missing {},
    Valid { value: String },
    Invalid { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayPlayback {
    Absent {},
    Unavailable {
        reason: MediaOverlayAvailabilityIssue,
    },
    Available {
        overlays: Vec<MediaOverlayTimelineSummary>,
        associations: Vec<MediaOverlayAssociation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayTimelineSummary {
    pub overlay_index: u32,
    pub canonical_path: String,
    pub declarations: Vec<MediaOverlayDeclaration>,
    pub skippable: Vec<String>,
    pub escapable: Vec<String>,
    pub segment_count: u32,
    pub playable_segment_count: u32,
    pub availability: MediaOverlayTimelineAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayTimelineAvailability {
    Available {},
    Unavailable { reason: MediaOverlayTimelineIssue },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg(test)]
pub(crate) enum MediaOverlayTimelineResult {
    Available {
        timeline: MediaOverlayTimeline,
    },
    Unavailable {
        summary: MediaOverlayTimelineSummary,
        reason: MediaOverlayTimelineIssue,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayTimeline {
    pub overlay_index: u32,
    pub canonical_path: String,
    pub declarations: Vec<MediaOverlayDeclaration>,
    pub skippable: Vec<String>,
    pub escapable: Vec<String>,
    pub segment_count: u32,
    pub playable_segment_count: u32,
    pub availability: MediaOverlayTimelineAvailability,
    pub segments: Vec<MediaOverlaySegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayAssociation {
    pub occurrence_index: u32,
    pub effective_linear: EffectiveLinear,
    pub content: ReadingOrderContentTarget,
    pub overlay: OverlayAssociationState,
    pub entry_segment_indexes: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EffectiveLinear {
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ReadingOrderContentTarget {
    MissingIdref {},
    MissingDeclaration {
        authored_idref: String,
    },
    AmbiguousDeclaration {
        authored_idref: String,
        candidates: Vec<MediaOverlayDeclaration>,
    },
    DeclarationWithoutResource {
        declaration: MediaOverlayDeclaration,
    },
    Resolved {
        declaration: MediaOverlayDeclaration,
        canonical_path: String,
        bytes: MediaOverlayLocalBytes,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum MediaOverlayLocalBytes {
    Present {},
    Missing {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum OverlayAssociationState {
    NotDeclared {},
    MissingDeclaration {
        authored_idref: String,
    },
    AmbiguousDeclaration {
        authored_idref: String,
        candidates: Vec<MediaOverlayDeclaration>,
    },
    DeclarationWithoutResource {
        declaration: MediaOverlayDeclaration,
    },
    AnalysisUnavailable {
        declaration: MediaOverlayDeclaration,
        overlay_index: u32,
        issue: MediaOverlayAnalysisIssue,
    },
    Resolved {
        declaration: MediaOverlayDeclaration,
        overlay_index: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlaySegment {
    pub segment_index: u32,
    pub semantic_path: Vec<MediaOverlaySemanticNode>,
    pub text: MediaOverlayBinding,
    pub audio: MediaOverlayBinding,
    pub clip_begin: SmilClock,
    pub clip_end: SmilClock,
    pub playability: MediaOverlaySegmentPlayability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlaySemanticNode {
    pub kind: MediaOverlaySemanticNodeKind,
    pub epub_types: Vec<String>,
    pub text_reference: Option<MediaOverlayReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlaySemanticNodeKind {
    Sequence,
    Parallel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum SmilClock {
    Absent {},
    Parsed { milliseconds: String },
    Unrecognized {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayReference {
    Absent {},
    LocalResource {
        authored_href: String,
        canonical_path: String,
        query: Option<String>,
    },
    LocalFragment {
        authored_href: String,
        canonical_path: String,
        query: Option<String>,
        fragment: String,
        fragment_exists: FragmentExistence,
    },
    MissingLocalResource {
        authored_href: String,
        canonical_path: String,
    },
    IndexedLocalResourceUnavailable {
        authored_href: String,
        canonical_path: String,
        query: Option<String>,
    },
    IndexedLocalFragmentUnavailable {
        authored_href: String,
        canonical_path: String,
        query: Option<String>,
        fragment: String,
        fragment_exists: FragmentExistence,
    },
    Remote {
        authored_href: String,
    },
    Data {
        authored_href: String,
    },
    External {
        authored_href: String,
    },
    Invalid {
        authored_href: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum FragmentExistence {
    Exists {},
    Ambiguous {},
    Missing {},
    Unknown {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayBinding {
    pub reference: MediaOverlayReference,
    pub availability: MediaOverlayBindingAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayBindingAvailability {
    Available {},
    Unavailable { reason: MediaOverlayBindingIssue },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlaySegmentPlayability {
    Playable {},
    Unsupported {
        reason: MediaOverlayUnsupportedReason,
    },
    Malformed {
        reason: MediaOverlayMalformedReason,
    },
    Unavailable {
        reason: MediaOverlaySegmentUnavailableReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayAvailabilityIssue {
    AnalysisUnavailable,
    RepresentationLimitExceeded,
    NoPlayableSegments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayTimelineIssue {
    AnalysisUnavailable,
    RepresentationLimitExceeded,
    NoPlayableSegments,
    AudioPreparationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayBindingIssue {
    MissingReference,
    MissingResource,
    MissingFragment,
    AmbiguousFragment,
    UnverifiedFragment,
    ResourceBytesUnavailable,
    UnsupportedTarget,
    UnsupportedMediaType,
    AudioBackingUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayUnsupportedReason {
    TextToSpeechUnavailable,
    FixedLayoutText,
    SvgText,
    NavigationDocument,
    UnsupportedTextFragment,
    UnsupportedAudioFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayMalformedReason {
    MissingBody,
    InvalidSequenceChildren,
    InvalidParallelChildren,
    MissingText,
    MultipleText,
    MultipleAudio,
    UnrecognizedClock,
    EmptyClip,
    ReversedClip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlaySegmentUnavailableReason {
    TextBindingUnavailable,
    AudioBindingUnavailable,
    AmbiguousContinuation,
    NumericOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayCoverage {
    pub state: MediaOverlayCoverageState,
    pub package_relationships: Option<MediaOverlayCoverageResult>,
    pub smil_extraction: Vec<MediaOverlayPathCoverage>,
    pub smil_relationships: Vec<MediaOverlayPathCoverage>,
    pub fragment_observations: Vec<MediaOverlayPathCoverage>,
    pub media_inspection: Vec<MediaOverlayPathCoverage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayCoverageState {
    Complete,
    Partial,
    Unavailable,
    Inapplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayCoverageResult {
    Complete {},
    Partial { issue: MediaOverlayAnalysisIssue },
    Unavailable { issue: MediaOverlayAnalysisIssue },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MediaOverlayPathCoverage {
    pub canonical_path: String,
    pub result: MediaOverlayCoverageResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MediaOverlayAnalysisIssue {
    ResourceLimit,
    PerResourceAnalysisLimit,
    TotalAnalysisLimit,
    TotalFingerprintLimit,
    SmilNodeLimit,
    SmilNestingLimit,
    Missing,
    Unreadable,
    Unsupported,
    Malformed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayCapability {
    Absent {},
    Unavailable {},
    Available {
        playable_occurrence_indexes: Vec<u32>,
        unavailable_occurrence_count: u32,
    },
}

impl MediaOverlayTimeline {
    /// Serializes the response for this timeline without cloning it.
    pub(crate) fn to_result_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        let borrowed = match &self.availability {
            MediaOverlayTimelineAvailability::Available {} => {
                MediaOverlayTimelineResultRef::Available { timeline: self }
            }
            MediaOverlayTimelineAvailability::Unavailable { reason } => {
                MediaOverlayTimelineResultRef::Unavailable {
                    summary: self.summary(),
                    reason: *reason,
                }
            }
        };
        serde_json::to_vec(&borrowed)
    }

    fn summary(&self) -> MediaOverlayTimelineSummary {
        MediaOverlayTimelineSummary {
            overlay_index: self.overlay_index,
            canonical_path: self.canonical_path.clone(),
            declarations: self.declarations.clone(),
            skippable: self.skippable.clone(),
            escapable: self.escapable.clone(),
            segment_count: self.segment_count,
            playable_segment_count: self.playable_segment_count,
            availability: self.availability.clone(),
        }
    }
}

impl MediaOverlayTimeline {
    #[cfg(test)]
    pub(crate) fn to_result(&self) -> MediaOverlayTimelineResult {
        match &self.availability {
            MediaOverlayTimelineAvailability::Available {} => {
                MediaOverlayTimelineResult::Available {
                    timeline: self.clone(),
                }
            }
            MediaOverlayTimelineAvailability::Unavailable { reason } => {
                MediaOverlayTimelineResult::Unavailable {
                    summary: MediaOverlayTimelineSummary {
                        overlay_index: self.overlay_index,
                        canonical_path: self.canonical_path.clone(),
                        declarations: self.declarations.clone(),
                        skippable: self.skippable.clone(),
                        escapable: self.escapable.clone(),
                        segment_count: self.segment_count,
                        playable_segment_count: self.playable_segment_count,
                        availability: self.availability.clone(),
                    },
                    reason: *reason,
                }
            }
        }
    }
}

/// Checks a stored capability still agrees with its description, without needing any timeline.
pub(crate) fn capability_matches_playback(
    capability: &MediaOverlayCapability,
    playback: &MediaOverlayPlayback,
) -> bool {
    match (capability, playback) {
        (MediaOverlayCapability::Absent {}, MediaOverlayPlayback::Absent {})
        | (MediaOverlayCapability::Unavailable {}, MediaOverlayPlayback::Unavailable { .. }) => {
            true
        }
        (
            MediaOverlayCapability::Available {
                playable_occurrence_indexes,
                unavailable_occurrence_count,
            },
            MediaOverlayPlayback::Available { associations, .. },
        ) => {
            playable_occurrence_indexes.len() + *unavailable_occurrence_count as usize
                == associations.len()
                && playable_occurrence_indexes
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && playable_occurrence_indexes
                    .iter()
                    .all(|index| (*index as usize) < associations.len())
        }
        _ => false,
    }
}

pub(crate) fn derive_media_overlay_capability(
    description: &MediaOverlayDescription,
    timelines: &[MediaOverlayTimeline],
) -> MediaOverlayCapability {
    let associations = match &description.playback {
        MediaOverlayPlayback::Absent {} => return MediaOverlayCapability::Absent {},
        MediaOverlayPlayback::Unavailable { .. } => return MediaOverlayCapability::Unavailable {},
        MediaOverlayPlayback::Available { associations, .. } => associations,
    };
    let playable_occurrence_indexes = associations
        .iter()
        .filter(|association| {
            if !matches!(
                association.content,
                ReadingOrderContentTarget::Resolved {
                    bytes: MediaOverlayLocalBytes::Present {},
                    ..
                }
            ) {
                return false;
            }
            let OverlayAssociationState::Resolved { overlay_index, .. } = association.overlay
            else {
                return false;
            };
            let Some(timeline) = timelines.get(overlay_index as usize).filter(|timeline| {
                matches!(
                    timeline.availability,
                    MediaOverlayTimelineAvailability::Available {}
                )
            }) else {
                return false;
            };
            association.entry_segment_indexes.iter().any(|index| {
                timeline
                    .segments
                    .get(*index as usize)
                    .is_some_and(|segment| {
                        matches!(
                            segment.playability,
                            MediaOverlaySegmentPlayability::Playable {}
                        )
                    })
            })
        })
        .map(|association| association.occurrence_index)
        .collect::<Vec<_>>();
    let unavailable_occurrence_count =
        (associations.len() - playable_occurrence_indexes.len()) as u32;
    MediaOverlayCapability::Available {
        playable_occurrence_indexes,
        unavailable_occurrence_count,
    }
}

/// Returns the bounded description for a publication with no media-overlay relationship at all.
#[cfg(test)]
pub(crate) fn absent_media_overlay_description() -> MediaOverlayDescription {
    MediaOverlayDescription {
        package: MediaOverlayPackageFacts::Available {
            declared: false,
            declarations: Vec::new(),
            metadata: MediaOverlayPackageMetadata {
                total_durations: Vec::new(),
                item_durations: Vec::new(),
                narrators: Vec::new(),
                active_classes: Vec::new(),
                playback_active_classes: Vec::new(),
            },
        },
        coverage: MediaOverlayCoverage {
            state: MediaOverlayCoverageState::Inapplicable,
            package_relationships: None,
            smil_extraction: Vec::new(),
            smil_relationships: Vec::new(),
            fragment_observations: Vec::new(),
            media_inspection: Vec::new(),
        },
        playback: MediaOverlayPlayback::Absent {},
    }
}

/// Narration products that cannot be represented or trusted degrade to this. Reading availability
/// never depends on Media Overlay products, so every load-time problem resolves here instead of
/// failing the publication.
pub(crate) fn representation_limited_description() -> MediaOverlayDescription {
    MediaOverlayDescription {
        package: MediaOverlayPackageFacts::Unavailable {
            reason: MediaOverlayPackageIssue::RepresentationLimitExceeded,
        },
        coverage: MediaOverlayCoverage {
            state: MediaOverlayCoverageState::Unavailable,
            package_relationships: Some(MediaOverlayCoverageResult::Unavailable {
                issue: MediaOverlayAnalysisIssue::ResourceLimit,
            }),
            smil_extraction: Vec::new(),
            smil_relationships: Vec::new(),
            fragment_observations: Vec::new(),
            media_inspection: Vec::new(),
        },
        playback: MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
        },
    }
}

pub(crate) struct AvailablePackageFields<'a> {
    pub(crate) declared: bool,
    pub(crate) declarations: &'a [MediaOverlayDeclaration],
    pub(crate) metadata: &'a MediaOverlayPackageMetadata,
}
