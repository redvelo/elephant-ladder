use crate::media_overlay::{
    FragmentExistence, MediaOverlayBindingAvailability, MediaOverlayBindingIssue,
    MediaOverlayMalformedReason, MediaOverlayReference, MediaOverlaySegmentPlayability,
    MediaOverlaySegmentUnavailableReason, MediaOverlaySemanticNodeKind,
    MediaOverlayUnsupportedReason,
};
use crate::tests::support::synthetic_epub;

use epub_navigator::kernel::project_reading_order;
use epub_stack::{EpubZip, analysis::AnalysisLimits};

use crate::media_overlay::{
    MediaOverlayCoverageState, MediaOverlayPackageFacts, MediaOverlayPlayback,
    MediaOverlayTimelineAvailability, validate::validate_media_overlay_timeline_result,
    validate_media_overlay_description,
};

use super::*;

fn analyze(
    bytes: &[u8],
    limits: AnalysisLimits,
) -> (PublicationAnalysis, Vec<ReadingOrderOccurrence>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.epub");
    std::fs::write(&path, bytes).unwrap();
    let zip = EpubZip::open(&path).unwrap();
    let epub = zip.default_rendition().unwrap();
    let facts = epub_navigator::facts::PublicationFacts::of(&epub).unwrap();
    let reading_order = project_reading_order(&facts).unwrap();
    let analysis = epub.analyze_with_limits(limits);
    (analysis, reading_order)
}

const BASE_PACKAGE_HEAD: &str = r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language></metadata>
  <manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#;

const NAV_DOC: &[u8] =
    br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;

pub(crate) fn playable_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
<item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
<item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
<seq id="seq1" epub:textref="chapter.xhtml">
  <par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="1.5s"/></par>
  <par id="par2"><text src="chapter.xhtml#s2"/><audio src="audio/chapter.mp3" clipBegin="1.5s" clipEnd="3s"/></par>
</seq>
  </body>
</smil>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p><p id="s2">Some years ago.</p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[
            ("EPUB/nav.xhtml", NAV_DOC),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
            ("EPUB/audio/chapter.mp3", b"id3-stub-audio-bytes"),
        ],
    )
}

fn sibling_structures_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
<item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
<item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
<seq id="root" epub:textref="chapter.xhtml">
  <seq id="n1" epub:type="note" epub:textref="chapter.xhtml#n1">
    <par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="1s"/></par>
  </seq>
  <seq id="n2" epub:type="note" epub:textref="chapter.xhtml#n2">
    <par id="par2"><text src="chapter.xhtml#s2"/><audio src="audio/chapter.mp3" clipBegin="1s" clipEnd="2s"/></par>
  </seq>
</seq>
  </body>
</smil>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="n1"><span id="s1">First note.</span></p><p id="n2"><span id="s2">Second note.</span></p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[
            ("EPUB/nav.xhtml", NAV_DOC),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
            ("EPUB/audio/chapter.mp3", b"id3-stub-audio-bytes"),
        ],
    )
}

