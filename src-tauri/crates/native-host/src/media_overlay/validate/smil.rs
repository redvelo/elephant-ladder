//! Authored SMIL as the host recorded it: declarations, segments, bindings,
//! and the references that reach back into publication resources.

use super::package::available_package_fields;
use super::primitive::{
    clip_begin_clock, invalid, parsed_clock, validate_clock, validate_declaration_index,
    validate_optional_string, validate_path, validate_sorted_unique_indexes, validate_string,
};
use crate::media_overlay::{
    FragmentExistence, MAX_MEDIA_OVERLAY_SEMANTIC_DEPTH, MAX_MEDIA_OVERLAY_TOKENS_PER_NODE,
    MediaOverlayBinding, MediaOverlayBindingAvailability, MediaOverlayBindingIssue,
    MediaOverlayMalformedReason, MediaOverlayReference, MediaOverlaySegment,
    MediaOverlaySegmentPlayability, MediaOverlaySegmentUnavailableReason,
    MediaOverlayUnsupportedReason,
};
use crate::media_overlay::{
    MAX_MEDIA_OVERLAY_AMBIGUOUS_CANDIDATES, MediaOverlayDeclaration, MediaOverlayManifestId,
    MediaOverlayPackageFacts, SmilClock,
};

pub(crate) fn validate_declaration_candidates(
    authored_idref: &str,
    candidates: &[MediaOverlayDeclaration],
    package: &MediaOverlayPackageFacts,
) -> Result<(), String> {
    let fields = available_package_fields(package)
        .ok_or_else(|| "declaration candidates require available package facts".to_owned())?;
    if candidates.len() < 2 || candidates.len() > MAX_MEDIA_OVERLAY_AMBIGUOUS_CANDIDATES {
        return invalid("ambiguous declaration candidates exceed their bounds");
    }
    let indexes = candidates
        .iter()
        .map(|declaration| declaration.declaration_index)
        .collect::<Vec<_>>();
    validate_sorted_unique_indexes(&indexes, fields.declarations.len(), true)?;
    for declaration in candidates {
        validate_registry_declaration(declaration, package)?;
        if !matches!(
            &declaration.authored_id,
            MediaOverlayManifestId::Valid { value } if value == authored_idref
        ) {
            return invalid("ambiguous candidate does not match the authored IDREF");
        }
    }
    Ok(())
}

pub(crate) fn validate_declaration(declaration: &MediaOverlayDeclaration) -> Result<(), String> {
    match &declaration.authored_id {
        MediaOverlayManifestId::Missing {} => {}
        MediaOverlayManifestId::Valid { value } => validate_string(value, false)?,
        MediaOverlayManifestId::Invalid { value } => validate_string(value, true)?,
    }
    validate_optional_string(&declaration.authored_href)?;
    validate_optional_string(&declaration.media_type)?;
    if let Some(path) = &declaration.canonical_path {
        validate_path(path)?;
        if declaration.authored_href.is_none() {
            return invalid("declaration without an authored href has a canonical path");
        }
    }
    Ok(())
}

pub(crate) fn validate_registry_declaration(
    declaration: &MediaOverlayDeclaration,
    package: &MediaOverlayPackageFacts,
) -> Result<(), String> {
    let package = available_package_fields(package)
        .ok_or_else(|| "declaration requires available package facts".to_owned())?;
    validate_declaration_index(declaration.declaration_index, package.declarations.len())?;
    if package.declarations[declaration.declaration_index as usize] != *declaration {
        return invalid("embedded declaration does not match the description registry");
    }
    Ok(())
}

