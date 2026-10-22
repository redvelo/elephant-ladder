//! Derives the bounded [`MediaOverlayDescription`] from one publication analysis snapshot.
//!
//! This module performs no HTTP, persistence, or audio-backing work. It reads only the public
//! `epub_stack` analysis surface and Navigator's projected reading order, and it never reruns
//! Stack analysis itself. Every discriminated-union shape produced here must satisfy
//! `media_overlay::validate_media_overlay_description`; the test module exercises that
//! validator directly against representative fixtures.
//!
//! Audio bindings are derived assuming a local audio resource can eventually be backed for
//! playback. [`crate::media_overlay::backing`] runs after this module and may downgrade specific
//! bindings to `AudioBackingUnavailable` when sidecar preparation fails; see
//! `apply_audio_backing_outcomes`.

use std::collections::HashMap;

use epub_navigator::kernel::{DocumentAdapter, OccurrenceDocumentTarget, ReadingOrderOccurrence};
use epub_stack::PublicationAnalysis;
use epub_stack::analysis::reference::ManifestTarget;
use epub_stack::analysis::{AnalysisIssue, AnalysisLimit, AnalysisOutcome};
use epub_stack::media_overlay::{
    MediaOverlayAssociationRef, MediaOverlayDurationMeta, MediaTime, SmilTime,
};
use epub_stack::resource::{ManifestDeclarationRef, ManifestIdValue, ManifestOrdinal, ResourceRef};

use crate::media_overlay::{
    EffectiveLinear, MAX_MEDIA_OVERLAY_DECLARATIONS, MAX_MEDIA_OVERLAY_TIMELINES,
    MediaOverlayAnalysisIssue, MediaOverlayAssociation, MediaOverlayAvailabilityIssue,
    MediaOverlayCoverage, MediaOverlayCoverageResult, MediaOverlayCoverageState,
    MediaOverlayDeclaration, MediaOverlayDescription, MediaOverlayDurationMetadata,
    MediaOverlayManifestId, MediaOverlayPackageFacts, MediaOverlayPackageIssue,
    MediaOverlayPackageMetadata, MediaOverlayPathCoverage, MediaOverlayPlayback,
    MediaOverlayTimeline, MediaOverlayTimelineAvailability, MediaOverlayTimelineIssue,
    MediaOverlayTimelineSummary, OverlayAssociationState, SmilClock,
};

/// The bounded description plus every full timeline (including segments) it summarizes.
///
/// The description alone (persisted as `media_overlay_json`) is what
/// `GET .../media-overlays` returns; `timelines` (persisted separately, since one timeline's
/// segments can be much larger than the rest of the description combined) is what
/// `GET .../media-overlays/{overlayIndex}` slices from. Both are derived together so an
/// unavailable timeline's summary and its full unavailable result always agree.
pub(crate) struct MediaOverlayDerivation {
    pub description: MediaOverlayDescription,
    pub timelines: Vec<MediaOverlayTimeline>,
}

