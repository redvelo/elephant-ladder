//! What a segment points at: its text and audio bindings, the references it
//! resolves, and the audio a publication needs backed.

use super::project_declaration;
use crate::media_overlay::validate::local_reference_path;
use crate::media_overlay::{
    FragmentExistence, MediaOverlayAvailabilityIssue, MediaOverlayBinding,
    MediaOverlayBindingAvailability, MediaOverlayBindingIssue, MediaOverlayDeclaration,
    MediaOverlayDescription, MediaOverlayLocalBytes, MediaOverlayPlayback, MediaOverlayReference,
    MediaOverlaySegmentPlayability, MediaOverlaySegmentUnavailableReason, MediaOverlayTimeline,
    MediaOverlayTimelineAvailability, MediaOverlayTimelineIssue, OverlayAssociationState,
    ReadingOrderContentTarget,
};
use epub_navigator::kernel::DocumentAdapter;
use epub_stack::PublicationAnalysis;
use epub_stack::analysis::reference::{HrefReference, HrefTarget};
use epub_stack::media_overlay::SmilNodeRef;
use epub_stack::resource::{EpubPath, IdrefTarget, ManifestOrdinal, ProviderPresence, ResourceRef};
use std::collections::HashMap;

/// Returns every distinct canonical local audio path an available binding depends on, across
/// every derived timeline.
///
/// The caller (import-time sidecar materialization) prepares exactly these resources; anything
/// not returned here never needed backing in the first place.
pub(crate) fn audio_resources_to_back(timelines: &[MediaOverlayTimeline]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for timeline in timelines {
        for segment in &timeline.segments {
            if matches!(
                segment.audio.availability,
                MediaOverlayBindingAvailability::Available {}
            ) && let Some(path) = local_reference_path(&segment.audio.reference)
                && seen.insert(path.to_owned())
            {
                paths.push(path.to_owned());
            }
        }
    }
    paths
}

/// Downgrades every segment, timeline, and playback rollup that depended on an audio resource
/// whose sidecar preparation failed, without discarding package facts or coverage.
///
/// Called once, after import-time sidecar preparation, before persisting the derivation. This is
/// the only place `MediaOverlayBindingIssue::AudioBackingUnavailable` is produced.
pub(crate) fn apply_audio_backing_failures(
    description: &mut MediaOverlayDescription,
    timelines: &mut [MediaOverlayTimeline],
    failed_canonical_paths: &std::collections::HashSet<String>,
) {
    if failed_canonical_paths.is_empty() {
        return;
    }
    for timeline in timelines.iter_mut() {
        let mut changed = false;
        for segment in timeline.segments.iter_mut() {
            let backing_failed = local_reference_path(&segment.audio.reference)
                .is_some_and(|path| failed_canonical_paths.contains(path));
            if !backing_failed
                || !matches!(
                    segment.audio.availability,
                    MediaOverlayBindingAvailability::Available {}
                )
            {
                continue;
            }
            segment.audio.availability = MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::AudioBackingUnavailable,
            };
            if matches!(
                segment.playability,
                MediaOverlaySegmentPlayability::Playable {}
            ) {
                segment.playability = MediaOverlaySegmentPlayability::Unavailable {
                    reason: MediaOverlaySegmentUnavailableReason::AudioBindingUnavailable,
                };
            }
            changed = true;
        }
        if !changed {
            continue;
        }
        timeline.playable_segment_count = timeline
            .segments
            .iter()
            .filter(|segment| {
                matches!(
                    segment.playability,
                    MediaOverlaySegmentPlayability::Playable {}
                )
            })
            .count() as u32;
        if timeline.playable_segment_count == 0 {
            timeline.availability = MediaOverlayTimelineAvailability::Unavailable {
                reason: MediaOverlayTimelineIssue::NoPlayableSegments,
            };
        }
    }
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut description.playback {
        for overlay in overlays.iter_mut() {
            if let Some(timeline) = timelines.get(overlay.overlay_index as usize) {
                overlay.playable_segment_count = timeline.playable_segment_count;
                overlay.availability = timeline.availability.clone();
            }
        }
        let playable_timeline_count = timelines
            .iter()
            .filter(|timeline| {
                matches!(
                    timeline.availability,
                    MediaOverlayTimelineAvailability::Available {}
                )
            })
            .count();
        if playable_timeline_count == 0 {
            description.playback = MediaOverlayPlayback::Unavailable {
                reason: MediaOverlayAvailabilityIssue::NoPlayableSegments,
            };
        }
    }
}

