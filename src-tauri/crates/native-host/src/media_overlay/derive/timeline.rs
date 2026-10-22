//! Building one overlay's timeline: its segments, their playability, and the
//! coverage each one contributes.

use super::{
    CoverageBuilder, project_analysis_issue, project_audio_binding, project_declaration,
    project_reference, project_smil_time, project_text_binding,
};
use crate::media_overlay::validate::local_reference_path;
use crate::media_overlay::{
    MAX_MEDIA_OVERLAY_SEGMENTS, MediaOverlayBinding, MediaOverlayBindingAvailability,
    MediaOverlayBindingIssue, MediaOverlayCoverageResult, MediaOverlayDeclaration,
    MediaOverlayMalformedReason, MediaOverlayReference, MediaOverlaySegment,
    MediaOverlaySegmentPlayability, MediaOverlaySegmentUnavailableReason, MediaOverlaySemanticNode,
    MediaOverlaySemanticNodeKind, MediaOverlayTimeline, MediaOverlayTimelineAvailability,
    MediaOverlayTimelineIssue, MediaOverlayUnsupportedReason, SmilClock,
};
use epub_navigator::kernel::DocumentAdapter;
use epub_stack::PublicationAnalysis;
use epub_stack::analysis::AnalysisOutcome;
use epub_stack::media_overlay::{MediaOverlayAssociationRef, SmilNodeFact, SmilNodeRef};
use std::collections::HashMap;

pub(crate) fn build_timeline(
    analysis: &PublicationAnalysis,
    adapters: &HashMap<&str, DocumentAdapter>,
    association: &MediaOverlayAssociationRef<'_>,
    overlay_index: u32,
    declaration_index_by_ordinal: &HashMap<u32, usize>,
    package_declarations: &[MediaOverlayDeclaration],
    coverage: &mut CoverageBuilder,
) -> MediaOverlayTimeline {
    let overlay_analysis = association
        .overlay_resource()
        .expect("timeline construction requires a resolved overlay resource");
    let overlay_resource = overlay_analysis.resource();
    let canonical_path = overlay_resource
        .local_path()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_default();
    let declarations = overlay_resource
        .declarations()
        .map(|declaration| {
            declaration_index_by_ordinal
                .get(&declaration.ordinal().as_u32())
                .and_then(|index| package_declarations.get(*index))
                .cloned()
                .unwrap_or_else(|| project_declaration(declaration, declaration.ordinal().as_u32()))
        })
        .collect::<Vec<_>>();

    {
        match overlay_analysis.content() {
            AnalysisOutcome::Complete(_) => {
                coverage.note_smil(&canonical_path, MediaOverlayCoverageResult::Complete {});
            }
            AnalysisOutcome::Partial { issue, .. } => {
                coverage.note_smil(
                    &canonical_path,
                    MediaOverlayCoverageResult::Partial {
                        issue: project_analysis_issue(*issue),
                    },
                );
            }
            AnalysisOutcome::Unavailable(issue) => {
                coverage.note_smil(
                    &canonical_path,
                    MediaOverlayCoverageResult::Unavailable {
                        issue: project_analysis_issue(*issue),
                    },
                );
            }
            AnalysisOutcome::NotApplicable => {}
        }
    }

    let Some(smil_facts) = association.smil_facts() else {
        return unavailable_timeline(
            overlay_index,
            canonical_path,
            declarations,
            MediaOverlayTimelineIssue::AnalysisUnavailable,
        );
    };

    let skippable = smil_facts.skippable().to_vec();
    let escapable = smil_facts.escapable().to_vec();

    let mut segments = Vec::new();
    let mut path = Vec::new();
    let mut segments_overflowed = false;
    for root in association.roots() {
        walk_node(
            analysis,
            adapters,
            root,
            &mut path,
            &mut segments,
            coverage,
            &mut segments_overflowed,
        );
    }

    if segments_overflowed {
        return unavailable_timeline(
            overlay_index,
            canonical_path,
            declarations,
            MediaOverlayTimelineIssue::RepresentationLimitExceeded,
        );
    }

    let playable_segment_count = segments
        .iter()
        .filter(|segment: &&MediaOverlaySegment| {
            matches!(
                segment.playability,
                MediaOverlaySegmentPlayability::Playable {}
            )
        })
        .count() as u32;

    let availability = if playable_segment_count == 0 {
        MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::NoPlayableSegments,
        }
    } else {
        MediaOverlayTimelineAvailability::Available {}
    };

    MediaOverlayTimeline {
        overlay_index,
        canonical_path,
        declarations,
        skippable,
        escapable,
        segment_count: segments.len() as u32,
        playable_segment_count,
        availability,
        segments,
    }
}