/// Derives the complete Media Overlay description for one publication analysis.
///
/// `reading_order` must be Navigator's projection of the same `analysis` snapshot
/// (`epub_navigator::kernel::project_reading_order`); occurrence ordinals in both are the same
/// zero-based authored spine positions.
pub(crate) fn derive_media_overlay_description(
    analysis: &PublicationAnalysis,
    reading_order: &[ReadingOrderOccurrence],
) -> MediaOverlayDerivation {
    let adapters = resource_adapter_map(reading_order);
    let mut coverage = CoverageBuilder::default();

    let (package_declarations, declaration_index_by_ordinal) =
        collect_package_declarations(analysis);
    if package_declarations.len() > MAX_MEDIA_OVERLAY_DECLARATIONS {
        return MediaOverlayDerivation {
            description: MediaOverlayDescription {
                package: MediaOverlayPackageFacts::Unavailable {
                    reason: MediaOverlayPackageIssue::RepresentationLimitExceeded,
                },
                coverage: coverage.finish_representation_limit(),
                playback: MediaOverlayPlayback::Unavailable {
                    reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
                },
            },
            timelines: Vec::new(),
        };
    }

    let mo_facts = analysis.media_overlays();
    let declared = mo_facts.present();
    let metadata = project_package_metadata(mo_facts.metadata(), &declaration_index_by_ordinal);

    if !declared {
        return MediaOverlayDerivation {
            description: MediaOverlayDescription {
                package: MediaOverlayPackageFacts::Available {
                    declared: false,
                    declarations: Vec::new(),
                    metadata,
                },
                coverage: coverage.finish_inapplicable(),
                playback: MediaOverlayPlayback::Absent {},
            },
            timelines: Vec::new(),
        };
    }

    let package = MediaOverlayPackageFacts::Available {
        declared,
        declarations: package_declarations.clone(),
        metadata,
    };

    // Pass 1: build one timeline per distinct resolved overlay resource, and record the overlay
    // association state for every occurrence that authors a media-overlay relationship.
    let mut timelines: Vec<MediaOverlayTimeline> = Vec::new();
    let mut timeline_index_by_resource: HashMap<u32, u32> = HashMap::new();
    let mut overlay_state_by_occurrence: HashMap<u32, OverlayAssociationState> = HashMap::new();
    let mut representation_limit_exceeded = false;

    for association in analysis.media_overlay_associations() {
        let occurrence_index = association.reading_order().ordinal().as_u32();
        let state = resolve_overlay_state(
            analysis,
            &adapters,
            &association,
            &declaration_index_by_ordinal,
            &package_declarations,
            &mut timeline_index_by_resource,
            &mut timelines,
            &mut coverage,
            &mut representation_limit_exceeded,
        );
        overlay_state_by_occurrence.insert(occurrence_index, state);
    }

    if representation_limit_exceeded || timelines.len() > MAX_MEDIA_OVERLAY_TIMELINES {
        return MediaOverlayDerivation {
            description: MediaOverlayDescription {
                package,
                coverage: coverage.finish_representation_limit(),
                playback: MediaOverlayPlayback::Unavailable {
                    reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
                },
            },
            timelines: Vec::new(),
        };
    }

    // Pass 2: build one association for every authored reading-order occurrence.
    let mut associations = Vec::new();
    for occurrence in analysis.resources().reading_order() {
        let occurrence_index = occurrence.ordinal().as_u32();
        let content = build_content_target(
            analysis,
            occurrence,
            &declaration_index_by_ordinal,
            &package_declarations,
        );
        let overlay = overlay_state_by_occurrence
            .remove(&occurrence_index)
            .unwrap_or(OverlayAssociationState::NotDeclared {});
        let entry_segment_indexes = entry_segment_indexes_for(&timelines, &overlay, &content);
        associations.push(MediaOverlayAssociation {
            occurrence_index,
            effective_linear: match occurrence.linear() {
                epub_stack::package::spine::Linear::Yes => EffectiveLinear::Yes,
                epub_stack::package::spine::Linear::No => EffectiveLinear::No,
            },
            content,
            overlay,
            entry_segment_indexes,
        });
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

    let playback = if playable_timeline_count == 0 {
        MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::NoPlayableSegments,
        }
    } else {
        MediaOverlayPlayback::Available {
            overlays: timelines.iter().map(timeline_summary).collect(),
            associations,
        }
    };

    MediaOverlayDerivation {
        description: MediaOverlayDescription {
            package,
            coverage: coverage.finish(),
            playback,
        },
        timelines,
    }
}

fn timeline_summary(timeline: &MediaOverlayTimeline) -> MediaOverlayTimelineSummary {
    MediaOverlayTimelineSummary {
        overlay_index: timeline.overlay_index,
        canonical_path: timeline.canonical_path.clone(),
        declarations: timeline.declarations.clone(),
        skippable: timeline.skippable.clone(),
        escapable: timeline.escapable.clone(),
        segment_count: timeline.segment_count,
        playable_segment_count: timeline.playable_segment_count,
        availability: timeline.availability.clone(),
    }
}

