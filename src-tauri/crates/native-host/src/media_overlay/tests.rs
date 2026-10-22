use super::validate::{
    available_package_fields, validate_association, validate_binding, validate_clock,
    validate_coverage, validate_declaration, validate_media_overlay_capability,
    validate_media_overlay_timeline_result, validate_metadata, validate_reference,
    validate_segment, validate_segment_coverage, validate_serialized_size,
    validate_timeline_summary,
};
use super::*;
use serde_json::{Value, json};

fn parsed(milliseconds: &str) -> SmilClock {
    SmilClock::Parsed {
        milliseconds: milliseconds.to_owned(),
    }
}

fn declaration() -> MediaOverlayDeclaration {
    MediaOverlayDeclaration {
        declaration_index: 0,
        authored_id: MediaOverlayManifestId::Valid {
            value: "overlay".to_owned(),
        },
        authored_href: Some("overlays/chapter.smil".to_owned()),
        canonical_path: Some("EPUB/overlays/chapter.smil".to_owned()),
        media_type: Some("application/smil+xml".to_owned()),
    }
}

fn content_declaration() -> MediaOverlayDeclaration {
    MediaOverlayDeclaration {
        declaration_index: 1,
        authored_id: MediaOverlayManifestId::Valid {
            value: "chapter".to_owned(),
        },
        authored_href: Some("chapter.xhtml".to_owned()),
        canonical_path: Some("EPUB/chapter.xhtml".to_owned()),
        media_type: Some("application/xhtml+xml".to_owned()),
    }
}

fn complete_coverage() -> MediaOverlayCoverage {
    MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Complete,
        package_relationships: Some(MediaOverlayCoverageResult::Complete {}),
        smil_extraction: vec![MediaOverlayPathCoverage {
            canonical_path: "EPUB/overlays/chapter.smil".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        }],
        smil_relationships: vec![MediaOverlayPathCoverage {
            canonical_path: "EPUB/overlays/chapter.smil".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        }],
        fragment_observations: vec![MediaOverlayPathCoverage {
            canonical_path: "EPUB/chapter.xhtml".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        }],
        media_inspection: vec![MediaOverlayPathCoverage {
            canonical_path: "EPUB/audio/chapter.mp3".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        }],
    }
}

fn add_smil_coverage(coverage: &mut MediaOverlayCoverage, path: &str) {
    let entry = MediaOverlayPathCoverage {
        canonical_path: path.to_owned(),
        result: MediaOverlayCoverageResult::Complete {},
    };
    coverage.smil_extraction.push(entry.clone());
    coverage.smil_relationships.push(entry);
}

fn available_package_mut(
    description: &mut MediaOverlayDescription,
) -> (
    &mut bool,
    &mut Vec<MediaOverlayDeclaration>,
    &mut MediaOverlayPackageMetadata,
) {
    match &mut description.package {
        MediaOverlayPackageFacts::Available {
            declared,
            declarations,
            metadata,
        } => (declared, declarations, metadata),
        MediaOverlayPackageFacts::Unavailable { .. } => {
            panic!("test description must have available package facts")
        }
    }
}

fn reference(path: &str, fragment: &str) -> MediaOverlayReference {
    MediaOverlayReference::LocalFragment {
        authored_href: format!("{path}#{fragment}"),
        canonical_path: path.to_owned(),
        query: None,
        fragment: fragment.to_owned(),
        fragment_exists: FragmentExistence::Exists {},
    }
}

fn segment() -> MediaOverlaySegment {
    MediaOverlaySegment {
        segment_index: 0,
        semantic_path: vec![MediaOverlaySemanticNode {
            kind: MediaOverlaySemanticNodeKind::Parallel,
            epub_types: vec!["sentence".to_owned()],
            text_reference: None,
        }],
        text: MediaOverlayBinding {
            reference: reference("EPUB/chapter.xhtml", "sentence-1"),
            availability: MediaOverlayBindingAvailability::Available {},
        },
        audio: MediaOverlayBinding {
            reference: MediaOverlayReference::LocalResource {
                authored_href: "../audio/chapter.mp3".to_owned(),
                canonical_path: "EPUB/audio/chapter.mp3".to_owned(),
                query: None,
            },
            availability: MediaOverlayBindingAvailability::Available {},
        },
        clip_begin: parsed("0"),
        clip_end: parsed("1250"),
        playability: MediaOverlaySegmentPlayability::Playable {},
    }
}

fn summary() -> MediaOverlayTimelineSummary {
    MediaOverlayTimelineSummary {
        overlay_index: 0,
        canonical_path: "EPUB/overlays/chapter.smil".to_owned(),
        declarations: vec![declaration()],
        skippable: vec!["pagebreak".to_owned()],
        escapable: vec!["sidebar".to_owned()],
        segment_count: 1,
        playable_segment_count: 1,
        availability: MediaOverlayTimelineAvailability::Available {},
    }
}

fn timeline() -> MediaOverlayTimeline {
    let summary = summary();
    MediaOverlayTimeline {
        overlay_index: summary.overlay_index,
        canonical_path: summary.canonical_path,
        declarations: summary.declarations,
        skippable: summary.skippable,
        escapable: summary.escapable,
        segment_count: summary.segment_count,
        playable_segment_count: summary.playable_segment_count,
        availability: summary.availability,
        segments: vec![segment()],
    }
}

