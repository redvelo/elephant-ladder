//! Derived timelines and the reading-order occurrences they play against.

use super::package::{available_package_fields, coverage_contains_path};
use super::primitive::{
    invalid, validate_declaration_index, validate_path, validate_sorted_unique_indexes,
    validate_string,
};
use super::smil::{validate_declaration_candidates, validate_registry_declaration};
use crate::media_overlay::MediaOverlayReference;
use crate::media_overlay::{
    MAX_MEDIA_OVERLAY_COLLECTION_ITEMS, MAX_MEDIA_OVERLAY_DECLARATIONS, MAX_MEDIA_OVERLAY_SEGMENTS,
    MediaOverlayAssociation, MediaOverlayCoverage, MediaOverlayCoverageResult,
    MediaOverlayDescription, MediaOverlayPackageFacts, MediaOverlayPlayback, MediaOverlayTimeline,
    MediaOverlayTimelineAvailability, MediaOverlayTimelineIssue, MediaOverlayTimelineSummary,
    OverlayAssociationState, ReadingOrderContentTarget,
};

pub(crate) fn validate_timeline_summary(
    summary: &MediaOverlayTimelineSummary,
    package: &MediaOverlayPackageFacts,
) -> Result<(), String> {
    let package = available_package_fields(package)
        .ok_or_else(|| "timeline requires available package facts".to_owned())?;
    validate_path(&summary.canonical_path)?;
    let over_limit = summary.segment_count as usize > MAX_MEDIA_OVERLAY_SEGMENTS;
    if (over_limit
        && !matches!(
            summary.availability,
            MediaOverlayTimelineAvailability::Unavailable {
                reason: MediaOverlayTimelineIssue::RepresentationLimitExceeded
            }
        ))
        || summary.playable_segment_count > summary.segment_count
        || summary.declarations.len() > MAX_MEDIA_OVERLAY_DECLARATIONS
        || summary.skippable.len() > MAX_MEDIA_OVERLAY_COLLECTION_ITEMS
        || summary.escapable.len() > MAX_MEDIA_OVERLAY_COLLECTION_ITEMS
    {
        return invalid("media overlay timeline summary has invalid counts or bounds");
    }
    let mut previous = None;
    for declaration in &summary.declarations {
        validate_declaration_index(declaration.declaration_index, package.declarations.len())?;
        if previous.is_some_and(|index| index >= declaration.declaration_index)
            || package.declarations[declaration.declaration_index as usize] != *declaration
            || declaration.canonical_path.as_deref() != Some(summary.canonical_path.as_str())
        {
            return invalid("timeline declarations are unsorted or do not match package facts");
        }
        previous = Some(declaration.declaration_index);
    }
    for value in summary.skippable.iter().chain(&summary.escapable) {
        validate_string(value, false)?;
    }
    match &summary.availability {
        MediaOverlayTimelineAvailability::Available {} if summary.playable_segment_count == 0 => {
            invalid("available timeline has no playable segments")
        }
        MediaOverlayTimelineAvailability::Unavailable { reason }
            if *reason == MediaOverlayTimelineIssue::NoPlayableSegments
                && summary.playable_segment_count != 0 =>
        {
            invalid("no-playable-segments conflicts with the timeline count")
        }
        MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::AnalysisUnavailable,
        } if summary.segment_count != 0
            || summary.playable_segment_count != 0
            || !summary.skippable.is_empty()
            || !summary.escapable.is_empty() =>
        {
            invalid("analysis-unavailable timeline retains extracted SMIL facts")
        }
        _ => Ok(()),
    }
}

pub(crate) fn validate_timeline_fields(
    timeline: &MediaOverlayTimeline,
    package: &MediaOverlayPackageFacts,
) -> Result<(), String> {
    validate_timeline_summary(
        &MediaOverlayTimelineSummary {
            overlay_index: timeline.overlay_index,
            canonical_path: timeline.canonical_path.clone(),
            declarations: timeline.declarations.clone(),
            skippable: timeline.skippable.clone(),
            escapable: timeline.escapable.clone(),
            segment_count: timeline.segment_count,
            playable_segment_count: timeline.playable_segment_count,
            availability: timeline.availability.clone(),
        },
        package,
    )
}