fn collect_package_declarations(
    analysis: &PublicationAnalysis,
) -> (Vec<MediaOverlayDeclaration>, HashMap<u32, usize>) {
    // Every manifest declaration is retained, not only ones authoring a `media-overlay` idref or
    // a SMIL media type: a resolved reading-order association embeds its *content* declaration
    // regardless of whether that particular occurrence has narration, and every embedded
    // declaration must resolve by index against this one registry.
    let mut declarations = Vec::new();
    let mut index_by_ordinal = HashMap::new();
    for declaration in analysis.resources().declarations() {
        let index = declarations.len();
        index_by_ordinal.insert(declaration.ordinal().as_u32(), index);
        declarations.push(project_declaration(declaration, index as u32));
    }
    (declarations, index_by_ordinal)
}

fn resource_adapter_map(
    reading_order: &[ReadingOrderOccurrence],
) -> HashMap<&str, DocumentAdapter> {
    let mut map = HashMap::new();
    for occurrence in reading_order {
        if let OccurrenceDocumentTarget::Loadable { resource, adapter } = &occurrence.document {
            map.entry(resource.as_str()).or_insert(*adapter);
        }
    }
    map
}

fn project_declaration(
    declaration: ManifestDeclarationRef<'_>,
    index: u32,
) -> MediaOverlayDeclaration {
    let authored_id = match declaration.id() {
        ManifestIdValue::Missing => MediaOverlayManifestId::Missing {},
        ManifestIdValue::Valid(value) => MediaOverlayManifestId::Valid {
            value: value.clone(),
        },
        ManifestIdValue::Invalid(value) => MediaOverlayManifestId::Invalid {
            value: value.clone(),
        },
    };
    let authored_href = declaration.href().map(|href| href.as_str().to_owned());
    let canonical_path = declaration
        .resource()
        .and_then(ResourceRef::local_path)
        .map(|path| path.as_str().to_owned());
    let media_type = declaration
        .media_type()
        .map(|value| value.as_str().to_owned());
    MediaOverlayDeclaration {
        declaration_index: index,
        authored_id,
        authored_href,
        canonical_path,
        media_type,
    }
}

fn project_package_metadata(
    metadata: &epub_stack::media_overlay::MediaOverlayMetadata,
    declaration_index_by_ordinal: &HashMap<u32, usize>,
) -> MediaOverlayPackageMetadata {
    MediaOverlayPackageMetadata {
        total_durations: metadata
            .total_durations()
            .filter(|duration| !duration.duration().authored().is_empty())
            .map(|duration| project_duration_meta(duration, declaration_index_by_ordinal))
            .collect(),
        item_durations: metadata
            .item_durations()
            .filter(|duration| !duration.duration().authored().is_empty())
            .map(|duration| project_duration_meta(duration, declaration_index_by_ordinal))
            .collect(),
        narrators: metadata.narrators().to_vec(),
        active_classes: metadata.active_classes().to_vec(),
        playback_active_classes: metadata.playback_active_classes().to_vec(),
    }
}

fn project_duration_meta(
    duration: &MediaOverlayDurationMeta,
    declaration_index_by_ordinal: &HashMap<u32, usize>,
) -> MediaOverlayDurationMetadata {
    let mut declaration_indexes = duration
        .targets()
        .iter()
        .filter_map(|ordinal| declaration_index_by_ordinal.get(&ordinal.as_u32()).copied())
        .map(|index| index as u32)
        .collect::<Vec<_>>();
    declaration_indexes.sort_unstable();
    declaration_indexes.dedup();
    MediaOverlayDurationMetadata {
        authored: duration.duration().authored().to_owned(),
        clock: project_media_time(duration.duration().parsed()),
        refines: duration.refines().map(str::to_owned),
        declaration_indexes,
    }
}

fn project_media_time(value: Option<MediaTime>) -> SmilClock {
    match value {
        Some(time) => SmilClock::Parsed {
            milliseconds: time.milliseconds().to_string(),
        },
        None => SmilClock::Unrecognized {},
    }
}

fn project_smil_time(value: Option<&SmilTime>) -> SmilClock {
    match value {
        None => SmilClock::Absent {},
        Some(time) => project_media_time(time.parsed()),
    }
}

#[derive(Default)]
pub(crate) struct CoverageBuilder {
    package_relationships: Option<MediaOverlayCoverageResult>,
    smil_extraction: Vec<MediaOverlayPathCoverage>,
    smil_relationships: Vec<MediaOverlayPathCoverage>,
    fragment_observations: Vec<MediaOverlayPathCoverage>,
    media_inspection: Vec<MediaOverlayPathCoverage>,
}