fn description() -> MediaOverlayDescription {
    MediaOverlayDescription {
        package: MediaOverlayPackageFacts::Available {
            declared: true,
            declarations: vec![declaration(), content_declaration()],
            metadata: MediaOverlayPackageMetadata {
                total_durations: vec![MediaOverlayDurationMetadata {
                    authored: "1.25s".to_owned(),
                    clock: parsed("1250"),
                    refines: None,
                    declaration_indexes: vec![],
                }],
                item_durations: vec![],
                narrators: vec!["Narrator".to_owned()],
                active_classes: vec!["active".to_owned()],
                playback_active_classes: vec!["playing".to_owned()],
            },
        },
        coverage: complete_coverage(),
        playback: MediaOverlayPlayback::Available {
            overlays: vec![summary()],
            associations: vec![MediaOverlayAssociation {
                occurrence_index: 0,
                effective_linear: EffectiveLinear::Yes,
                content: ReadingOrderContentTarget::Resolved {
                    declaration: content_declaration(),
                    canonical_path: "EPUB/chapter.xhtml".to_owned(),
                    bytes: MediaOverlayLocalBytes::Present {},
                },
                overlay: OverlayAssociationState::Resolved {
                    declaration: declaration(),
                    overlay_index: 0,
                },
                entry_segment_indexes: vec![0],
            }],
        },
    }
}

#[test]
fn representative_description_and_timeline_json_are_strict_and_valid() {
    let description = description();
    validate_media_overlay_description(&description).unwrap();
    let json = serde_json::to_value(&description).unwrap();
    assert_eq!(json["playback"]["state"], "available");
    assert_eq!(json["package"]["state"], "available");
    assert_eq!(json["coverage"]["state"], "complete");
    assert_eq!(
        json["coverage"]["packageRelationships"]["state"],
        "complete"
    );
    assert_eq!(
        json["coverage"]["smilExtraction"][0]["canonicalPath"],
        "EPUB/overlays/chapter.smil"
    );
    assert_eq!(json["package"]["declarations"][0]["declarationIndex"], 0);
    assert_eq!(
        json["playback"]["overlays"][0]["declarations"][0],
        json["package"]["declarations"][0]
    );
    assert!(
        json["playback"]["overlays"][0]
            .get("declarationIndexes")
            .is_none()
    );
    assert_eq!(
        json["playback"]["associations"][0]["effectiveLinear"],
        "yes"
    );
    assert_eq!(
        json["package"]["metadata"]["totalDurations"][0]["clock"],
        json!({
            "state": "parsed", "milliseconds": "1250"
        })
    );

    let result = MediaOverlayTimelineResult::Available {
        timeline: timeline(),
    };
    validate_media_overlay_timeline_result(&result, &description).unwrap();
    let result_json = serde_json::to_value(result).unwrap();
    assert!(result_json["timeline"].get("summary").is_none());
    assert_eq!(result_json["timeline"]["overlayIndex"], 0);
    assert!(result_json["timeline"]["declarations"].is_array());
    assert!(result_json["timeline"]["segments"].is_array());
    let round_trip: MediaOverlayTimelineResult = serde_json::from_value(result_json).unwrap();
    assert!(matches!(
        round_trip,
        MediaOverlayTimelineResult::Available { .. }
    ));
}

#[test]
fn rejects_unknown_struct_and_enum_fields() {
    let mut value = serde_json::to_value(description()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("extra".to_owned(), json!(true));
    assert!(serde_json::from_value::<MediaOverlayDescription>(value).is_err());

    assert!(
        serde_json::from_value::<SmilClock>(json!({
            "state": "absent", "milliseconds": "0"
        }))
        .is_err()
    );
    let mut association = serde_json::to_value(OverlayAssociationState::Resolved {
        declaration: declaration(),
        overlay_index: 0,
    })
    .unwrap();
    association
        .as_object_mut()
        .unwrap()
        .insert("extra".to_owned(), json!(1));
    assert!(serde_json::from_value::<OverlayAssociationState>(association).is_err());
    assert!(
        serde_json::from_value::<MediaOverlayCoverageResult>(json!({
            "state": "complete", "issue": "missing"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<MediaOverlayPackageFacts>(json!({
            "state": "unavailable",
            "reason": "representation-limit-exceeded",
            "extra": true
        }))
        .is_err()
    );
}

#[test]
fn unavailable_package_has_exact_wire_shape_and_pairing() {
    let package = MediaOverlayPackageFacts::Unavailable {
        reason: MediaOverlayPackageIssue::RepresentationLimitExceeded,
    };
    let json = json!({
        "state": "unavailable",
        "reason": "representation-limit-exceeded"
    });
    assert_eq!(serde_json::to_value(&package).unwrap(), json);
    assert_eq!(
        serde_json::from_value::<MediaOverlayPackageFacts>(json).unwrap(),
        package
    );

    let coverage = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Complete,
        package_relationships: Some(MediaOverlayCoverageResult::Complete {}),
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    let compact = MediaOverlayDescription {
        package: package.clone(),
        coverage: coverage.clone(),
        playback: MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
        },
    };
    validate_media_overlay_description(&compact).unwrap();

    let mut wrong_reason = compact.clone();
    wrong_reason.playback = MediaOverlayPlayback::Unavailable {
        reason: MediaOverlayAvailabilityIssue::AnalysisUnavailable,
    };
    assert!(validate_media_overlay_description(&wrong_reason).is_err());

    let mut inapplicable = compact;
    inapplicable.coverage = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Inapplicable,
        package_relationships: None,
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    assert!(validate_media_overlay_description(&inapplicable).is_err());

    let mut retained_package = description();
    retained_package.playback = MediaOverlayPlayback::Unavailable {
        reason: MediaOverlayAvailabilityIssue::RepresentationLimitExceeded,
    };
    validate_media_overlay_description(&retained_package).unwrap();
}

#[test]
fn preserves_absent_unrecognized_and_parsed_clock_states() {
    assert_ne!(SmilClock::Absent {}, SmilClock::Unrecognized {});
    assert_ne!(SmilClock::Unrecognized {}, parsed("0"));
    for (value, state) in [
        (SmilClock::Absent {}, "absent"),
        (SmilClock::Unrecognized {}, "unrecognized"),
        (parsed("0"), "parsed"),
    ] {
        assert_eq!(serde_json::to_value(value).unwrap()["state"], state);
    }
}

#[test]
fn rejects_noncanonical_and_oversized_clocks() {
    for value in ["", "00", "01", "+1", "1.0", "18446744073709551616"] {
        assert!(validate_clock(&parsed(value)).is_err(), "accepted {value}");
    }
    assert!(validate_clock(&parsed("18446744073709551615")).is_ok());
}

#[test]
fn rejects_noncanonical_paths_and_separates_local_reference_states() {
    for path in ["/EPUB/chapter.xhtml", "EPUB/../chapter.xhtml", "EPUB//a"] {
        let mut value = description();
        available_package_mut(&mut value).1[0].canonical_path = Some(path.to_owned());
        assert!(validate_media_overlay_description(&value).is_err());
    }
    assert_ne!(
        MediaOverlayReference::LocalResource {
            authored_href: "chapter.xhtml".to_owned(),
            canonical_path: "EPUB/chapter.xhtml".to_owned(),
            query: None,
        },
        reference("EPUB/chapter.xhtml", "one")
    );
}

#[test]
fn rejects_noncontiguous_indexes_and_bad_cross_references() {
    let mut value = description();
    available_package_mut(&mut value).1[0].declaration_index = 1;
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut value.playback {
        overlays[0].overlay_index = 1;
    }
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        associations[0].overlay = OverlayAssociationState::Resolved {
            declaration: content_declaration(),
            overlay_index: 0,
        };
    }
    assert!(validate_media_overlay_description(&value).is_err());
}

#[test]
fn rejects_unsorted_duplicate_and_out_of_range_entry_indexes() {
    for indexes in [vec![1], vec![0, 0], vec![1, 0]] {
        let mut value = description();
        if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
            associations[0].entry_segment_indexes = indexes;
        }
        assert!(validate_media_overlay_description(&value).is_err());
    }
}

#[test]
fn rejects_count_mismatches_and_zero_playable_available_description() {
    let mut value = description();
    available_package_mut(&mut value).1[1].declaration_index = 0;
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut value.playback {
        overlays[0].playable_segment_count = 0;
        overlays[0].availability = MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::NoPlayableSegments,
        };
    }
    assert!(validate_media_overlay_description(&value).is_err());
}