#[test]
fn sibling_structures_carry_distinct_sequence_text_references() {
    let (analysis, reading_order) = analyze(&sibling_structures_epub(), AnalysisLimits::default());
    let MediaOverlayDerivation { timelines, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    let segments = &timelines[0].segments;
    assert_eq!(segments.len(), 2);

    let structure = |index: usize| {
        segments[index]
            .semantic_path
            .iter()
            .filter(|node| node.kind == MediaOverlaySemanticNodeKind::Sequence)
            .filter(|node| node.epub_types.iter().any(|token| token == "note"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let first = structure(0);
    let second = structure(1);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);

    // Identical `epub:type` tokens; only the authored `epub:textref` separates the siblings,
    // and escape relies on that to leave the right structure.
    assert_eq!(first[0].epub_types, second[0].epub_types);
    assert_ne!(first[0].text_reference, second[0].text_reference);
    assert!(matches!(
        first[0].text_reference,
        Some(MediaOverlayReference::LocalFragment { ref fragment, .. }) if fragment == "n1"
    ));
    assert!(matches!(
        second[0].text_reference,
        Some(MediaOverlayReference::LocalFragment { ref fragment, .. }) if fragment == "n2"
    ));
}

#[test]
fn playable_publication_produces_two_playable_segments() {
    let (analysis, reading_order) = analyze(&playable_epub(), AnalysisLimits::default());
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();

    let MediaOverlayPackageFacts::Available { declared, .. } = &description.package else {
        panic!("expected available package facts");
    };
    assert!(*declared);

    let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &description.playback
    else {
        panic!(
            "expected available playback, got {:?}",
            description.playback
        );
    };
    assert_eq!(overlays.len(), 1);
    assert_eq!(overlays[0].segment_count, 2);
    assert_eq!(overlays[0].playable_segment_count, 2);
    assert_eq!(
        overlays[0].availability,
        MediaOverlayTimelineAvailability::Available {}
    );
    assert_eq!(associations.len(), 1);
    assert!(matches!(
        associations[0].overlay,
        OverlayAssociationState::Resolved {
            overlay_index: 0,
            ..
        }
    ));
    assert_eq!(associations[0].entry_segment_indexes, vec![0, 1]);
    assert_eq!(
        description.coverage.state,
        MediaOverlayCoverageState::Complete
    );
}

pub(crate) fn text_only_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
<item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
<seq id="seq1" epub:textref="chapter.xhtml">
  <par id="par1"><text src="chapter.xhtml#s1"/></par>
</seq>
  </body>
</smil>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Text only.</p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[
            ("EPUB/nav.xhtml", NAV_DOC),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
        ],
    )
}

#[test]
fn text_only_segment_is_unsupported_not_malformed() {
    let (analysis, reading_order) = analyze(&text_only_epub(), AnalysisLimits::default());
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();

    // No playable segment exists, so overall playback is bounded-unavailable, but the
    // underlying package facts and declared relationship remain visible.
    assert!(matches!(
        description.package,
        MediaOverlayPackageFacts::Available { declared: true, .. }
    ));
    assert_eq!(
        description.playback,
        MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::NoPlayableSegments
        }
    );
}

pub(crate) fn malformed_missing_text_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
<item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
<item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
<seq id="seq1" epub:textref="chapter.xhtml">
  <par id="par1"><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="1s"/></par>
</seq>
  </body>
</smil>"#;
    let chapter =
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Text.</p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[
            ("EPUB/nav.xhtml", NAV_DOC),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
            ("EPUB/audio/chapter.mp3", b"id3-stub-audio-bytes"),
        ],
    )
}

#[test]
fn parallel_without_text_is_malformed() {
    let (analysis, reading_order) =
        analyze(&malformed_missing_text_epub(), AnalysisLimits::default());
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();
    let MediaOverlayPlayback::Unavailable { reason } = description.playback else {
        panic!("expected unavailable playback");
    };
    assert_eq!(reason, MediaOverlayAvailabilityIssue::NoPlayableSegments);
}

pub(crate) fn absent_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>No overlay here.</p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[("EPUB/nav.xhtml", NAV_DOC), ("EPUB/chapter.xhtml", chapter)],
    )
}

#[test]
fn publication_without_media_overlays_is_absent() {
    let (analysis, reading_order) = analyze(&absent_epub(), AnalysisLimits::default());
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();
    assert_eq!(description.playback, MediaOverlayPlayback::Absent {});
    assert_eq!(
        description.coverage.state,
        MediaOverlayCoverageState::Inapplicable
    );
    let MediaOverlayPackageFacts::Available {
        declared,
        declarations,
        ..
    } = &description.package
    else {
        panic!("expected available package facts");
    };
    assert!(!declared);
    assert!(declarations.is_empty());
}

fn missing_overlay_declaration_epub() -> Vec<u8> {
    let package = format!(
        r#"{head}
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="does-not-exist"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#,
        head = BASE_PACKAGE_HEAD
    );
    let chapter =
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Chapter.</p></body></html>"#;
    synthetic_epub(
        package.as_bytes(),
        &[("EPUB/nav.xhtml", NAV_DOC), ("EPUB/chapter.xhtml", chapter)],
    )
}

#[test]
fn missing_overlay_declaration_is_visible_in_associations() {
    let (analysis, reading_order) = analyze(
        &missing_overlay_declaration_epub(),
        AnalysisLimits::default(),
    );
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();
    // Declared is true because `media-overlay` was authored on a manifest declaration, even
    // though it never resolves to a physical SMIL resource.
    let MediaOverlayPackageFacts::Available { declared, .. } = &description.package else {
        panic!("expected available package facts");
    };
    assert!(*declared);
    assert_eq!(
        description.playback,
        MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::NoPlayableSegments
        }
    );
}