impl CoverageBuilder {
    fn note_smil(&mut self, path: &str, result: MediaOverlayCoverageResult) {
        push_unique(&mut self.smil_extraction, path, result.clone());
        push_unique(&mut self.smil_relationships, path, result);
    }

    fn note_fragment(&mut self, path: &str, result: MediaOverlayCoverageResult) {
        push_unique(&mut self.fragment_observations, path, result);
    }

    fn note_media(&mut self, path: &str, result: MediaOverlayCoverageResult) {
        push_unique(&mut self.media_inspection, path, result);
    }

    fn finish(mut self) -> MediaOverlayCoverage {
        self.package_relationships = Some(MediaOverlayCoverageResult::Complete {});
        let state = aggregate_state(&self);
        MediaOverlayCoverage {
            state,
            package_relationships: self.package_relationships,
            smil_extraction: self.smil_extraction,
            smil_relationships: self.smil_relationships,
            fragment_observations: self.fragment_observations,
            media_inspection: self.media_inspection,
        }
    }

    fn finish_inapplicable(self) -> MediaOverlayCoverage {
        MediaOverlayCoverage {
            state: MediaOverlayCoverageState::Inapplicable,
            package_relationships: None,
            smil_extraction: Vec::new(),
            smil_relationships: Vec::new(),
            fragment_observations: Vec::new(),
            media_inspection: Vec::new(),
        }
    }

    fn finish_representation_limit(self) -> MediaOverlayCoverage {
        MediaOverlayCoverage {
            state: MediaOverlayCoverageState::Unavailable,
            package_relationships: Some(MediaOverlayCoverageResult::Unavailable {
                issue: MediaOverlayAnalysisIssue::ResourceLimit,
            }),
            smil_extraction: Vec::new(),
            smil_relationships: Vec::new(),
            fragment_observations: Vec::new(),
            media_inspection: Vec::new(),
        }
    }
}

fn push_unique(
    entries: &mut Vec<MediaOverlayPathCoverage>,
    path: &str,
    result: MediaOverlayCoverageResult,
) {
    if entries.iter().any(|entry| entry.canonical_path == path) {
        return;
    }
    entries.push(MediaOverlayPathCoverage {
        canonical_path: path.to_owned(),
        result,
    });
}