#[test]
fn zero_playable_derived_result_is_unavailable() {
    let mut value = description();
    value.playback = MediaOverlayPlayback::Unavailable {
        reason: MediaOverlayAvailabilityIssue::NoPlayableSegments,
    };
    assert!(validate_media_overlay_description(&value).is_ok());
    assert_eq!(
        serde_json::to_value(value).unwrap()["playback"],
        json!({"state": "unavailable", "reason": "no-playable-segments"})
    );
}

#[test]
fn rejects_unordered_or_unusable_playable_segments() {
    let mut reversed = segment();
    reversed.clip_begin = parsed("1250");
    reversed.clip_end = parsed("1250");
    assert!(validate_segment(&reversed).is_err());

    let mut missing_fragment = segment();
    if let MediaOverlayReference::LocalFragment {
        fragment_exists, ..
    } = &mut missing_fragment.text.reference
    {
        *fragment_exists = FragmentExistence::Missing {};
    }
    assert!(validate_segment(&missing_fragment).is_err());

    let mut ambiguous_fragment = segment();
    if let MediaOverlayReference::LocalFragment {
        fragment_exists, ..
    } = &mut ambiguous_fragment.text.reference
    {
        *fragment_exists = FragmentExistence::Ambiguous {};
    }
    assert!(validate_segment(&ambiguous_fragment).is_err());
    ambiguous_fragment.text.availability = MediaOverlayBindingAvailability::Unavailable {
        reason: MediaOverlayBindingIssue::UnsupportedTarget,
    };
    ambiguous_fragment.playability = MediaOverlaySegmentPlayability::Unavailable {
        reason: MediaOverlaySegmentUnavailableReason::TextBindingUnavailable,
    };
    assert!(validate_segment(&ambiguous_fragment).is_err());
    ambiguous_fragment.text.availability = MediaOverlayBindingAvailability::Unavailable {
        reason: MediaOverlayBindingIssue::AmbiguousFragment,
    };
    assert!(validate_segment(&ambiguous_fragment).is_ok());
}

#[test]
fn unavailable_timeline_has_no_segment_substitute_and_matching_reason() {
    let mut description = description();
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut description.playback {
        overlays[0].playable_segment_count = 0;
        overlays[0].availability = MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::NoPlayableSegments,
        };
    }
    if let MediaOverlayPlayback::Available { associations, .. } = &mut description.playback {
        associations[0].overlay = OverlayAssociationState::Resolved {
            declaration: declaration(),
            overlay_index: 0,
        };
        associations[0].entry_segment_indexes.clear();
    }
    // Keep the overall description available by adding one independently playable summary.
    let mut second = summary();
    second.overlay_index = 1;
    second.canonical_path = "EPUB/overlays/second.smil".to_owned();
    let mut second_declaration = declaration();
    second_declaration.declaration_index = 2;
    second_declaration.canonical_path = Some(second.canonical_path.clone());
    second.declarations = vec![second_declaration.clone()];
    available_package_mut(&mut description)
        .1
        .push(second_declaration);
    add_smil_coverage(&mut description.coverage, &second.canonical_path);
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut description.playback {
        overlays.push(second);
    }
    let result = MediaOverlayTimelineResult::Unavailable {
        summary: match &description.playback {
            MediaOverlayPlayback::Available { overlays, .. } => overlays[0].clone(),
            _ => unreachable!(),
        },
        reason: MediaOverlayTimelineIssue::NoPlayableSegments,
    };
    validate_media_overlay_timeline_result(&result, &description).unwrap();
    assert!(
        serde_json::to_value(result)
            .unwrap()
            .get("segments")
            .is_none()
    );
}