#[test]
fn oversized_smil_becomes_representation_limit_exceeded_timeline() {
    let mut limits = AnalysisLimits::default();
    limits.max_smil_nodes = Some(1);
    let (analysis, reading_order) = analyze(&playable_epub(), limits);
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();
    let MediaOverlayPlayback::Unavailable { reason } = description.playback else {
        panic!("expected unavailable playback under a tight SMIL node budget");
    };
    assert_eq!(reason, MediaOverlayAvailabilityIssue::NoPlayableSegments);
}

const ONE_DOC: &[u8] =
    br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="a">One.</p></body></html>"#;
const TWO_DOC: &[u8] =
    br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="b">Two.</p></body></html>"#;

fn par(text: &str, audio: &str, clip: &str) -> String {
    format!(r#"<par><text src="{text}"/><audio src="{audio}" {clip}/></par>"#)
}

fn overlay_epub(items: &str, spine: &str, pars: &[String], extra: &[(&str, &[u8])]) -> Vec<u8> {
    let package = format!(
        "{BASE_PACKAGE_HEAD}\n{items}\n  </manifest>\n  <spine>{spine}</spine>\n</package>"
    );
    let smil = format!(
        r#"<smil xmlns="http://www.w3.org/ns/SMIL" version="3.0"><body>{}</body></smil>"#,
        pars.concat()
    );
    let mut resources = vec![
        ("EPUB/nav.xhtml", NAV_DOC),
        ("EPUB/one.xhtml", ONE_DOC),
        ("EPUB/overlay.smil", smil.as_bytes()),
        ("EPUB/a.mp3", b"aaaa".as_slice()),
    ];
    resources.extend_from_slice(extra);
    synthetic_epub(package.as_bytes(), &resources)
}

const ONE_ITEM: &str = r#"    <item id="one" href="one.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
<item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
<item id="audio-a" href="a.mp3" media-type="audio/mpeg"/>"#;

fn derive(bytes: &[u8]) -> MediaOverlayDerivation {
    let (analysis, reading_order) = analyze(bytes, AnalysisLimits::default());
    let derivation = derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&derivation.description).unwrap();
    derivation
}

fn playability(timeline: &MediaOverlayTimeline) -> Vec<MediaOverlaySegmentPlayability> {
    timeline
        .segments
        .iter()
        .map(|segment| segment.playability.clone())
        .collect()
}

fn entry_indexes(description: &MediaOverlayDescription) -> Vec<(u32, u32, Vec<u32>)> {
    let MediaOverlayPlayback::Available { associations, .. } = &description.playback else {
        panic!(
            "expected available playback, got {:?}",
            description.playback
        );
    };
    associations
        .iter()
        .map(|association| {
            let OverlayAssociationState::Resolved { overlay_index, .. } = association.overlay
            else {
                panic!("expected a resolved overlay");
            };
            (
                association.occurrence_index,
                overlay_index,
                association.entry_segment_indexes.clone(),
            )
        })
        .collect()
}