pub(crate) fn project_text_binding(
    analysis: &PublicationAnalysis,
    adapters: &HashMap<&str, DocumentAdapter>,
    node: Option<SmilNodeRef<'_>>,
) -> MediaOverlayBinding {
    let reference = project_reference(analysis, node.and_then(SmilNodeRef::text_reference));
    let availability = match &reference {
        MediaOverlayReference::Absent {} => MediaOverlayBindingAvailability::Unavailable {
            reason: MediaOverlayBindingIssue::MissingReference,
        },
        MediaOverlayReference::MissingLocalResource { .. } => {
            MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::MissingResource,
            }
        }
        MediaOverlayReference::IndexedLocalResourceUnavailable { .. }
        | MediaOverlayReference::IndexedLocalFragmentUnavailable { .. } => {
            MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::ResourceBytesUnavailable,
            }
        }
        MediaOverlayReference::Remote { .. }
        | MediaOverlayReference::Data { .. }
        | MediaOverlayReference::External { .. }
        | MediaOverlayReference::Invalid { .. }
        | MediaOverlayReference::LocalResource { .. } => {
            MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            }
        }
        MediaOverlayReference::LocalFragment {
            fragment_exists,
            canonical_path,
            ..
        } => match fragment_exists {
            FragmentExistence::Exists {} => {
                if is_supported_text_target(adapters, canonical_path) {
                    MediaOverlayBindingAvailability::Available {}
                } else {
                    MediaOverlayBindingAvailability::Unavailable {
                        reason: MediaOverlayBindingIssue::UnsupportedTarget,
                    }
                }
            }
            FragmentExistence::Missing {} => MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::MissingFragment,
            },
            FragmentExistence::Ambiguous {} => MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::AmbiguousFragment,
            },
            FragmentExistence::Unknown {} => MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnverifiedFragment,
            },
        },
    };
    MediaOverlayBinding {
        reference,
        availability,
    }
}

pub(crate) fn fragment_existence(
    analysis: &PublicationAnalysis,
    resource: epub_stack::resource::ResourceOrdinal,
    fragment: &str,
    exists: Option<bool>,
) -> FragmentExistence {
    match exists {
        Some(true) => {
            let matching = analysis
                .resource(resource)
                .and_then(|resource| resource.content().value())
                .and_then(|content| {
                    content
                        .as_xhtml()
                        .map(|facts| facts.fragments())
                        .or_else(|| content.as_svg().map(|facts| facts.fragments()))
                })
                .map_or(1, |fragments| {
                    fragments
                        .iter()
                        .filter(|fact| fact.id() == fragment)
                        .count()
                });
            if matching > 1 {
                FragmentExistence::Ambiguous {}
            } else {
                FragmentExistence::Exists {}
            }
        }
        Some(false) => FragmentExistence::Missing {},
        None => FragmentExistence::Unknown {},
    }
}

pub(crate) fn is_supported_text_target(
    adapters: &HashMap<&str, DocumentAdapter>,
    canonical_path: &str,
) -> bool {
    match adapters.get(canonical_path) {
        // A fixed page and an SVG document both have elements a class can be put on, so
        // narration binds to them the way it binds to a reflowing document. An image page is one
        // picture, with no element a reference can name.
        Some(
            DocumentAdapter::ReflowableXhtml
            | DocumentAdapter::FixedXhtml
            | DocumentAdapter::FixedSvg,
        ) => true,
        Some(DocumentAdapter::FixedImage) => false,
        // A text target outside the projected reading order (e.g. a non-linear or unreferenced
        // document) cannot be classified; treat it as supported rather than guessing a specific
        // unsupported reason.
        None => true,
    }
}