fn aggregate_state(builder: &CoverageBuilder) -> MediaOverlayCoverageState {
    let results = builder
        .smil_extraction
        .iter()
        .chain(&builder.smil_relationships)
        .chain(&builder.fragment_observations)
        .chain(&builder.media_inspection)
        .map(|entry| &entry.result)
        .chain(builder.package_relationships.iter());
    let mut complete = 0usize;
    let mut unavailable = 0usize;
    let mut total = 0usize;
    for result in results {
        total += 1;
        match result {
            MediaOverlayCoverageResult::Complete {} => complete += 1,
            MediaOverlayCoverageResult::Partial { .. } => {}
            MediaOverlayCoverageResult::Unavailable { .. } => unavailable += 1,
        }
    }
    if total == 0 {
        MediaOverlayCoverageState::Inapplicable
    } else if complete == total {
        MediaOverlayCoverageState::Complete
    } else if unavailable == total {
        MediaOverlayCoverageState::Unavailable
    } else {
        MediaOverlayCoverageState::Partial
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_overlay_state(
    analysis: &PublicationAnalysis,
    adapters: &HashMap<&str, DocumentAdapter>,
    association: &MediaOverlayAssociationRef<'_>,
    declaration_index_by_ordinal: &HashMap<u32, usize>,
    package_declarations: &[MediaOverlayDeclaration],
    timeline_index_by_resource: &mut HashMap<u32, u32>,
    timelines: &mut Vec<MediaOverlayTimeline>,
    coverage: &mut CoverageBuilder,
    representation_limit_exceeded: &mut bool,
) -> OverlayAssociationState {
    let reference = association.reference();
    let authored_idref = reference.declared().as_str().to_owned();
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

    match reference.target() {
        ManifestTarget::InvalidManifestIdref | ManifestTarget::Missing => {
            OverlayAssociationState::MissingDeclaration { authored_idref }
        }
        ManifestTarget::Ambiguous { candidates } => OverlayAssociationState::AmbiguousDeclaration {
            authored_idref,
            candidates: candidates.iter().copied().map(declaration_for).collect(),
        },
        ManifestTarget::Declaration {
            declaration,
            resource,
        } => {
            let declaration_projection = declaration_for(*declaration);
            let Some(resource_ordinal) = resource else {
                return OverlayAssociationState::DeclarationWithoutResource {
                    declaration: declaration_projection,
                };
            };
            if timelines.len() >= MAX_MEDIA_OVERLAY_TIMELINES
                && !timeline_index_by_resource.contains_key(&resource_ordinal.as_u32())
            {
                *representation_limit_exceeded = true;
            }
            let overlay_index = *timeline_index_by_resource
                .entry(resource_ordinal.as_u32())
                .or_insert_with(|| {
                    let index = timelines.len() as u32;
                    timelines.push(build_timeline(
                        analysis,
                        adapters,
                        association,
                        index,
                        declaration_index_by_ordinal,
                        package_declarations,
                        coverage,
                    ));
                    index
                });
            let timeline = &timelines[overlay_index as usize];
            if matches!(
                timeline.availability,
                MediaOverlayTimelineAvailability::Unavailable {
                    reason: MediaOverlayTimelineIssue::AnalysisUnavailable
                }
            ) {
                let issue = timeline_analysis_issue(analysis, *resource_ordinal);
                OverlayAssociationState::AnalysisUnavailable {
                    declaration: declaration_projection,
                    overlay_index,
                    issue,
                }
            } else {
                OverlayAssociationState::Resolved {
                    declaration: declaration_projection,
                    overlay_index,
                }
            }
        }
    }
}

fn timeline_analysis_issue(
    analysis: &PublicationAnalysis,
    resource: epub_stack::resource::ResourceOrdinal,
) -> MediaOverlayAnalysisIssue {
    match analysis.resource(resource) {
        Some(facts) => match facts.content() {
            AnalysisOutcome::Unavailable(issue) => project_analysis_issue(*issue),
            AnalysisOutcome::Partial { issue, .. } => project_analysis_issue(*issue),
            AnalysisOutcome::Complete(_) | AnalysisOutcome::NotApplicable => {
                MediaOverlayAnalysisIssue::Malformed
            }
        },
        None => MediaOverlayAnalysisIssue::Malformed,
    }
}

fn project_analysis_issue(issue: AnalysisIssue) -> MediaOverlayAnalysisIssue {
    match issue {
        AnalysisIssue::Limit(AnalysisLimit::AnalyzedResources) => {
            MediaOverlayAnalysisIssue::ResourceLimit
        }
        AnalysisIssue::Limit(AnalysisLimit::ResourceAnalysisBytes) => {
            MediaOverlayAnalysisIssue::PerResourceAnalysisLimit
        }
        AnalysisIssue::Limit(AnalysisLimit::TotalAnalysisBytes) => {
            MediaOverlayAnalysisIssue::TotalAnalysisLimit
        }
        AnalysisIssue::Limit(AnalysisLimit::TotalFingerprintBytes) => {
            MediaOverlayAnalysisIssue::TotalFingerprintLimit
        }
        AnalysisIssue::Limit(AnalysisLimit::SmilNodes) => MediaOverlayAnalysisIssue::SmilNodeLimit,
        AnalysisIssue::Limit(AnalysisLimit::SmilNesting) => {
            MediaOverlayAnalysisIssue::SmilNestingLimit
        }
        AnalysisIssue::Missing => MediaOverlayAnalysisIssue::Missing,
        AnalysisIssue::Unreadable => MediaOverlayAnalysisIssue::Unreadable,
        AnalysisIssue::Unsupported => MediaOverlayAnalysisIssue::Unsupported,
        AnalysisIssue::Malformed => MediaOverlayAnalysisIssue::Malformed,
    }
}

mod binding;
mod timeline;

pub(crate) use binding::{
    apply_audio_backing_failures, audio_resources_to_back, build_content_target,
    entry_segment_indexes_for, project_audio_binding, project_reference, project_text_binding,
};
pub(crate) use timeline::build_timeline;

#[cfg(test)]
pub(crate) mod tests;