pub(crate) fn unavailable_timeline(
    overlay_index: u32,
    canonical_path: String,
    declarations: Vec<MediaOverlayDeclaration>,
    reason: MediaOverlayTimelineIssue,
) -> MediaOverlayTimeline {
    MediaOverlayTimeline {
        overlay_index,
        canonical_path,
        declarations,
        skippable: Vec::new(),
        escapable: Vec::new(),
        segment_count: 0,
        playable_segment_count: 0,
        availability: MediaOverlayTimelineAvailability::Unavailable { reason },
        segments: Vec::new(),
    }
}

pub(crate) fn walk_node(
    analysis: &PublicationAnalysis,
    adapters: &HashMap<&str, DocumentAdapter>,
    node: SmilNodeRef<'_>,
    path: &mut Vec<MediaOverlaySemanticNode>,
    segments: &mut Vec<MediaOverlaySegment>,
    coverage: &mut CoverageBuilder,
    overflowed: &mut bool,
) {
    match node.fact() {
        SmilNodeFact::Sequence { epub_types, .. } => {
            path.push(MediaOverlaySemanticNode {
                kind: MediaOverlaySemanticNodeKind::Sequence,
                epub_types: epub_types.clone(),
                text_reference: node
                    .text_reference()
                    .map(|reference| project_reference(analysis, Some(reference))),
            });
            for child in node.children() {
                walk_node(
                    analysis, adapters, child, path, segments, coverage, overflowed,
                );
            }
            path.pop();
        }
        SmilNodeFact::Parallel { epub_types, .. } => {
            let segment_index = segments.len() as u32;
            if segment_index as usize >= MAX_MEDIA_OVERLAY_SEGMENTS {
                *overflowed = true;
                return;
            }
            let mut text_child = None;
            let mut audio_child = None;
            let mut extra_text = false;
            let mut extra_audio = false;
            let mut invalid_children = false;
            for child in node.children() {
                match child.fact() {
                    SmilNodeFact::Text { .. } => {
                        if text_child.is_some() {
                            extra_text = true;
                        } else {
                            text_child = Some(child);
                        }
                    }
                    SmilNodeFact::Audio { .. } => {
                        if audio_child.is_some() {
                            extra_audio = true;
                        } else {
                            audio_child = Some(child);
                        }
                    }
                    SmilNodeFact::Sequence { .. } | SmilNodeFact::Parallel { .. } => {
                        invalid_children = true;
                    }
                }
            }

            let text_reference = text_child.and_then(SmilNodeRef::text_reference);
            let mut semantic_path = path.clone();
            semantic_path.push(MediaOverlaySemanticNode {
                kind: MediaOverlaySemanticNodeKind::Parallel,
                epub_types: epub_types.clone(),
                text_reference: text_reference
                    .map(|reference| project_reference(analysis, Some(reference))),
            });

            let text_binding = project_text_binding(analysis, adapters, text_child);
            let audio_binding = project_audio_binding(analysis, audio_child);
            let (clip_begin, clip_end) = match audio_child {
                Some(child) => (
                    project_smil_time(child.fact().clip_begin()),
                    project_smil_time(child.fact().clip_end()),
                ),
                None => (SmilClock::Absent {}, SmilClock::Absent {}),
            };

            note_binding_coverage(coverage, &text_binding, &audio_binding);

            let playability = classify_segment(
                invalid_children,
                text_child.is_some(),
                extra_text,
                audio_child.is_some(),
                extra_audio,
                &text_binding,
                &audio_binding,
                &clip_begin,
                &clip_end,
                adapters,
            );

            segments.push(MediaOverlaySegment {
                segment_index,
                semantic_path,
                text: text_binding,
                audio: audio_binding,
                clip_begin,
                clip_end,
                playability,
            });
        }
        SmilNodeFact::Text { .. } | SmilNodeFact::Audio { .. } => {
            // A bare text/audio node outside a parallel is not a playable synchronization unit.
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_segment(
    invalid_children: bool,
    has_text: bool,
    extra_text: bool,
    has_audio: bool,
    extra_audio: bool,
    text_binding: &MediaOverlayBinding,
    audio_binding: &MediaOverlayBinding,
    clip_begin: &SmilClock,
    clip_end: &SmilClock,
    adapters: &HashMap<&str, DocumentAdapter>,
) -> MediaOverlaySegmentPlayability {
    if invalid_children {
        return MediaOverlaySegmentPlayability::Malformed {
            reason: MediaOverlayMalformedReason::InvalidParallelChildren,
        };
    }
    if !has_text {
        return MediaOverlaySegmentPlayability::Malformed {
            reason: MediaOverlayMalformedReason::MissingText,
        };
    }
    if extra_text {
        return MediaOverlaySegmentPlayability::Malformed {
            reason: MediaOverlayMalformedReason::MultipleText,
        };
    }
    if extra_audio {
        return MediaOverlaySegmentPlayability::Malformed {
            reason: MediaOverlayMalformedReason::MultipleAudio,
        };
    }

    // Text-side classification takes priority: an unsupported target is a property of the
    // text document regardless of whether audio is present.
    if let MediaOverlayBindingAvailability::Unavailable { reason } = text_binding.availability {
        match reason {
            MediaOverlayBindingIssue::UnsupportedTarget => {
                if let Some(reason) = unsupported_text_reason(adapters, &text_binding.reference) {
                    return MediaOverlaySegmentPlayability::Unsupported { reason };
                }
                return MediaOverlaySegmentPlayability::Unavailable {
                    reason: MediaOverlaySegmentUnavailableReason::TextBindingUnavailable,
                };
            }
            _ => {
                return MediaOverlaySegmentPlayability::Unavailable {
                    reason: MediaOverlaySegmentUnavailableReason::TextBindingUnavailable,
                };
            }
        }
    }

    if !has_audio {
        return MediaOverlaySegmentPlayability::Unsupported {
            reason: MediaOverlayUnsupportedReason::TextToSpeechUnavailable,
        };
    }

    if let MediaOverlayBindingAvailability::Unavailable { reason } = audio_binding.availability {
        return match reason {
            MediaOverlayBindingIssue::UnsupportedMediaType => {
                MediaOverlaySegmentPlayability::Unsupported {
                    reason: MediaOverlayUnsupportedReason::UnsupportedAudioFormat,
                }
            }
            _ => MediaOverlaySegmentPlayability::Unavailable {
                reason: MediaOverlaySegmentUnavailableReason::AudioBindingUnavailable,
            },
        };
    }

    if matches!(clip_begin, SmilClock::Unrecognized {})
        || matches!(clip_end, SmilClock::Unrecognized {})
    {
        return MediaOverlaySegmentPlayability::Malformed {
            reason: MediaOverlayMalformedReason::UnrecognizedClock,
        };
    }
    // Both clocks are optional: an absent beginning is the start of the audio resource and an
    // absent end is its intrinsic end, which only playback knows.
    let begin = match clip_begin {
        SmilClock::Absent {} => Some(0),
        SmilClock::Parsed { milliseconds } => milliseconds.parse::<u64>().ok(),
        SmilClock::Unrecognized {} => unreachable!("unrecognized clocks are rejected above"),
    };
    let end = match clip_end {
        SmilClock::Absent {} => None,
        SmilClock::Parsed { milliseconds } => match milliseconds.parse::<u64>() {
            Ok(end) => Some(end),
            Err(_) => {
                return MediaOverlaySegmentPlayability::Unavailable {
                    reason: MediaOverlaySegmentUnavailableReason::NumericOverflow,
                };
            }
        },
        SmilClock::Unrecognized {} => unreachable!("unrecognized clocks are rejected above"),
    };
    let Some(begin) = begin else {
        return MediaOverlaySegmentPlayability::Unavailable {
            reason: MediaOverlaySegmentUnavailableReason::NumericOverflow,
        };
    };
    if let Some(end) = end {
        if begin == end {
            return MediaOverlaySegmentPlayability::Malformed {
                reason: MediaOverlayMalformedReason::EmptyClip,
            };
        }
        if begin > end {
            return MediaOverlaySegmentPlayability::Malformed {
                reason: MediaOverlayMalformedReason::ReversedClip,
            };
        }
    }
    MediaOverlaySegmentPlayability::Playable {}
}

pub(crate) fn unsupported_text_reason(
    adapters: &HashMap<&str, DocumentAdapter>,
    reference: &MediaOverlayReference,
) -> Option<MediaOverlayUnsupportedReason> {
    match reference {
        MediaOverlayReference::LocalResource { .. } => {
            Some(MediaOverlayUnsupportedReason::UnsupportedTextFragment)
        }
        MediaOverlayReference::LocalFragment { canonical_path, .. } => {
            match adapters.get(canonical_path.as_str()) {
                Some(DocumentAdapter::FixedImage) => {
                    Some(MediaOverlayUnsupportedReason::FixedLayoutText)
                }
                Some(
                    DocumentAdapter::ReflowableXhtml
                    | DocumentAdapter::FixedXhtml
                    | DocumentAdapter::FixedSvg,
                )
                | None => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn note_binding_coverage(
    coverage: &mut CoverageBuilder,
    text: &MediaOverlayBinding,
    audio: &MediaOverlayBinding,
) {
    if let Some(path) = local_reference_path(&text.reference)
        && matches!(
            text.reference,
            MediaOverlayReference::LocalFragment { .. }
                | MediaOverlayReference::IndexedLocalFragmentUnavailable { .. }
        )
    {
        coverage.note_fragment(path, MediaOverlayCoverageResult::Complete {});
    }
    if let Some(path) = local_reference_path(&audio.reference) {
        coverage.note_media(path, MediaOverlayCoverageResult::Complete {});
    }
}