pub(crate) fn project_audio_binding(
    analysis: &PublicationAnalysis,
    node: Option<SmilNodeRef<'_>>,
) -> MediaOverlayBinding {
    let Some(reference) = node.and_then(SmilNodeRef::audio_reference) else {
        return MediaOverlayBinding {
            reference: MediaOverlayReference::Absent {},
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::MissingReference,
            },
        };
    };
    let authored_href = reference.declared().as_str().to_owned();
    match reference.target() {
        HrefTarget::Resource {
            resource: resource_ordinal,
            query,
        } => match analysis.resources().resource(*resource_ordinal) {
            Some(resolved) => {
                let canonical_path = resolved
                    .local_path()
                    .map(|path| path.as_str().to_owned())
                    .unwrap_or_default();
                match resolved.presence() {
                    ProviderPresence::Present { .. } => {
                        let reference = MediaOverlayReference::LocalResource {
                            authored_href,
                            canonical_path,
                            query: query.clone(),
                        };
                        let availability = if is_supported_audio_target(resolved) {
                            MediaOverlayBindingAvailability::Available {}
                        } else {
                            MediaOverlayBindingAvailability::Unavailable {
                                reason: MediaOverlayBindingIssue::UnsupportedMediaType,
                            }
                        };
                        MediaOverlayBinding {
                            reference,
                            availability,
                        }
                    }
                    ProviderPresence::Missing | ProviderPresence::NotApplicable => {
                        MediaOverlayBinding {
                            reference: MediaOverlayReference::IndexedLocalResourceUnavailable {
                                authored_href,
                                canonical_path,
                                query: query.clone(),
                            },
                            availability: MediaOverlayBindingAvailability::Unavailable {
                                reason: MediaOverlayBindingIssue::ResourceBytesUnavailable,
                            },
                        }
                    }
                }
            }
            None => MediaOverlayBinding {
                reference: MediaOverlayReference::Invalid { authored_href },
                availability: MediaOverlayBindingAvailability::Unavailable {
                    reason: MediaOverlayBindingIssue::UnsupportedTarget,
                },
            },
        },
        HrefTarget::Fragment { .. } => MediaOverlayBinding {
            reference: MediaOverlayReference::Invalid { authored_href },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            },
        },
        HrefTarget::MissingLocal(path) => MediaOverlayBinding {
            reference: MediaOverlayReference::MissingLocalResource {
                authored_href,
                canonical_path: path.as_str().to_owned(),
            },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::MissingResource,
            },
        },
        HrefTarget::Remote { .. } => MediaOverlayBinding {
            reference: MediaOverlayReference::Remote { authored_href },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            },
        },
        HrefTarget::Data(_) => MediaOverlayBinding {
            reference: MediaOverlayReference::Data { authored_href },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            },
        },
        HrefTarget::External(_) => MediaOverlayBinding {
            reference: MediaOverlayReference::External { authored_href },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            },
        },
        HrefTarget::Invalid(_) => MediaOverlayBinding {
            reference: MediaOverlayReference::Invalid { authored_href },
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::UnsupportedTarget,
            },
        },
    }
}

pub(crate) const SUPPORTED_AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "m4a", "mp4", "aac", "oga", "ogg", "wav"];

pub(crate) fn is_supported_audio_target(resource: ResourceRef<'_>) -> bool {
    if resource
        .declarations()
        .any(|declaration| declaration.media_type().is_some_and(is_audio_media_type))
    {
        return true;
    }
    resource
        .local_path()
        .and_then(EpubPath::extension)
        .is_some_and(|extension| SUPPORTED_AUDIO_EXTENSIONS.contains(&extension))
}

pub(crate) fn is_audio_media_type(media_type: &epub_stack::resource::MediaType) -> bool {
    media_type
        .as_str()
        .to_ascii_lowercase()
        .starts_with("audio/")
}

pub(crate) fn project_reference(
    analysis: &PublicationAnalysis,
    reference: Option<&HrefReference>,
) -> MediaOverlayReference {
    let Some(reference) = reference else {
        return MediaOverlayReference::Absent {};
    };
    let authored_href = reference.declared().as_str().to_owned();
    match reference.target() {
        HrefTarget::Resource { resource, query } => {
            match analysis.resources().resource(*resource) {
                Some(resolved) => {
                    let canonical_path = resolved
                        .local_path()
                        .map(|path| path.as_str().to_owned())
                        .unwrap_or_default();
                    match resolved.presence() {
                        ProviderPresence::Present { .. } => MediaOverlayReference::LocalResource {
                            authored_href,
                            canonical_path,
                            query: query.clone(),
                        },
                        ProviderPresence::Missing | ProviderPresence::NotApplicable => {
                            MediaOverlayReference::IndexedLocalResourceUnavailable {
                                authored_href,
                                canonical_path,
                                query: query.clone(),
                            }
                        }
                    }
                }
                None => MediaOverlayReference::Invalid { authored_href },
            }
        }
        HrefTarget::Fragment {
            resource,
            query,
            fragment,
            exists,
        } => match analysis.resources().resource(*resource) {
            Some(resolved) => {
                let canonical_path = resolved
                    .local_path()
                    .map(|path| path.as_str().to_owned())
                    .unwrap_or_default();
                let fragment_exists = fragment_existence(analysis, *resource, fragment, *exists);
                match resolved.presence() {
                    ProviderPresence::Present { .. } => MediaOverlayReference::LocalFragment {
                        authored_href,
                        canonical_path,
                        query: query.clone(),
                        fragment: fragment.clone(),
                        fragment_exists,
                    },
                    ProviderPresence::Missing | ProviderPresence::NotApplicable => {
                        MediaOverlayReference::IndexedLocalFragmentUnavailable {
                            authored_href,
                            canonical_path,
                            query: query.clone(),
                            fragment: fragment.clone(),
                            fragment_exists,
                        }
                    }
                }
            }
            None => MediaOverlayReference::Invalid { authored_href },
        },
        HrefTarget::Remote { .. } => MediaOverlayReference::Remote { authored_href },
        HrefTarget::Data(_) => MediaOverlayReference::Data { authored_href },
        HrefTarget::External(_) => MediaOverlayReference::External { authored_href },
        HrefTarget::MissingLocal(path) => MediaOverlayReference::MissingLocalResource {
            authored_href,
            canonical_path: path.as_str().to_owned(),
        },
        HrefTarget::Invalid(_) => MediaOverlayReference::Invalid { authored_href },
    }
}