pub(crate) fn validate_segment(segment: &MediaOverlaySegment) -> Result<(), String> {
    if segment.semantic_path.is_empty()
        || segment.semantic_path.len() > MAX_MEDIA_OVERLAY_SEMANTIC_DEPTH
    {
        return invalid("segment semantic path is empty or too deep");
    }
    for node in &segment.semantic_path {
        if node.epub_types.len() > MAX_MEDIA_OVERLAY_TOKENS_PER_NODE {
            return invalid("semantic node has too many EPUB type tokens");
        }
        for token in &node.epub_types {
            validate_string(token, false)?;
            if token.chars().any(char::is_whitespace) {
                return invalid("semantic EPUB type is not one token");
            }
        }
        if let Some(reference) = &node.text_reference {
            validate_reference(reference)?;
        }
    }
    validate_binding(&segment.text)?;
    validate_binding(&segment.audio)?;
    validate_clock(&segment.clip_begin)?;
    validate_clock(&segment.clip_end)?;

    if matches!(
        segment.playability,
        MediaOverlaySegmentPlayability::Playable {}
    ) {
        if !matches!(
            segment.text.reference,
            MediaOverlayReference::LocalFragment {
                fragment_exists: FragmentExistence::Exists {},
                ..
            }
        ) || !matches!(
            segment.audio.reference,
            MediaOverlayReference::LocalResource { .. }
        ) || !matches!(
            segment.text.availability,
            MediaOverlayBindingAvailability::Available {}
        ) || !matches!(
            segment.audio.availability,
            MediaOverlayBindingAvailability::Available {}
        ) {
            return invalid("playable segment does not have usable local bindings");
        }
        let begin = clip_begin_clock(&segment.clip_begin)
            .ok_or_else(|| "playable segment has an unusable clip begin".to_owned())?;
        if matches!(segment.clip_end, SmilClock::Unrecognized {}) {
            return invalid("playable segment has an unrecognized clip end");
        }
        if parsed_clock(&segment.clip_end).is_some_and(|end| begin >= end) {
            return invalid("playable segment clip interval is empty or reversed");
        }
    }
    match segment.playability {
        MediaOverlaySegmentPlayability::Unsupported { reason } => match reason {
            MediaOverlayUnsupportedReason::TextToSpeechUnavailable => {
                if !matches!(
                    segment.text.availability,
                    MediaOverlayBindingAvailability::Available {}
                ) {
                    return invalid("text-only segment has no available supported text binding");
                }
                require_binding_issue(
                    &segment.audio,
                    MediaOverlayBindingIssue::MissingReference,
                    "text-only segment",
                )?;
            }
            MediaOverlayUnsupportedReason::FixedLayoutText
            | MediaOverlayUnsupportedReason::SvgText
            | MediaOverlayUnsupportedReason::NavigationDocument
            | MediaOverlayUnsupportedReason::UnsupportedTextFragment => {
                require_binding_issue(
                    &segment.text,
                    MediaOverlayBindingIssue::UnsupportedTarget,
                    "unsupported text segment",
                )?;
            }
            MediaOverlayUnsupportedReason::UnsupportedAudioFormat => {
                require_binding_issue(
                    &segment.audio,
                    MediaOverlayBindingIssue::UnsupportedMediaType,
                    "unsupported audio segment",
                )?;
            }
        },
        MediaOverlaySegmentPlayability::Malformed { reason } => match reason {
            MediaOverlayMalformedReason::MissingText => {
                require_binding_issue(
                    &segment.text,
                    MediaOverlayBindingIssue::MissingReference,
                    "missing-text segment",
                )?;
            }
            MediaOverlayMalformedReason::UnrecognizedClock
                if !matches!(segment.clip_begin, SmilClock::Unrecognized {})
                    && !matches!(segment.clip_end, SmilClock::Unrecognized {}) =>
            {
                return invalid("unrecognized-clock has no unrecognized clock evidence");
            }
            MediaOverlayMalformedReason::EmptyClip
                if clip_begin_clock(&segment.clip_begin) != parsed_clock(&segment.clip_end)
                    || parsed_clock(&segment.clip_end).is_none() =>
            {
                return invalid("empty-clip contradicts the retained clocks");
            }
            MediaOverlayMalformedReason::ReversedClip
                if !matches!(
                    (clip_begin_clock(&segment.clip_begin), parsed_clock(&segment.clip_end)),
                    (Some(begin), Some(end)) if begin > end
                ) =>
            {
                return invalid("reversed-clip contradicts the retained clocks");
            }
            _ => {}
        },
        MediaOverlaySegmentPlayability::Unavailable { reason } => match reason {
            MediaOverlaySegmentUnavailableReason::TextBindingUnavailable => {
                if !matches!(
                    segment.text.availability,
                    MediaOverlayBindingAvailability::Unavailable { .. }
                ) {
                    return invalid("text-binding-unavailable contradicts binding availability");
                }
            }
            MediaOverlaySegmentUnavailableReason::AudioBindingUnavailable => {
                if !matches!(
                    segment.audio.availability,
                    MediaOverlayBindingAvailability::Unavailable { .. }
                ) {
                    return invalid("audio-binding-unavailable contradicts binding availability");
                }
            }
            MediaOverlaySegmentUnavailableReason::AmbiguousContinuation
            | MediaOverlaySegmentUnavailableReason::NumericOverflow => {}
        },
        MediaOverlaySegmentPlayability::Playable {} => {}
    }
    Ok(())
}