#[test]
fn enforces_string_collection_ambiguity_and_coverage_bounds() {
    let mut value = description();
    available_package_mut(&mut value).2.narrators[0] =
        "x".repeat(MAX_MEDIA_OVERLAY_STRING_BYTES + 1);
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    available_package_mut(&mut value).2.active_classes =
        vec!["x".to_owned(); MAX_MEDIA_OVERLAY_COLLECTION_ITEMS + 1];
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    value.coverage.smil_extraction = vec![
        MediaOverlayPathCoverage {
            canonical_path: "EPUB/overlays/chapter.smil".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        };
        MAX_MEDIA_OVERLAY_COVERAGE_ISSUES + 1
    ];
    assert!(validate_media_overlay_description(&value).is_err());

    let mut value = description();
    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        associations[0].content = ReadingOrderContentTarget::AmbiguousDeclaration {
            authored_idref: "duplicate".to_owned(),
            candidates: vec![declaration(); MAX_MEDIA_OVERLAY_AMBIGUOUS_CANDIDATES + 1],
        };
    }
    assert!(validate_media_overlay_description(&value).is_err());
}

#[test]
fn serialized_size_bound_is_explicitly_enforced() {
    assert!(validate_serialized_size(&json!({"value": "1234"}), 4, "test").is_err());
}

#[test]
fn capability_uses_status_discriminator() {
    let value = serde_json::to_value(MediaOverlayCapability::Available {
        playable_occurrence_indexes: vec![0, 2],
        unavailable_occurrence_count: 1,
    })
    .unwrap();
    assert_eq!(value["status"], "available");
    assert!(value.get("state").is_none());
    assert!(
        serde_json::from_value::<MediaOverlayCapability>(json!({
            "state": "absent"
        }))
        .is_err()
    );
}

#[test]
fn every_reference_variant_round_trips_without_flattening() {
    let values = vec![
        MediaOverlayReference::Absent {},
        MediaOverlayReference::MissingLocalResource {
            authored_href: "missing.mp3".to_owned(),
            canonical_path: "EPUB/missing.mp3".to_owned(),
        },
        MediaOverlayReference::IndexedLocalResourceUnavailable {
            authored_href: "empty.mp3".to_owned(),
            canonical_path: "EPUB/empty.mp3".to_owned(),
            query: None,
        },
        MediaOverlayReference::Remote {
            authored_href: "https://example.test/audio.mp3".to_owned(),
        },
        MediaOverlayReference::Data {
            authored_href: "data:audio/mpeg;base64,AA==".to_owned(),
        },
        MediaOverlayReference::External {
            authored_href: "urn:audio:one".to_owned(),
        },
        MediaOverlayReference::Invalid {
            authored_href: "%".to_owned(),
        },
    ];
    for value in values {
        let json: Value = serde_json::to_value(&value).unwrap();
        assert_eq!(
            serde_json::from_value::<MediaOverlayReference>(json).unwrap(),
            value
        );
    }
}

#[test]
fn standalone_smil_needs_no_relationship_declaration() {
    let mut value = description();
    available_package_mut(&mut value).1.clear();
    if let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &mut value.playback
    {
        overlays[0].declarations.clear();
        associations.clear();
    }
    validate_media_overlay_description(&value).unwrap();
    assert!(available_package_fields(&value.package).unwrap().declared);
}

#[test]
fn unavailable_diagnostic_playable_count_does_not_make_description_available() {
    let mut value = description();
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut value.playback {
        overlays[0].availability = MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::AudioPreparationFailed,
        };
        assert_eq!(overlays[0].playable_segment_count, 1);
    }
    assert!(validate_media_overlay_description(&value).is_err());
}

#[test]
fn resolved_and_analysis_unavailable_associations_remain_distinct() {
    let value = description();
    let package = value.package;
    let mut coverage = value.coverage;
    let mut overlay = summary();
    overlay.availability = MediaOverlayTimelineAvailability::Unavailable {
        reason: MediaOverlayTimelineIssue::AudioPreparationFailed,
    };
    let mut association = MediaOverlayAssociation {
        occurrence_index: 0,
        effective_linear: EffectiveLinear::Yes,
        content: ReadingOrderContentTarget::Resolved {
            declaration: content_declaration(),
            canonical_path: "EPUB/chapter.xhtml".to_owned(),
            bytes: MediaOverlayLocalBytes::Present {},
        },
        overlay: OverlayAssociationState::Resolved {
            declaration: declaration(),
            overlay_index: 0,
        },
        entry_segment_indexes: vec![0],
    };
    validate_association(&association, &[overlay.clone()], &package, &coverage).unwrap();

    association.overlay = OverlayAssociationState::AnalysisUnavailable {
        declaration: declaration(),
        overlay_index: 0,
        issue: MediaOverlayAnalysisIssue::Unreadable,
    };
    assert!(validate_association(&association, &[overlay.clone()], &package, &coverage).is_err());
    overlay.availability = MediaOverlayTimelineAvailability::Unavailable {
        reason: MediaOverlayTimelineIssue::AnalysisUnavailable,
    };
    overlay.segment_count = 0;
    overlay.playable_segment_count = 0;
    overlay.skippable.clear();
    overlay.escapable.clear();
    association.entry_segment_indexes.clear();
    coverage.state = MediaOverlayCoverageState::Partial;
    coverage.smil_extraction[0].result = MediaOverlayCoverageResult::Unavailable {
        issue: MediaOverlayAnalysisIssue::Unreadable,
    };
    validate_association(&association, &[overlay.clone()], &package, &coverage).unwrap();

    association.overlay = OverlayAssociationState::Resolved {
        declaration: declaration(),
        overlay_index: 0,
    };
    assert!(validate_association(&association, &[overlay], &package, &coverage).is_err());
}