pub(crate) fn build_content_target(
    analysis: &PublicationAnalysis,
    occurrence: epub_stack::resource::ReadingOrderOccurrenceRef<'_>,
    declaration_index_by_ordinal: &HashMap<u32, usize>,
    package_declarations: &[MediaOverlayDeclaration],
) -> ReadingOrderContentTarget {
    let declaration_for = |ordinal: ManifestOrdinal| -> MediaOverlayDeclaration {
        declaration_index_by_ordinal
            .get(&ordinal.as_u32())
            .and_then(|index| package_declarations.get(*index))
            .cloned()
            .unwrap_or_else(|| {
                project_declaration(
                    analysis
                        .resources()
                        .declaration(ordinal)
                        .expect("resolved declaration ordinal is valid"),
                    ordinal.as_u32(),
                )
            })
    };
    let authored_idref = || {
        occurrence
            .idref()
            .map(|value| value.as_str().to_owned())
            .unwrap_or_default()
    };
    match occurrence.target() {
        None => ReadingOrderContentTarget::MissingIdref {},
        Some(IdrefTarget::Invalid | IdrefTarget::Missing) => {
            ReadingOrderContentTarget::MissingDeclaration {
                authored_idref: authored_idref(),
            }
        }
        Some(IdrefTarget::Ambiguous(candidates)) => {
            ReadingOrderContentTarget::AmbiguousDeclaration {
                authored_idref: authored_idref(),
                candidates: candidates.iter().copied().map(declaration_for).collect(),
            }
        }
        Some(IdrefTarget::Declaration(declaration)) => {
            let declaration_projection = declaration_for(*declaration);
            match occurrence.resource() {
                None => ReadingOrderContentTarget::DeclarationWithoutResource {
                    declaration: declaration_projection,
                },
                Some(resource_ref) => {
                    let canonical_path = resource_ref
                        .local_path()
                        .map(|path| path.as_str().to_owned())
                        .unwrap_or_default();
                    let bytes = match resource_ref.presence() {
                        ProviderPresence::Present { .. } => MediaOverlayLocalBytes::Present {},
                        ProviderPresence::Missing | ProviderPresence::NotApplicable => {
                            MediaOverlayLocalBytes::Missing {}
                        }
                    };
                    ReadingOrderContentTarget::Resolved {
                        declaration: declaration_projection,
                        canonical_path,
                        bytes,
                    }
                }
            }
        }
    }
}

pub(crate) fn entry_segment_indexes_for(
    timelines: &[MediaOverlayTimeline],
    overlay: &OverlayAssociationState,
    content: &ReadingOrderContentTarget,
) -> Vec<u32> {
    let OverlayAssociationState::Resolved { overlay_index, .. } = overlay else {
        return Vec::new();
    };
    let ReadingOrderContentTarget::Resolved { canonical_path, .. } = content else {
        return Vec::new();
    };
    let Some(timeline) = timelines.get(*overlay_index as usize) else {
        return Vec::new();
    };
    timeline
        .segments
        .iter()
        .filter(|segment| {
            local_reference_path(&segment.text.reference) == Some(canonical_path.as_str())
        })
        .map(|segment| segment.segment_index)
        .collect()
}
