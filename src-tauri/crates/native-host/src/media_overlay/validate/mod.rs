//! Strict validation of stored media-overlay descriptions and timelines.
//!
//! A stored description is untrusted input on reopen: every field, path, clock,
//! index, and cross-reference is rechecked before it can influence playback.
//! Unusable overlay data degrades the narration capability rather than failing
//! the publication load.

mod package;
mod primitive;
mod smil;
mod timeline;

#[cfg(test)]
use crate::media_overlay::MediaOverlayTimelineResult;
use crate::media_overlay::{
    MAX_MEDIA_OVERLAY_ASSOCIATIONS, MAX_MEDIA_OVERLAY_SEGMENTS, MAX_MEDIA_OVERLAY_TIMELINES,
    MediaOverlayAvailabilityIssue, MediaOverlayCoverageState, MediaOverlayDescription,
    MediaOverlayPackageFacts, MediaOverlayPackageIssue, MediaOverlayPlayback,
    MediaOverlaySegmentPlayability, MediaOverlayTimeline, MediaOverlayTimelineAvailability,
    MediaOverlayTimelineIssue, MediaOverlayTimelineSummary,
};
#[cfg(test)]
use crate::media_overlay::{
    MediaOverlayCapability, MediaOverlayLocalBytes, OverlayAssociationState,
    ReadingOrderContentTarget,
};
use serde::Serialize;

pub(crate) use package::{coverage_contains_path, validate_coverage, validate_package};
pub(crate) use primitive::invalid;
pub(crate) use smil::validate_segment;
pub(crate) use timeline::{
    local_reference_path, timeline_matches_summary, validate_association,
    validate_association_entry_paths, validate_segment_coverage, validate_timeline_fields,
    validate_timeline_summary,
};

// Checks the module's own tests exercise directly, rather than through an entry point.
#[cfg(test)]
pub(crate) use package::{available_package_fields, validate_metadata};
#[cfg(test)]
pub(crate) use primitive::{
    validate_clock, validate_serialized_size, validate_sorted_unique_indexes,
};
#[cfg(test)]
pub(crate) use smil::{validate_binding, validate_declaration, validate_reference};

/// Mirrors [`MediaOverlayTimelineResult`] on the wire without owning the timeline, so serving a
/// timeline does not clone every segment first.
#[derive(Serialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum MediaOverlayTimelineResultRef<'a> {
    Available {
        timeline: &'a MediaOverlayTimeline,
    },
    Unavailable {
        summary: MediaOverlayTimelineSummary,
        reason: MediaOverlayTimelineIssue,
    },
}