#[test]
fn timeline_entry_segments_must_target_each_duplicate_occurrence_content() {
    let mut value = description();
    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        let mut duplicate = associations[0].clone();
        duplicate.occurrence_index = 1;
        associations.push(duplicate);
    }
    let result = MediaOverlayTimelineResult::Available {
        timeline: timeline(),
    };
    validate_media_overlay_timeline_result(&result, &value).unwrap();

    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        let mut other = content_declaration();
        other.declaration_index = 2;
        other.authored_id = MediaOverlayManifestId::Valid {
            value: "other".to_owned(),
        };
        other.authored_href = Some("other.xhtml".to_owned());
        other.canonical_path = Some("EPUB/other.xhtml".to_owned());
        associations[1].content = ReadingOrderContentTarget::Resolved {
            declaration: other.clone(),
            canonical_path: "EPUB/other.xhtml".to_owned(),
            bytes: MediaOverlayLocalBytes::Present {},
        };
        available_package_mut(&mut value).1.push(other);
    }
    assert!(validate_media_overlay_timeline_result(&result, &value).is_err());
}

#[test]
fn rejects_coverage_state_and_description_contradictions() {
    let mut complete = description();
    complete.coverage.smil_extraction[0].result = MediaOverlayCoverageResult::Unavailable {
        issue: MediaOverlayAnalysisIssue::Unreadable,
    };
    assert!(validate_media_overlay_description(&complete).is_err());

    let mut inapplicable = description();
    *available_package_mut(&mut inapplicable).0 = false;
    available_package_mut(&mut inapplicable).1.clear();
    inapplicable.playback = MediaOverlayPlayback::Absent {};
    inapplicable.coverage.state = MediaOverlayCoverageState::Inapplicable;
    assert!(validate_media_overlay_description(&inapplicable).is_err());
}

#[test]
fn representation_limit_summary_preserves_overlimit_diagnostic_count() {
    let package = description().package;
    let mut overlimit = summary();
    overlimit.segment_count = MAX_MEDIA_OVERLAY_SEGMENTS as u32 + 1;
    overlimit.availability = MediaOverlayTimelineAvailability::Unavailable {
        reason: MediaOverlayTimelineIssue::RepresentationLimitExceeded,
    };
    validate_timeline_summary(&overlimit, &package).unwrap();

    overlimit.availability = MediaOverlayTimelineAvailability::Available {};
    assert!(validate_timeline_summary(&overlimit, &package).is_err());
}

#[test]
fn empty_decoded_local_fragment_is_retained() {
    let mut value = segment();
    if let MediaOverlayReference::LocalFragment {
        authored_href,
        fragment,
        ..
    } = &mut value.text.reference
    {
        *authored_href = "EPUB/chapter.xhtml#".to_owned();
        fragment.clear();
    }
    validate_segment(&value).unwrap();
}

#[test]
fn rejects_contradictory_binding_and_segment_classification() {
    let contradictory = MediaOverlayBinding {
        reference: MediaOverlayReference::Absent {},
        availability: MediaOverlayBindingAvailability::Unavailable {
            reason: MediaOverlayBindingIssue::AudioBackingUnavailable,
        },
    };
    assert!(validate_binding(&contradictory).is_err());

    let mut value = segment();
    value.playability = MediaOverlaySegmentPlayability::Unavailable {
        reason: MediaOverlaySegmentUnavailableReason::AudioBindingUnavailable,
    };
    assert!(validate_segment(&value).is_err());
}

#[test]
fn capability_is_derived_exactly_from_available_associations() {
    let value = description();
    let timelines = [MediaOverlayTimelineResult::Available {
        timeline: timeline(),
    }];
    let capability = MediaOverlayCapability::Available {
        playable_occurrence_indexes: vec![0],
        unavailable_occurrence_count: 0,
    };
    validate_media_overlay_capability(&capability, &value, &timelines).unwrap();

    let duplicate = MediaOverlayCapability::Available {
        playable_occurrence_indexes: vec![0, 0],
        unavailable_occurrence_count: 0,
    };
    assert!(validate_media_overlay_capability(&duplicate, &value, &timelines).is_err());

    let wrong_count = MediaOverlayCapability::Available {
        playable_occurrence_indexes: vec![0],
        unavailable_occurrence_count: 1,
    };
    assert!(validate_media_overlay_capability(&wrong_count, &value, &timelines).is_err());

    let out_of_range = MediaOverlayCapability::Available {
        playable_occurrence_indexes: vec![1],
        unavailable_occurrence_count: 0,
    };
    assert!(validate_media_overlay_capability(&out_of_range, &value, &timelines).is_err());
}

#[test]
fn aggregate_coverage_states_follow_applicable_results() {
    let inapplicable = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Inapplicable,
        package_relationships: None,
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    validate_coverage(&inapplicable).unwrap();

    let complete = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Complete,
        package_relationships: Some(MediaOverlayCoverageResult::Complete {}),
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    validate_coverage(&complete).unwrap();

    let partial = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Partial,
        package_relationships: Some(MediaOverlayCoverageResult::Partial {
            issue: MediaOverlayAnalysisIssue::ResourceLimit,
        }),
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    validate_coverage(&partial).unwrap();

    let unavailable = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Unavailable,
        package_relationships: Some(MediaOverlayCoverageResult::Unavailable {
            issue: MediaOverlayAnalysisIssue::Unreadable,
        }),
        smil_extraction: vec![MediaOverlayPathCoverage {
            canonical_path: "EPUB/overlays/chapter.smil".to_owned(),
            result: MediaOverlayCoverageResult::Unavailable {
                issue: MediaOverlayAnalysisIssue::Malformed,
            },
        }],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    validate_coverage(&unavailable).unwrap();

    let mut incoherent = unavailable;
    incoherent.state = MediaOverlayCoverageState::Partial;
    assert!(validate_coverage(&incoherent).is_err());

    let mut duplicate = complete_coverage();
    duplicate
        .smil_extraction
        .push(duplicate.smil_extraction[0].clone());
    assert!(validate_coverage(&duplicate).is_err());
}