pub(crate) fn validate_binding(binding: &MediaOverlayBinding) -> Result<(), String> {
    validate_reference(&binding.reference)?;
    match (&binding.reference, &binding.availability) {
        (
            MediaOverlayReference::LocalResource { .. },
            MediaOverlayBindingAvailability::Available {},
        )
        | (
            MediaOverlayReference::LocalFragment {
                fragment_exists: FragmentExistence::Exists {},
                ..
            },
            MediaOverlayBindingAvailability::Available {},
        ) => Ok(()),
        (_, MediaOverlayBindingAvailability::Available {}) => {
            invalid("available binding does not have an available local reference")
        }
        (reference, MediaOverlayBindingAvailability::Unavailable { reason }) => {
            let consistent = match reference {
                MediaOverlayReference::Absent {} => {
                    *reason == MediaOverlayBindingIssue::MissingReference
                }
                MediaOverlayReference::MissingLocalResource { .. } => {
                    *reason == MediaOverlayBindingIssue::MissingResource
                }
                MediaOverlayReference::IndexedLocalResourceUnavailable { .. }
                | MediaOverlayReference::IndexedLocalFragmentUnavailable { .. } => {
                    *reason == MediaOverlayBindingIssue::ResourceBytesUnavailable
                }
                MediaOverlayReference::Remote { .. }
                | MediaOverlayReference::Data { .. }
                | MediaOverlayReference::External { .. }
                | MediaOverlayReference::Invalid { .. } => {
                    *reason == MediaOverlayBindingIssue::UnsupportedTarget
                }
                MediaOverlayReference::LocalFragment {
                    fragment_exists: FragmentExistence::Missing {},
                    ..
                } => *reason == MediaOverlayBindingIssue::MissingFragment,
                MediaOverlayReference::LocalFragment {
                    fragment_exists: FragmentExistence::Ambiguous {},
                    ..
                } => *reason == MediaOverlayBindingIssue::AmbiguousFragment,
                MediaOverlayReference::LocalFragment {
                    fragment_exists: FragmentExistence::Unknown {},
                    ..
                } => *reason == MediaOverlayBindingIssue::UnverifiedFragment,
                MediaOverlayReference::LocalResource { .. }
                | MediaOverlayReference::LocalFragment {
                    fragment_exists: FragmentExistence::Exists {},
                    ..
                } => matches!(
                    reason,
                    MediaOverlayBindingIssue::UnsupportedTarget
                        | MediaOverlayBindingIssue::UnsupportedMediaType
                        | MediaOverlayBindingIssue::AudioBackingUnavailable
                ),
            };
            if !consistent {
                return invalid("binding reason contradicts its reference state");
            }
            Ok(())
        }
    }
}

pub(crate) fn require_binding_issue(
    binding: &MediaOverlayBinding,
    expected: MediaOverlayBindingIssue,
    context: &str,
) -> Result<(), String> {
    if !matches!(
        binding.availability,
        MediaOverlayBindingAvailability::Unavailable { reason } if reason == expected
    ) {
        return Err(format!("{context} contradicts binding evidence"));
    }
    Ok(())
}

pub(crate) fn validate_reference(reference: &MediaOverlayReference) -> Result<(), String> {
    match reference {
        MediaOverlayReference::Absent {} => Ok(()),
        MediaOverlayReference::LocalResource {
            authored_href,
            canonical_path,
            query,
        } => {
            validate_string(authored_href, true)?;
            validate_path(canonical_path)?;
            validate_optional_string(query)
        }
        MediaOverlayReference::LocalFragment {
            authored_href,
            canonical_path,
            query,
            fragment,
            ..
        } => {
            validate_string(authored_href, true)?;
            validate_path(canonical_path)?;
            validate_optional_string(query)?;
            validate_string(fragment, true)
        }
        MediaOverlayReference::MissingLocalResource {
            authored_href,
            canonical_path,
        } => {
            validate_string(authored_href, true)?;
            validate_path(canonical_path)
        }
        MediaOverlayReference::IndexedLocalResourceUnavailable {
            authored_href,
            canonical_path,
            query,
        } => {
            validate_string(authored_href, true)?;
            validate_path(canonical_path)?;
            validate_optional_string(query)
        }
        MediaOverlayReference::IndexedLocalFragmentUnavailable {
            authored_href,
            canonical_path,
            query,
            fragment,
            ..
        } => {
            validate_string(authored_href, true)?;
            validate_path(canonical_path)?;
            validate_optional_string(query)?;
            validate_string(fragment, true)
        }
        MediaOverlayReference::Remote { authored_href }
        | MediaOverlayReference::Data { authored_href }
        | MediaOverlayReference::External { authored_href }
        | MediaOverlayReference::Invalid { authored_href } => validate_string(authored_href, true),
    }
}