#[test]
fn one_shared_timeline_spans_two_documents() {
    let items = format!(
        "{ONE_ITEM}\n    <item id=\"two\" href=\"two.xhtml\" media-type=\"application/xhtml+xml\" media-overlay=\"overlay\"/>"
    );
    let derivation = derive(&overlay_epub(
        &items,
        r#"<itemref idref="one"/><itemref idref="two"/>"#,
        &[
            par("one.xhtml#a", "a.mp3", r#"clipBegin="0s" clipEnd="1s""#),
            par("two.xhtml#b", "a.mp3", r#"clipBegin="1s" clipEnd="2s""#),
        ],
        &[("EPUB/two.xhtml", TWO_DOC)],
    ));
    assert_eq!(derivation.timelines.len(), 1);
    assert_eq!(derivation.timelines[0].playable_segment_count, 2);
    assert_eq!(
        entry_indexes(&derivation.description),
        vec![(0, 0, vec![0]), (1, 0, vec![1])]
    );
    assert_eq!(
        audio_resources_to_back(&derivation.timelines),
        vec!["EPUB/a.mp3".to_owned()]
    );
}

#[test]
fn duplicate_spine_occurrences_share_one_timeline() {
    let derivation = derive(&overlay_epub(
        ONE_ITEM,
        r#"<itemref idref="one"/><itemref idref="one"/>"#,
        &[par(
            "one.xhtml#a",
            "a.mp3",
            r#"clipBegin="0s" clipEnd="1s""#,
        )],
        &[],
    ));
    assert_eq!(derivation.timelines.len(), 1);
    assert_eq!(
        entry_indexes(&derivation.description),
        vec![(0, 0, vec![0]), (1, 0, vec![0])]
    );
}

#[test]
fn every_referenced_audio_resource_is_backed_once() {
    let items =
        format!("{ONE_ITEM}\n    <item id=\"audio-b\" href=\"b.mp3\" media-type=\"audio/mpeg\"/>");
    let derivation = derive(&overlay_epub(
        &items,
        r#"<itemref idref="one"/>"#,
        &[
            par("one.xhtml#a", "a.mp3", r#"clipBegin="0s" clipEnd="1s""#),
            par("one.xhtml#a", "b.mp3", r#"clipBegin="0s" clipEnd="1s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="1s" clipEnd="2s""#),
        ],
        &[("EPUB/b.mp3", b"bbbb")],
    ));
    assert_eq!(derivation.timelines[0].playable_segment_count, 3);
    assert_eq!(
        audio_resources_to_back(&derivation.timelines),
        vec!["EPUB/a.mp3".to_owned(), "EPUB/b.mp3".to_owned()]
    );
}

#[test]
fn unusable_audio_references_keep_their_reasons() {
    let clip = r#"clipBegin="0s" clipEnd="1s""#;
    let derivation = derive(&overlay_epub(
        ONE_ITEM,
        r#"<itemref idref="one"/>"#,
        &[
            par("one.xhtml#a", "missing.mp3", clip),
            par("one.xhtml#a", "https://example.com/a.mp3", clip),
            par("one.xhtml#a", "data:audio/mpeg;base64,AAAA", clip),
            par("one.xhtml#a", "one.xhtml", clip),
        ],
        &[],
    ));
    let timeline = &derivation.timelines[0];
    let audio = timeline
        .segments
        .iter()
        .map(|segment| segment.audio.availability.clone())
        .collect::<Vec<_>>();
    let unavailable = |reason| MediaOverlayBindingAvailability::Unavailable { reason };
    assert_eq!(
        audio,
        vec![
            unavailable(MediaOverlayBindingIssue::MissingResource),
            unavailable(MediaOverlayBindingIssue::UnsupportedTarget),
            unavailable(MediaOverlayBindingIssue::UnsupportedTarget),
            unavailable(MediaOverlayBindingIssue::UnsupportedMediaType),
        ]
    );
    let audio_unavailable = MediaOverlaySegmentPlayability::Unavailable {
        reason: MediaOverlaySegmentUnavailableReason::AudioBindingUnavailable,
    };
    assert_eq!(
        playability(timeline),
        vec![
            audio_unavailable.clone(),
            audio_unavailable.clone(),
            audio_unavailable,
            MediaOverlaySegmentPlayability::Unsupported {
                reason: MediaOverlayUnsupportedReason::UnsupportedAudioFormat,
            },
        ]
    );
    assert_eq!(
        derivation.description.playback,
        MediaOverlayPlayback::Unavailable {
            reason: MediaOverlayAvailabilityIssue::NoPlayableSegments
        }
    );
    assert_eq!(
        audio_resources_to_back(&derivation.timelines),
        Vec::<String>::new()
    );
}

#[test]
fn absent_clip_clocks_play_the_whole_resource_and_bad_ones_are_malformed() {
    let derivation = derive(&overlay_epub(
        ONE_ITEM,
        r#"<itemref idref="one"/>"#,
        &[
            par("one.xhtml#a", "a.mp3", r#"clipEnd="1s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="0s""#),
            par("one.xhtml#a", "a.mp3", ""),
            par("one.xhtml#a", "a.mp3", r#"clipEnd="0s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="soon" clipEnd="1s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="2s" clipEnd="1s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="1s" clipEnd="1s""#),
            par("one.xhtml#a", "a.mp3", r#"clipBegin="1s" clipEnd="2s""#),
        ],
        &[],
    ));
    let malformed = |reason| MediaOverlaySegmentPlayability::Malformed { reason };
    assert_eq!(
        playability(&derivation.timelines[0]),
        vec![
            MediaOverlaySegmentPlayability::Playable {},
            MediaOverlaySegmentPlayability::Playable {},
            MediaOverlaySegmentPlayability::Playable {},
            malformed(MediaOverlayMalformedReason::EmptyClip),
            malformed(MediaOverlayMalformedReason::UnrecognizedClock),
            malformed(MediaOverlayMalformedReason::ReversedClip),
            malformed(MediaOverlayMalformedReason::EmptyClip),
            MediaOverlaySegmentPlayability::Playable {},
        ]
    );
    assert_eq!(derivation.timelines[0].playable_segment_count, 4);
}

#[test]
fn unusable_text_references_are_unavailable_with_precise_reasons() {
    let duplicate = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="twice">One.</p><p id="twice">Two.</p></body></html>"#;
    let clip = r#"clipBegin="0s" clipEnd="1s""#;
    let items = format!(
        "{ONE_ITEM}\n    <item id=\"dup\" href=\"dup.xhtml\" media-type=\"application/xhtml+xml\"/>"
    );
    let derivation = derive(&overlay_epub(
        &items,
        r#"<itemref idref="one"/>"#,
        &[
            par("one.xhtml#missing", "a.mp3", clip),
            par("dup.xhtml#twice", "a.mp3", clip),
            par("https://example.com/one.xhtml#a", "a.mp3", clip),
            par("one.xhtml#a", "a.mp3", clip),
        ],
        &[("EPUB/dup.xhtml", duplicate)],
    ));
    let timeline = &derivation.timelines[0];
    let text = timeline
        .segments
        .iter()
        .map(|segment| segment.text.availability.clone())
        .collect::<Vec<_>>();
    let unavailable = |reason| MediaOverlayBindingAvailability::Unavailable { reason };
    assert_eq!(
        text,
        vec![
            unavailable(MediaOverlayBindingIssue::MissingFragment),
            unavailable(MediaOverlayBindingIssue::AmbiguousFragment),
            unavailable(MediaOverlayBindingIssue::UnsupportedTarget),
            MediaOverlayBindingAvailability::Available {},
        ]
    );
    assert!(matches!(
        &timeline.segments[1].text.reference,
        MediaOverlayReference::LocalFragment {
            fragment_exists: FragmentExistence::Ambiguous {},
            ..
        }
    ));
    let text_unavailable = MediaOverlaySegmentPlayability::Unavailable {
        reason: MediaOverlaySegmentUnavailableReason::TextBindingUnavailable,
    };
    assert_eq!(
        playability(timeline),
        vec![
            text_unavailable.clone(),
            text_unavailable.clone(),
            text_unavailable,
            MediaOverlaySegmentPlayability::Playable {},
        ]
    );
}

#[test]
fn moby_dick_media_overlay_publication_is_playable() {
    let bytes = std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-mo.epub")
        .expect("moby-dick-mo.epub fixture is committed");
    let (analysis, reading_order) = analyze(&bytes, AnalysisLimits::default());
    let MediaOverlayDerivation { description, .. } =
        derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();

    let MediaOverlayPackageFacts::Available { declared, .. } = &description.package else {
        panic!("expected available package facts");
    };
    assert!(*declared);
    let MediaOverlayPlayback::Available {
        overlays,
        associations,
    } = &description.playback
    else {
        panic!(
            "expected available playback, got {:?}",
            description.playback
        );
    };
    assert!(!overlays.is_empty());
    assert!(
        overlays
            .iter()
            .any(|overlay| overlay.playable_segment_count > 0)
    );
    assert!(associations.iter().any(|association| matches!(
        association.overlay,
        OverlayAssociationState::Resolved { .. }
    )));
    for overlay in overlays {
        assert!(overlay.playable_segment_count <= overlay.segment_count);
    }
}

#[test]
#[ignore = "writes Navigator's Moby-Dick playback description fixture"]
fn export_moby_dick_navigator_fixture() {
    let output = std::path::Path::new(
        "../../../../navigator/tests/fixtures/epub3-samples/moby-dick-media-overlays",
    );
    let bytes = std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-mo.epub")
        .expect("moby-dick-mo.epub fixture is committed");
    let (analysis, reading_order) = analyze(&bytes, AnalysisLimits::default());
    let MediaOverlayDerivation {
        description,
        timelines,
    } = derive_media_overlay_description(&analysis, &reading_order);
    validate_media_overlay_description(&description).unwrap();
    let write = |name: String, value: String| std::fs::write(output.join(name), value + "\n");
    write(
        "description.json".to_owned(),
        serde_json::to_string_pretty(&description).unwrap(),
    )
    .unwrap();
    for timeline in &timelines {
        let result = timeline.to_result();
        validate_media_overlay_timeline_result(&result, &description).unwrap();
        write(
            format!("timeline-{}.json", timeline.overlay_index),
            serde_json::to_string_pretty(&result).unwrap(),
        )
        .unwrap();
    }
}