#[test]
fn complete_coverage_allows_authored_target_failures() {
    let mut missing = description();
    if let MediaOverlayPlayback::Available { associations, .. } = &mut missing.playback {
        associations[0].content = ReadingOrderContentTarget::MissingDeclaration {
            authored_idref: "missing".to_owned(),
        };
        associations[0].overlay = OverlayAssociationState::MissingDeclaration {
            authored_idref: "missing-overlay".to_owned(),
        };
        associations[0].entry_segment_indexes.clear();
    }
    validate_media_overlay_description(&missing).unwrap();
    assert_eq!(missing.coverage.state, MediaOverlayCoverageState::Complete);

    let mut ambiguous = description();
    for declaration in available_package_mut(&mut ambiguous).1 {
        declaration.authored_id = MediaOverlayManifestId::Valid {
            value: "duplicate".to_owned(),
        };
    }
    let declarations = available_package_fields(&ambiguous.package)
        .unwrap()
        .declarations
        .to_vec();
    if let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &mut ambiguous.playback
    {
        overlays[0].declarations[0] = declarations[0].clone();
        associations[0].content = ReadingOrderContentTarget::AmbiguousDeclaration {
            authored_idref: "duplicate".to_owned(),
            candidates: declarations.clone(),
        };
        associations[0].overlay = OverlayAssociationState::AmbiguousDeclaration {
            authored_idref: "duplicate".to_owned(),
            candidates: declarations,
        };
        associations[0].entry_segment_indexes.clear();
    }
    validate_media_overlay_description(&ambiguous).unwrap();
    let json = serde_json::to_value(&ambiguous).unwrap();
    assert_eq!(
        json["playback"]["associations"][0]["content"]["candidates"][1]["declarationIndex"],
        1
    );
    if let MediaOverlayPlayback::Available { associations, .. } = &mut ambiguous.playback {
        let ReadingOrderContentTarget::AmbiguousDeclaration { candidates, .. } =
            &mut associations[0].content
        else {
            unreachable!();
        };
        candidates[1].authored_href = Some("different.xhtml".to_owned());
    }
    assert!(validate_media_overlay_description(&ambiguous).is_err());
}

#[test]
fn complete_coverage_is_independent_of_audio_preparation() {
    let mut value = description();
    if let MediaOverlayPlayback::Available { overlays, .. } = &mut value.playback {
        overlays[0].availability = MediaOverlayTimelineAvailability::Unavailable {
            reason: MediaOverlayTimelineIssue::AudioPreparationFailed,
        };
        let mut available = summary();
        available.overlay_index = 1;
        available.canonical_path = "EPUB/overlays/standalone.smil".to_owned();
        available.declarations.clear();
        overlays.push(available);
    }
    add_smil_coverage(&mut value.coverage, "EPUB/overlays/standalone.smil");
    validate_media_overlay_description(&value).unwrap();
    assert_eq!(value.coverage.state, MediaOverlayCoverageState::Complete);
}

#[test]
fn capability_requires_an_actually_playable_entry_segment() {
    let mut value = description();
    if let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &mut value.playback
    {
        overlays[0].segment_count = 2;
        associations[0].entry_segment_indexes = vec![0];
    }
    let mut full = timeline();
    full.segment_count = 2;
    full.segments[0].playability = MediaOverlaySegmentPlayability::Malformed {
        reason: MediaOverlayMalformedReason::MultipleText,
    };
    let mut playable = segment();
    playable.segment_index = 1;
    playable.text.reference = reference("EPUB/other.xhtml", "sentence-2");
    full.segments.push(playable);
    value
        .coverage
        .fragment_observations
        .push(MediaOverlayPathCoverage {
            canonical_path: "EPUB/other.xhtml".to_owned(),
            result: MediaOverlayCoverageResult::Complete {},
        });
    let timelines = [MediaOverlayTimelineResult::Available { timeline: full }];
    validate_media_overlay_capability(
        &MediaOverlayCapability::Available {
            playable_occurrence_indexes: vec![],
            unavailable_occurrence_count: 1,
        },
        &value,
        &timelines,
    )
    .unwrap();
    assert!(
        validate_media_overlay_capability(
            &MediaOverlayCapability::Available {
                playable_occurrence_indexes: vec![0],
                unavailable_occurrence_count: 0,
            },
            &value,
            &timelines,
        )
        .is_err()
    );
}

#[test]
fn text_to_speech_unavailable_requires_usable_text() {
    let mut text_only = segment();
    text_only.audio = MediaOverlayBinding {
        reference: MediaOverlayReference::Absent {},
        availability: MediaOverlayBindingAvailability::Unavailable {
            reason: MediaOverlayBindingIssue::MissingReference,
        },
    };
    text_only.playability = MediaOverlaySegmentPlayability::Unsupported {
        reason: MediaOverlayUnsupportedReason::TextToSpeechUnavailable,
    };
    validate_segment(&text_only).unwrap();

    text_only.text = MediaOverlayBinding {
        reference: MediaOverlayReference::MissingLocalResource {
            authored_href: "missing.xhtml".to_owned(),
            canonical_path: "EPUB/missing.xhtml".to_owned(),
        },
        availability: MediaOverlayBindingAvailability::Unavailable {
            reason: MediaOverlayBindingIssue::MissingResource,
        },
    };
    assert!(validate_segment(&text_only).is_err());
}