pub(crate) fn validate_media_overlay_description(
    description: &MediaOverlayDescription,
) -> Result<(), String> {
    validate_package(&description.package)?;
    validate_coverage(&description.coverage)?;

    match (&description.package, &description.playback) {
        (
            MediaOverlayPackageFacts::Available {
                declared,
                declarations,
                ..
            },
            MediaOverlayPlayback::Absent {},
        ) => {
            if *declared
                || !declarations.is_empty()
                || description.coverage.state != MediaOverlayCoverageState::Inapplicable
            {
                return invalid("absent playback conflicts with package or coverage applicability");
            }
        }
        (
            MediaOverlayPackageFacts::Available { declared, .. },
            MediaOverlayPlayback::Unavailable { .. },
        ) => {
            if !declared || description.coverage.state == MediaOverlayCoverageState::Inapplicable {
                return invalid("unavailable playback requires applicable declared media overlays");
            }
        }
        (
            MediaOverlayPackageFacts::Available { declared, .. },
            MediaOverlayPlayback::Available {
                overlays,
                associations,
            },
        ) => {
            if !declared
                || description.coverage.state == MediaOverlayCoverageState::Inapplicable
                || overlays.is_empty()
            {
                return invalid("available playback requires declared timelines");
            }
            if overlays.len() > MAX_MEDIA_OVERLAY_TIMELINES
                || associations.len() > MAX_MEDIA_OVERLAY_ASSOCIATIONS
            {
                return invalid("media overlay description exceeds timeline or association bounds");
            }
            let mut playable_timeline_count = 0u32;
            for (index, summary) in overlays.iter().enumerate() {
                if summary.overlay_index as usize != index {
                    return invalid("overlay indexes are not contiguous");
                }
                validate_timeline_summary(summary, &description.package)?;
                if matches!(
                    summary.availability,
                    MediaOverlayTimelineAvailability::Available {}
                ) {
                    playable_timeline_count += 1;
                }
            }
            if playable_timeline_count == 0 {
                return invalid("available playback has no available playable timeline");
            }
            for (index, association) in associations.iter().enumerate() {
                if association.occurrence_index as usize != index {
                    return invalid("association occurrence indexes are not contiguous");
                }
                validate_association(
                    association,
                    overlays,
                    &description.package,
                    &description.coverage,
                )?;
            }
            for summary in overlays {
                if !coverage_contains_path(
                    &description.coverage.smil_extraction,
                    &summary.canonical_path,
                ) || !coverage_contains_path(
                    &description.coverage.smil_relationships,
                    &summary.canonical_path,
                ) {
                    return invalid("timeline summary lacks SMIL analysis coverage");
                }
            }
        }
        (
            MediaOverlayPackageFacts::Unavailable {
                reason: MediaOverlayPackageIssue::RepresentationLimitExceeded,
            },
            MediaOverlayPlayback::Unavailable {
                reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
            },
        ) if description.coverage.state != MediaOverlayCoverageState::Inapplicable => {}
        (MediaOverlayPackageFacts::Unavailable { .. }, _) => {
            return invalid("unavailable package facts conflict with playback or coverage");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn validate_media_overlay_timeline_result(
    result: &MediaOverlayTimelineResult,
    description: &MediaOverlayDescription,
) -> Result<(), String> {
    validate_media_overlay_description(description)?;
    validate_timeline_result_for_description(result, description)
}

/// Checks a derived timeline against the description derived beside it.
///
/// This asserts nothing about the publication. An overlay whose audio never
/// backed is a normal outcome and stays committed as an unavailable timeline;
/// what is refused is a timeline that contradicts the summary this host derived
/// alongside it, because serving that is a worse failure than not committing it.
pub(crate) fn validate_timeline_for_description(
    timeline: &MediaOverlayTimeline,
    description: &MediaOverlayDescription,
) -> Result<(), String> {
    // A description that is not available has already degraded the capability for
    // this publication, so there is no summary to agree with and nothing to refuse.
    let MediaOverlayPlayback::Available { overlays, .. } = &description.playback else {
        return Ok(());
    };
    validate_timeline_fields(timeline, &description.package)?;
    let expected = overlays
        .get(timeline.overlay_index as usize)
        .ok_or_else(|| "timeline has an unknown overlay index".to_owned())?;
    if !timeline_matches_summary(timeline, expected) {
        return invalid("timeline does not match its description summary");
    }
    let MediaOverlayTimelineAvailability::Available {} = timeline.availability else {
        return Ok(());
    };
    if timeline.segments.len() != timeline.segment_count as usize
        || timeline.segments.len() > MAX_MEDIA_OVERLAY_SEGMENTS
    {
        return invalid("timeline segment count is inconsistent or out of bounds");
    }
    let mut playable = 0u32;
    for (index, segment) in timeline.segments.iter().enumerate() {
        if segment.segment_index as usize != index {
            return invalid("segment indexes are not contiguous");
        }
        validate_segment(segment)?;
        if matches!(
            segment.playability,
            MediaOverlaySegmentPlayability::Playable {}
        ) {
            playable += 1;
        }
    }
    if playable != timeline.playable_segment_count || playable == 0 {
        return invalid("timeline playable segment count is inconsistent");
    }
    validate_segment_coverage(timeline, &description.coverage)?;
    validate_association_entry_paths(timeline, description)
}

#[cfg(test)]
pub(crate) fn validate_timeline_result_for_description(
    result: &MediaOverlayTimelineResult,
    description: &MediaOverlayDescription,
) -> Result<(), String> {
    let overlays = match &description.playback {
        MediaOverlayPlayback::Available { overlays, .. } => overlays,
        _ => return invalid("timeline result requires an available description"),
    };

    match result {
        MediaOverlayTimelineResult::Available { timeline } => {
            validate_timeline_for_description(timeline, description)?;
        }
        MediaOverlayTimelineResult::Unavailable { summary, reason } => {
            validate_timeline_summary(summary, &description.package)?;
            let expected = overlays
                .get(summary.overlay_index as usize)
                .ok_or_else(|| "timeline result has an unknown overlay index".to_owned())?;
            if expected != summary
                || !matches!(
                    &summary.availability,
                    MediaOverlayTimelineAvailability::Unavailable { reason: value } if value == reason
                )
            {
                return invalid("unavailable timeline does not match its description summary");
            }
            if summary.playable_segment_count != 0
                && *reason == MediaOverlayTimelineIssue::NoPlayableSegments
            {
                return invalid("no-playable-segments conflicts with the timeline count");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn validate_media_overlay_capability(
    capability: &MediaOverlayCapability,
    description: &MediaOverlayDescription,
    timelines: &[MediaOverlayTimelineResult],
) -> Result<(), String> {
    validate_media_overlay_description(description)?;
    match (capability, &description.playback) {
        (MediaOverlayCapability::Absent {}, MediaOverlayPlayback::Absent {})
        | (MediaOverlayCapability::Unavailable {}, MediaOverlayPlayback::Unavailable { .. })
            if timelines.is_empty() =>
        {
            Ok(())
        }
        (
            MediaOverlayCapability::Available {
                playable_occurrence_indexes,
                unavailable_occurrence_count,
            },
            MediaOverlayPlayback::Available {
                overlays,
                associations,
            },
        ) => {
            if timelines.len() != overlays.len() {
                return invalid("capability does not have one result per timeline summary");
            }
            for (index, result) in timelines.iter().enumerate() {
                validate_timeline_result_for_description(result, description)?;
                if timeline_result_index(result) as usize != index {
                    return invalid("capability timeline results are not in description order");
                }
            }
            validate_sorted_unique_indexes(playable_occurrence_indexes, associations.len(), false)?;
            let expected = associations
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
                    let OverlayAssociationState::Resolved { overlay_index, .. } =
                        association.overlay
                    else {
                        return false;
                    };
                    let Some(MediaOverlayTimelineResult::Available { timeline }) =
                        timelines.get(overlay_index as usize)
                    else {
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
            if *playable_occurrence_indexes != expected {
                return invalid("capability playable occurrences do not match associations");
            }
            let unavailable = associations.len() - expected.len();
            if *unavailable_occurrence_count as usize != unavailable {
                return invalid("capability unavailable occurrence count is inconsistent");
            }
            Ok(())
        }
        _ => invalid("media overlay capability status conflicts with playback"),
    }
}

#[cfg(test)]
pub(crate) fn timeline_result_index(result: &MediaOverlayTimelineResult) -> u32 {
    match result {
        MediaOverlayTimelineResult::Available { timeline } => timeline.overlay_index,
        MediaOverlayTimelineResult::Unavailable { summary, .. } => summary.overlay_index,
    }
}
