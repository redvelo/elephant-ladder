//! What the package declares about narration, and how far the host got
//! looking: declarations, metadata, and the coverage of that inspection.

use super::primitive::{
    invalid, validate_clock, validate_optional_string, validate_path,
    validate_sorted_unique_indexes, validate_string,
};
use super::smil::validate_declaration;
use crate::media_overlay::{
    AvailablePackageFields, MAX_MEDIA_OVERLAY_COLLECTION_ITEMS, MAX_MEDIA_OVERLAY_COVERAGE_ENTRIES,
    MAX_MEDIA_OVERLAY_COVERAGE_ISSUES, MAX_MEDIA_OVERLAY_DECLARATIONS, MediaOverlayCoverage,
    MediaOverlayCoverageResult, MediaOverlayCoverageState, MediaOverlayPackageFacts,
    MediaOverlayPackageMetadata, MediaOverlayPathCoverage, SmilClock,
};
use std::collections::HashSet;

pub(crate) fn validate_package(package: &MediaOverlayPackageFacts) -> Result<(), String> {
    let Some(package) = available_package_fields(package) else {
        return Ok(());
    };
    if package.declarations.len() > MAX_MEDIA_OVERLAY_DECLARATIONS {
        return invalid("media overlay declarations exceed their bound");
    }
    if !package.declared && !package.declarations.is_empty() {
        return invalid("undeclared media overlays cannot contain declarations");
    }
    for (index, declaration) in package.declarations.iter().enumerate() {
        if declaration.declaration_index as usize != index {
            return invalid("media overlay declaration indexes are not contiguous");
        }
        validate_declaration(declaration)?;
    }
    validate_metadata(package.metadata, package.declarations.len())
}

pub(crate) fn available_package_fields(
    package: &MediaOverlayPackageFacts,
) -> Option<AvailablePackageFields<'_>> {
    match package {
        MediaOverlayPackageFacts::Available {
            declared,
            declarations,
            metadata,
        } => Some(AvailablePackageFields {
            declared: *declared,
            declarations,
            metadata,
        }),
        MediaOverlayPackageFacts::Unavailable { .. } => None,
    }
}

pub(crate) fn validate_metadata(
    metadata: &MediaOverlayPackageMetadata,
    declaration_count: usize,
) -> Result<(), String> {
    for length in [
        metadata.total_durations.len(),
        metadata.item_durations.len(),
        metadata.narrators.len(),
        metadata.active_classes.len(),
        metadata.playback_active_classes.len(),
    ] {
        if length > MAX_MEDIA_OVERLAY_COLLECTION_ITEMS {
            return invalid("media overlay metadata exceeds its collection bound");
        }
    }
    for duration in metadata.total_durations.iter() {
        validate_string(&duration.authored, true)?;
        validate_clock(&duration.clock)?;
        if duration.refines.is_some()
            || !duration.declaration_indexes.is_empty()
            || matches!(duration.clock, SmilClock::Absent {})
        {
            return invalid("total duration has refinement data or no authored clock");
        }
    }
    for duration in &metadata.item_durations {
        validate_string(&duration.authored, true)?;
        validate_clock(&duration.clock)?;
        if duration.refines.is_none() || matches!(duration.clock, SmilClock::Absent {}) {
            return invalid("item duration has no refinement or authored clock");
        }
        validate_optional_string(&duration.refines)?;
        validate_sorted_unique_indexes(&duration.declaration_indexes, declaration_count, false)?;
    }
    for value in metadata
        .narrators
        .iter()
        .chain(&metadata.active_classes)
        .chain(&metadata.playback_active_classes)
    {
        validate_string(value, true)?;
    }
    Ok(())
}

pub(crate) fn validate_coverage(coverage: &MediaOverlayCoverage) -> Result<(), String> {
    let families = [
        coverage.smil_extraction.as_slice(),
        coverage.smil_relationships.as_slice(),
        coverage.fragment_observations.as_slice(),
        coverage.media_inspection.as_slice(),
    ];
    let applicable_count = usize::from(coverage.package_relationships.is_some())
        + families.iter().map(|family| family.len()).sum::<usize>();
    if applicable_count > MAX_MEDIA_OVERLAY_COVERAGE_ENTRIES {
        return invalid("media overlay coverage exceeds its entry bound");
    }
    for family in families {
        let mut paths = HashSet::with_capacity(family.len());
        for entry in family {
            validate_path(&entry.canonical_path)?;
            if !paths.insert(entry.canonical_path.as_str()) {
                return invalid("media overlay coverage family contains a duplicate path");
            }
        }
    }

    let results = coverage
        .package_relationships
        .iter()
        .chain(
            coverage
                .smil_extraction
                .iter()
                .chain(&coverage.smil_relationships)
                .chain(&coverage.fragment_observations)
                .chain(&coverage.media_inspection)
                .map(|entry| &entry.result),
        )
        .collect::<Vec<_>>();
    let complete_count = results
        .iter()
        .filter(|result| matches!(result, MediaOverlayCoverageResult::Complete {}))
        .count();
    let partial_count = results
        .iter()
        .filter(|result| matches!(result, MediaOverlayCoverageResult::Partial { .. }))
        .count();
    let unavailable_count = results
        .iter()
        .filter(|result| matches!(result, MediaOverlayCoverageResult::Unavailable { .. }))
        .count();
    if partial_count + unavailable_count > MAX_MEDIA_OVERLAY_COVERAGE_ISSUES {
        return invalid("media overlay coverage exceeds its issue bound");
    }
    let coherent = match coverage.state {
        MediaOverlayCoverageState::Inapplicable => applicable_count == 0,
        MediaOverlayCoverageState::Complete => {
            applicable_count > 0 && complete_count == applicable_count
        }
        MediaOverlayCoverageState::Partial => {
            applicable_count > 0
                && partial_count + unavailable_count > 0
                && complete_count + partial_count > 0
        }
        MediaOverlayCoverageState::Unavailable => {
            applicable_count > 0 && unavailable_count == applicable_count
        }
    };
    if !coherent {
        return invalid("media overlay aggregate coverage state is inconsistent");
    }
    Ok(())
}

pub(crate) fn coverage_contains_path(entries: &[MediaOverlayPathCoverage], path: &str) -> bool {
    entries.iter().any(|entry| entry.canonical_path == path)
}