#[test]
fn duration_metadata_enforces_total_and_item_partitions() {
    let package = description().package;
    let package = available_package_fields(&package).unwrap();
    let mut metadata = package.metadata.clone();
    metadata.item_durations.push(MediaOverlayDurationMetadata {
        authored: "1.25s".to_owned(),
        clock: SmilClock::Unrecognized {},
        refines: Some("#duplicate".to_owned()),
        declaration_indexes: vec![],
    });
    validate_metadata(&metadata, package.declarations.len()).unwrap();
    metadata.item_durations[0].declaration_indexes = vec![0, 1];
    validate_metadata(&metadata, package.declarations.len()).unwrap();

    metadata.total_durations[0].refines = Some("#overlay".to_owned());
    assert!(validate_metadata(&metadata, package.declarations.len()).is_err());
    metadata.total_durations[0].refines = None;
    metadata.total_durations[0].declaration_indexes = vec![0];
    assert!(validate_metadata(&metadata, package.declarations.len()).is_err());
    metadata.total_durations[0].declaration_indexes.clear();
    metadata.total_durations[0].clock = SmilClock::Absent {};
    assert!(validate_metadata(&metadata, package.declarations.len()).is_err());
    metadata.total_durations[0].clock = parsed("1250");
    metadata.item_durations[0].refines = None;
    assert!(validate_metadata(&metadata, package.declarations.len()).is_err());
    metadata.item_durations[0].refines = Some("#duplicate".to_owned());
    metadata.item_durations[0].clock = SmilClock::Absent {};
    assert!(validate_metadata(&metadata, package.declarations.len()).is_err());
}

#[test]
fn manifest_id_and_missing_href_states_are_distinct() {
    let missing = MediaOverlayDeclaration {
        declaration_index: 0,
        authored_id: MediaOverlayManifestId::Missing {},
        authored_href: None,
        canonical_path: None,
        media_type: None,
    };
    let invalid = MediaOverlayDeclaration {
        declaration_index: 0,
        authored_id: MediaOverlayManifestId::Invalid {
            value: String::new(),
        },
        authored_href: Some(String::new()),
        canonical_path: None,
        media_type: Some(String::new()),
    };
    validate_declaration(&missing).unwrap();
    validate_declaration(&invalid).unwrap();
    let missing_json = serde_json::to_value(missing).unwrap();
    let invalid_json = serde_json::to_value(invalid).unwrap();
    assert_eq!(missing_json["authoredId"]["state"], "missing");
    assert!(missing_json["authoredHref"].is_null());
    assert_eq!(invalid_json["authoredId"]["state"], "invalid");
    assert_eq!(invalid_json["authoredHref"], "");
}

#[test]
fn resolved_content_preserves_missing_local_bytes() {
    let mut value = description();
    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        let ReadingOrderContentTarget::Resolved { bytes, .. } = &mut associations[0].content else {
            unreachable!();
        };
        *bytes = MediaOverlayLocalBytes::Missing {};
    }
    validate_media_overlay_description(&value).unwrap();
    assert_eq!(
        serde_json::to_value(&value).unwrap()["playback"]["associations"][0]["content"]["bytes"]["state"],
        "missing"
    );
    let timelines = [MediaOverlayTimelineResult::Available {
        timeline: timeline(),
    }];
    validate_media_overlay_capability(
        &MediaOverlayCapability::Available {
            playable_occurrence_indexes: vec![],
            unavailable_occurrence_count: 1,
        },
        &value,
        &timelines,
    )
    .unwrap();
    assert!(
        validate_media_overlay_capability(
            &MediaOverlayCapability::Available {
                playable_occurrence_indexes: vec![0],
                unavailable_occurrence_count: 0,
            },
            &value,
            &timelines,
        )
        .is_err()
    );
}

#[test]
fn indexed_unavailable_references_preserve_resource_and_fragment_facts() {
    let references = [
        (
            MediaOverlayReference::IndexedLocalResourceUnavailable {
                authored_href: "audio.mp3?edition=1".to_owned(),
                canonical_path: "EPUB/audio.mp3".to_owned(),
                query: Some("edition=1".to_owned()),
            },
            json!({
                "state": "indexed-local-resource-unavailable",
                "authoredHref": "audio.mp3?edition=1",
                "canonicalPath": "EPUB/audio.mp3",
                "query": "edition=1"
            }),
        ),
        (
            MediaOverlayReference::IndexedLocalFragmentUnavailable {
                authored_href: "chapter.xhtml?#".to_owned(),
                canonical_path: "EPUB/chapter.xhtml".to_owned(),
                query: Some(String::new()),
                fragment: String::new(),
                fragment_exists: FragmentExistence::Unknown {},
            },
            json!({
                "state": "indexed-local-fragment-unavailable",
                "authoredHref": "chapter.xhtml?#",
                "canonicalPath": "EPUB/chapter.xhtml",
                "query": "",
                "fragment": "",
                "fragmentExists": { "state": "unknown" }
            }),
        ),
    ];
    for (reference, expected_json) in references {
        validate_reference(&reference).unwrap();
        let json = serde_json::to_value(&reference).unwrap();
        assert_eq!(json, expected_json);
        let round_trip = serde_json::from_value::<MediaOverlayReference>(json.clone()).unwrap();
        assert_eq!(round_trip, reference);
        let binding = MediaOverlayBinding {
            reference: reference.clone(),
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::ResourceBytesUnavailable,
            },
        };
        validate_binding(&binding).unwrap();
        let contradictory = MediaOverlayBinding {
            reference,
            availability: MediaOverlayBindingAvailability::Unavailable {
                reason: MediaOverlayBindingIssue::MissingResource,
            },
        };
        assert!(validate_binding(&contradictory).is_err());
    }
}

#[test]
fn full_timeline_requires_every_entry_candidate_in_order() {
    let mut value = description();
    if let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &mut value.playback
    {
        overlays[0].segment_count = 2;
        let mut duplicate = associations[0].clone();
        duplicate.occurrence_index = 1;
        associations.push(duplicate);
    }
    let mut full = timeline();
    full.segment_count = 2;
    let mut second = segment();
    second.segment_index = 1;
    second.playability = MediaOverlaySegmentPlayability::Malformed {
        reason: MediaOverlayMalformedReason::MultipleText,
    };
    full.segments.push(second);
    let result = MediaOverlayTimelineResult::Available { timeline: full };
    assert!(validate_media_overlay_timeline_result(&result, &value).is_err());

    if let MediaOverlayPlayback::Available { associations, .. } = &mut value.playback {
        associations[0].entry_segment_indexes = vec![0, 1];
        associations[1].entry_segment_indexes = vec![0, 1];
    }
    validate_media_overlay_timeline_result(&result, &value).unwrap();
}