pub(crate) fn timeline_matches_summary(
    timeline: &MediaOverlayTimeline,
    summary: &MediaOverlayTimelineSummary,
) -> bool {
    timeline.overlay_index == summary.overlay_index
        && timeline.canonical_path == summary.canonical_path
        && timeline.declarations == summary.declarations
        && timeline.skippable == summary.skippable
        && timeline.escapable == summary.escapable
        && timeline.segment_count == summary.segment_count
        && timeline.playable_segment_count == summary.playable_segment_count
        && timeline.availability == summary.availability
}

pub(crate) fn validate_segment_coverage(
    timeline: &MediaOverlayTimeline,
    coverage: &MediaOverlayCoverage,
) -> Result<(), String> {
    for segment in &timeline.segments {
        if matches!(
            segment.text.reference,
            MediaOverlayReference::LocalFragment { .. }
                | MediaOverlayReference::IndexedLocalFragmentUnavailable { .. }
        ) {
            let path = local_reference_path(&segment.text.reference)
                .expect("local fragment references always have canonical paths");
            if !coverage_contains_path(&coverage.fragment_observations, path) {
                return invalid("segment text fragment lacks observation coverage");
            }
        }
        if matches!(
            segment.audio.reference,
            MediaOverlayReference::LocalResource { .. }
                | MediaOverlayReference::LocalFragment { .. }
                | MediaOverlayReference::IndexedLocalResourceUnavailable { .. }
                | MediaOverlayReference::IndexedLocalFragmentUnavailable { .. }
        ) {
            let path = local_reference_path(&segment.audio.reference)
                .expect("local audio references always have canonical paths");
            if !coverage_contains_path(&coverage.media_inspection, path) {
                return invalid("segment audio lacks media inspection coverage");
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_association_entry_paths(
    timeline: &MediaOverlayTimeline,
    description: &MediaOverlayDescription,
) -> Result<(), String> {
    let MediaOverlayPlayback::Available { associations, .. } = &description.playback else {
        return invalid("timeline associations require an available description");
    };
    for association in associations {
        let OverlayAssociationState::Resolved { overlay_index, .. } = association.overlay else {
            continue;
        };
        if overlay_index != timeline.overlay_index {
            continue;
        }
        let ReadingOrderContentTarget::Resolved { canonical_path, .. } = &association.content
        else {
            if association.entry_segment_indexes.is_empty() {
                continue;
            }
            return invalid("entry segments require resolved reading-order content");
        };
        let expected = timeline
            .segments
            .iter()
            .filter(|segment| {
                local_reference_path(&segment.text.reference) == Some(canonical_path.as_str())
            })
            .map(|segment| segment.segment_index)
            .collect::<Vec<_>>();
        if association.entry_segment_indexes != expected {
            return invalid("association entry segments are not the complete target set");
        }
    }
    Ok(())
}

pub(crate) fn local_reference_path(reference: &MediaOverlayReference) -> Option<&str> {
    match reference {
        MediaOverlayReference::LocalResource { canonical_path, .. }
        | MediaOverlayReference::LocalFragment { canonical_path, .. }
        | MediaOverlayReference::IndexedLocalResourceUnavailable { canonical_path, .. }
        | MediaOverlayReference::IndexedLocalFragmentUnavailable { canonical_path, .. } => {
            Some(canonical_path)
        }
        _ => None,
    }
}

pub(crate) fn validate_association(
    association: &MediaOverlayAssociation,
    overlays: &[MediaOverlayTimelineSummary],
    package: &MediaOverlayPackageFacts,
    coverage: &MediaOverlayCoverage,
) -> Result<(), String> {
    match &association.content {
        ReadingOrderContentTarget::MissingIdref {} => {}
        ReadingOrderContentTarget::MissingDeclaration { authored_idref } => {
            validate_string(authored_idref, true)?;
        }
        ReadingOrderContentTarget::AmbiguousDeclaration {
            authored_idref,
            candidates,
        } => {
            validate_string(authored_idref, true)?;
            validate_declaration_candidates(authored_idref, candidates, package)?;
        }
        ReadingOrderContentTarget::DeclarationWithoutResource { declaration } => {
            validate_registry_declaration(declaration, package)?;
            if declaration.canonical_path.is_some() {
                return invalid("resource-less content declaration has a canonical path");
            }
        }
        ReadingOrderContentTarget::Resolved {
            declaration,
            canonical_path,
            bytes: _,
        } => {
            validate_registry_declaration(declaration, package)?;
            validate_path(canonical_path)?;
            if declaration.canonical_path.as_deref() != Some(canonical_path) {
                return invalid("resolved content path conflicts with its declaration");
            }
        }
    }

    let overlay_index = match &association.overlay {
        OverlayAssociationState::NotDeclared {} => None,
        OverlayAssociationState::MissingDeclaration { authored_idref } => {
            validate_string(authored_idref, true)?;
            None
        }
        OverlayAssociationState::AmbiguousDeclaration {
            authored_idref,
            candidates,
        } => {
            validate_string(authored_idref, true)?;
            validate_declaration_candidates(authored_idref, candidates, package)?;
            None
        }
        OverlayAssociationState::DeclarationWithoutResource { declaration } => {
            validate_registry_declaration(declaration, package)?;
            if declaration.canonical_path.is_some() {
                return invalid("resource-less association points to a resolved declaration");
            }
            None
        }
        OverlayAssociationState::AnalysisUnavailable {
            declaration,
            overlay_index,
            issue,
        } => {
            if !association.entry_segment_indexes.is_empty() {
                return invalid("analysis-unavailable association has entry segments");
            }
            validate_registry_declaration(declaration, package)?;
            let summary = overlays
                .get(*overlay_index as usize)
                .ok_or_else(|| "association has an unknown overlay index".to_owned())?;
            if !summary
                .declarations
                .iter()
                .any(|candidate| candidate == declaration)
            {
                return invalid("association declaration does not belong to its overlay");
            }
            if !matches!(
                summary.availability,
                MediaOverlayTimelineAvailability::Unavailable {
                    reason: MediaOverlayTimelineIssue::AnalysisUnavailable
                }
            ) || !coverage.smil_extraction.iter().any(|entry| {
                entry.canonical_path == summary.canonical_path
                    && matches!(
                        entry.result,
                        MediaOverlayCoverageResult::Unavailable { issue: value }
                            if value == *issue
                    )
            }) {
                return invalid(
                    "analysis-unavailable association lacks matching analysis evidence",
                );
            }
            Some(*overlay_index)
        }
        OverlayAssociationState::Resolved {
            declaration,
            overlay_index,
        } => {
            validate_registry_declaration(declaration, package)?;
            let summary = overlays
                .get(*overlay_index as usize)
                .ok_or_else(|| "association has an unknown overlay index".to_owned())?;
            if !summary
                .declarations
                .iter()
                .any(|candidate| candidate == declaration)
            {
                return invalid("association declaration does not belong to its overlay");
            }
            if matches!(
                summary.availability,
                MediaOverlayTimelineAvailability::Unavailable {
                    reason: MediaOverlayTimelineIssue::AnalysisUnavailable
                }
            ) {
                return invalid("resolved association points to an analysis-unavailable timeline");
            }
            Some(*overlay_index)
        }
    };

    if let Some(index) = overlay_index {
        validate_sorted_unique_indexes(
            &association.entry_segment_indexes,
            overlays[index as usize].segment_count as usize,
            false,
        )?;
        if !association.entry_segment_indexes.is_empty()
            && !matches!(
                association.content,
                ReadingOrderContentTarget::Resolved { .. }
            )
        {
            return invalid("entry segment indexes require resolved content");
        }
    } else if !association.entry_segment_indexes.is_empty() {
        return invalid("unresolved association has entry segment indexes");
    }
    Ok(())
}