#[test]
fn package_presence_and_coverage_applicability_agree() {
    let mut absent = description();
    *available_package_mut(&mut absent).0 = false;
    available_package_mut(&mut absent).1.clear();
    absent.coverage = MediaOverlayCoverage {
        state: MediaOverlayCoverageState::Inapplicable,
        package_relationships: None,
        smil_extraction: vec![],
        smil_relationships: vec![],
        fragment_observations: vec![],
        media_inspection: vec![],
    };
    absent.playback = MediaOverlayPlayback::Absent {};
    validate_media_overlay_description(&absent).unwrap();

    let mut declared = description();
    declared.coverage = absent.coverage.clone();
    assert!(validate_media_overlay_description(&declared).is_err());

    absent.coverage = complete_coverage();
    assert!(validate_media_overlay_description(&absent).is_err());
}

#[test]
fn description_timelines_require_matching_smil_coverage_paths() {
    let mut missing_extraction = description();
    missing_extraction.coverage.smil_extraction.clear();
    assert!(validate_media_overlay_description(&missing_extraction).is_err());

    let mut substitute_extraction = description();
    substitute_extraction.coverage.smil_extraction[0].canonical_path =
        "EPUB/overlays/other.smil".to_owned();
    assert!(validate_media_overlay_description(&substitute_extraction).is_err());

    let mut missing_relationships = description();
    missing_relationships.coverage.smil_relationships.clear();
    assert!(validate_media_overlay_description(&missing_relationships).is_err());

    let mut substitute_relationships = description();
    substitute_relationships.coverage.smil_relationships[0].canonical_path =
        "EPUB/overlays/other.smil".to_owned();
    assert!(validate_media_overlay_description(&substitute_relationships).is_err());
}

#[test]
fn full_timeline_requires_fragment_and_media_coverage_paths() {
    let result = MediaOverlayTimelineResult::Available {
        timeline: timeline(),
    };

    let mut missing_fragment = description();
    missing_fragment.coverage.fragment_observations.clear();
    assert!(validate_media_overlay_timeline_result(&result, &missing_fragment).is_err());

    let mut substitute_fragment = description();
    substitute_fragment.coverage.fragment_observations[0].canonical_path =
        "EPUB/other.xhtml".to_owned();
    assert!(validate_media_overlay_timeline_result(&result, &substitute_fragment).is_err());

    let mut missing_inspection = description();
    missing_inspection.coverage.media_inspection.clear();
    assert!(validate_media_overlay_timeline_result(&result, &missing_inspection).is_err());

    let mut substitute_inspection = description();
    substitute_inspection.coverage.media_inspection[0].canonical_path =
        "EPUB/audio/other.mp3".to_owned();
    assert!(validate_media_overlay_timeline_result(&result, &substitute_inspection).is_err());
}

#[test]
fn local_text_resource_without_fragment_needs_no_fragment_coverage() {
    let mut value = description();
    value.coverage.fragment_observations.clear();
    let mut full = timeline();
    full.segments[0].text.reference = MediaOverlayReference::LocalResource {
        authored_href: "chapter.xhtml".to_owned(),
        canonical_path: "EPUB/chapter.xhtml".to_owned(),
        query: None,
    };
    validate_segment_coverage(&full, &value.coverage).unwrap();
}

#[test]
fn analysis_unavailable_retains_no_extracted_timeline_or_entries() {
    let package = description().package;
    let mut summary = summary();
    summary.availability = MediaOverlayTimelineAvailability::Unavailable {
        reason: MediaOverlayTimelineIssue::AnalysisUnavailable,
    };
    assert!(validate_timeline_summary(&summary, &package).is_err());

    summary.segment_count = 0;
    summary.playable_segment_count = 0;
    summary.skippable.clear();
    summary.escapable.clear();
    validate_timeline_summary(&summary, &package).unwrap();

    let value = description();
    let association = MediaOverlayAssociation {
        occurrence_index: 0,
        effective_linear: EffectiveLinear::Yes,
        content: ReadingOrderContentTarget::Resolved {
            declaration: content_declaration(),
            canonical_path: "EPUB/chapter.xhtml".to_owned(),
            bytes: MediaOverlayLocalBytes::Present {},
        },
        overlay: OverlayAssociationState::AnalysisUnavailable {
            declaration: declaration(),
            overlay_index: 0,
            issue: MediaOverlayAnalysisIssue::Unreadable,
        },
        entry_segment_indexes: vec![0],
    };
    assert!(
        validate_association(&association, &[summary], &value.package, &value.coverage,).is_err()
    );
}

#[test]
fn every_analysis_issue_variant_has_a_stable_wire_value() {
    let issues = [
        (MediaOverlayAnalysisIssue::ResourceLimit, "resource-limit"),
        (
            MediaOverlayAnalysisIssue::PerResourceAnalysisLimit,
            "per-resource-analysis-limit",
        ),
        (
            MediaOverlayAnalysisIssue::TotalAnalysisLimit,
            "total-analysis-limit",
        ),
        (
            MediaOverlayAnalysisIssue::TotalFingerprintLimit,
            "total-fingerprint-limit",
        ),
        (MediaOverlayAnalysisIssue::SmilNodeLimit, "smil-node-limit"),
        (
            MediaOverlayAnalysisIssue::SmilNestingLimit,
            "smil-nesting-limit",
        ),
        (MediaOverlayAnalysisIssue::Missing, "missing"),
        (MediaOverlayAnalysisIssue::Unreadable, "unreadable"),
        (MediaOverlayAnalysisIssue::Unsupported, "unsupported"),
        (MediaOverlayAnalysisIssue::Malformed, "malformed"),
    ];
    for (issue, expected) in issues {
        assert_eq!(serde_json::to_value(issue).unwrap(), expected);
    }
}
