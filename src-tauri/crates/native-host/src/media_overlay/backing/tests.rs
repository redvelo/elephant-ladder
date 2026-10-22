/// Storage is not a constraint in these tests.
const TEST_AVAILABLE_BYTES: u64 = u64::MAX;

use crate::tests::support::synthetic_epub;

use epub_stack::EpubZip;

use super::*;

const PACKAGE: &[u8] = br#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language></metadata>
  <manifest>
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml"/>
<item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#;

fn open(
    bytes: &[u8],
) -> (
    tempfile::TempDir,
    epub_stack::Epub<EpubZip<std::io::BufReader<std::fs::File>>>,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.epub");
    std::fs::write(&path, bytes).unwrap();
    let zip = EpubZip::open(&path).unwrap();
    let epub = zip.default_rendition().unwrap();
    (directory, epub)
}

fn prepare(
    epub: &epub_stack::Epub<EpubZip<std::io::BufReader<std::fs::File>>>,
    path: &str,
    staging: &Path,
) -> Result<PreparedSidecar, SidecarPreparationError> {
    prepare_audio_sidecar(
        epub,
        &EpubPath::new(path).unwrap(),
        staging,
        TEST_AVAILABLE_BYTES,
        &HashMap::new(),
    )
}

#[test]
fn prepares_a_staged_sidecar_with_matching_length_and_deterministic_hash() {
    let audio = b"id3-stub-audio-bytes-that-are-not-actually-decodable";
    let bytes = synthetic_epub(
        PACKAGE,
        &[
            ("EPUB/chapter.xhtml", b"<html/>"),
            ("EPUB/audio/chapter.mp3", audio),
        ],
    );
    let (_directory, epub) = open(&bytes);
    let staging = tempfile::tempdir().unwrap();
    let prepared = prepare(&epub, "EPUB/audio/chapter.mp3", staging.path()).unwrap();
    assert_eq!(prepared.length, audio.len() as u64);
    assert_eq!(prepared.canonical_path, "EPUB/audio/chapter.mp3");
    assert_eq!(prepared.origin, SidecarOrigin::Copied);
    assert_eq!(prepared.media_type, None);
    let staged_bytes = std::fs::read(prepared.staging_path()).unwrap();
    assert_eq!(staged_bytes, audio);
    assert_eq!(
        prepared.storage_name,
        blake3::hash(audio).to_hex().to_string()
    );
}

#[test]
fn missing_resource_is_reported_as_unavailable() {
    let bytes = synthetic_epub(
        PACKAGE,
        &[
            ("EPUB/chapter.xhtml", b"<html/>"),
            ("EPUB/audio/chapter.mp3", b"stub"),
        ],
    );
    let (_directory, epub) = open(&bytes);
    let staging = tempfile::tempdir().unwrap();
    let error = prepare(&epub, "EPUB/audio/missing.mp3", staging.path()).unwrap_err();
    assert_eq!(error, SidecarPreparationError::ResourceUnavailable);
}

#[test]
fn oversized_resource_is_rejected_and_leaves_no_staging_file() {
    let audio = vec![0_u8; 4096];
    let bytes = synthetic_epub(
        PACKAGE,
        &[
            ("EPUB/chapter.xhtml", b"<html/>"),
            ("EPUB/audio/chapter.mp3", &audio),
        ],
    );
    let (_directory, epub) = open(&bytes);
    let staging = tempfile::tempdir().unwrap();
    let path = EpubPath::new("EPUB/audio/chapter.mp3").unwrap();
    // Storage available to the import, not an invented ceiling, is the only bound.
    let error =
        prepare_audio_sidecar_with_limit(&epub, &path, staging.path(), 1024, &HashMap::new())
            .unwrap_err();
    assert_eq!(error, SidecarPreparationError::InsufficientStorage);
    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);

    let error =
        prepare_audio_sidecar(&epub, &path, staging.path(), 128, &HashMap::new()).unwrap_err();
    assert_eq!(error, SidecarPreparationError::InsufficientStorage);
    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
}

#[test]
fn corrupt_entry_crc_fails_preparation_and_leaves_no_staging_file() {
    let audio = b"crc-protected-audio-bytes";
    let mut bytes = synthetic_epub(
        PACKAGE,
        &[
            ("EPUB/chapter.xhtml", b"<html/>"),
            ("EPUB/audio/chapter.mp3", audio),
        ],
    );
    let name = b"EPUB/audio/chapter.mp3";
    let local_header = bytes
        .windows(4 + 26 + name.len())
        .position(|window| window.starts_with(b"PK\x03\x04") && window.ends_with(name))
        .unwrap();
    for crc_offset in [local_header + 14, central_crc_offset(&bytes, name)] {
        bytes[crc_offset] ^= 0xff;
    }
    let (_directory, epub) = open(&bytes);
    let staging = tempfile::tempdir().unwrap();
    let error = prepare(&epub, "EPUB/audio/chapter.mp3", staging.path()).unwrap_err();
    assert!(
        matches!(
            error,
            SidecarPreparationError::Io(_) | SidecarPreparationError::ResourceUnavailable
        ),
        "{error:?}"
    );
    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
}

fn central_crc_offset(bytes: &[u8], name: &[u8]) -> usize {
    let header = bytes
        .windows(46 + name.len())
        .position(|window| window.starts_with(b"PK\x01\x02") && window.ends_with(name))
        .unwrap();
    header + 16
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use std::io::Cursor;

    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    use super::*;

    const EXCERPT: &[u8] =
        include_bytes!("../../../../../../tests/fixtures/media-overlays/moby-dick-excerpt.m4a");

    fn package(manifest: &str) -> Vec<u8> {
        format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language></metadata>
  <manifest>
<item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml"/>
{manifest}
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#
        )
        .into_bytes()
    }

    fn decode_i16(label: &str, bytes: Vec<u8>) -> (Vec<i16>, u32, usize) {
        let mut format = symphonia::default::get_probe()
            .probe(
                &Hint::new(),
                MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default()),
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .unwrap_or_else(|error| panic!("{label} probe failed: {error}"));
        let track = format.default_track(TrackType::Audio).unwrap();
        let track_id = track.id;
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            panic!("audio track");
        };
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .unwrap();
        let mut samples = Vec::new();
        let mut packet_samples = Vec::<i16>::new();
        let mut rate = 0;
        let mut channels = 0;
        loop {
            let packet = match format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(symphonia::core::errors::Error::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(error) => panic!("{error}"),
            };
            if packet.track_id != track_id {
                continue;
            }
            let decoded = decoder.decode(&packet).unwrap();
            rate = decoded.spec().rate();
            channels = decoded.spec().channels().count();
            decoded.copy_to_vec_interleaved::<i16>(&mut packet_samples);
            samples.extend_from_slice(&packet_samples);
        }
        (samples, rate, channels)
    }

    /// Walks every Ogg page, proving the framing is exact, and returns the final granule.
    fn last_granule(ogg: &[u8]) -> u64 {
        let mut offset = 0;
        let mut granule = 0;
        let mut pages = 0;
        while offset + 27 <= ogg.len() {
            assert_eq!(&ogg[offset..offset + 4], b"OggS");
            granule = u64::from_le_bytes(ogg[offset + 6..offset + 14].try_into().unwrap());
            let segments = usize::from(ogg[offset + 26]);
            let table = &ogg[offset + 27..offset + 27 + segments];
            let body = table
                .iter()
                .map(|length| usize::from(*length))
                .sum::<usize>();
            offset += 27 + segments + body;
            pages += 1;
        }
        assert_eq!(offset, ogg.len(), "ogg pages must tile the file exactly");
        assert!(pages >= 3, "expected header, tags, and audio pages");
        granule
    }

    #[test]
    fn aac_is_transcoded_to_playable_ogg_opus() {
        let bytes = synthetic_epub(
            &package(r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4"/>"#),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", EXCERPT),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let prepared = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap();
        assert_eq!(prepared.origin, SidecarOrigin::Transcoded);
        assert_eq!(prepared.media_type.as_deref(), Some("audio/ogg"));
        assert_eq!(
            prepared.source_hash.as_deref(),
            Some(blake3::hash(EXCERPT).to_hex().as_str())
        );
        let ogg = std::fs::read(prepared.staging_path()).unwrap();
        assert_eq!(prepared.length, ogg.len() as u64);
        assert_eq!(
            prepared.storage_name,
            blake3::hash(&ogg).to_hex().to_string()
        );
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 1);

        let head = ogg
            .windows(8)
            .position(|window| window == b"OpusHead")
            .expect("an Opus identification header");
        assert!(ogg.windows(8).any(|window| window == b"OpusTags"));
        assert_eq!(ogg[head + 8], 1, "Opus header version");
        let channels = usize::from(ogg[head + 9]);
        let pre_skip = u16::from_le_bytes([ogg[head + 10], ogg[head + 11]]);
        let input_rate = u32::from_le_bytes(ogg[head + 12..head + 16].try_into().unwrap());

        let (source_samples, source_rate, source_channels) = decode_i16("aac", EXCERPT.to_vec());
        assert_eq!(input_rate, source_rate, "OpusHead records the source rate");
        assert_eq!(channels, source_channels.min(2), "mono stays mono");

        // Duration survives the resample: the final granule counts 48 kHz samples plus the
        // encoder delay the decoder discards.
        let source_frames = (source_samples.len() / source_channels) as u64;
        let expected = source_frames * 48_000 / u64::from(source_rate);
        let granule = last_granule(&ogg);
        let slack = 2 * 960 + u64::from(pre_skip);
        assert!(
            granule >= expected && granule <= expected + slack,
            "granule {granule} should be within {slack} of {expected}"
        );

        // The whole point: a lossy source does not become gigabytes of lossless audio.
        let lossless = source_frames * source_channels as u64 * 2;
        assert!(
            prepared.length * 4 < lossless,
            "transcode {} should be far below lossless {lossless}",
            prepared.length
        );

        let again = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap();
        assert_eq!(again.storage_name, prepared.storage_name);
    }

    #[test]
    fn a_playable_manifest_fallback_is_served_instead_of_transcoding() {
        let bytes = synthetic_epub(
            &package(
                r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4" fallback="mp3"/>
<item id="mp3" href="audio/chapter.mp3" media-type="audio/mpeg"/>"#,
            ),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", EXCERPT),
                ("EPUB/audio/chapter.mp3", b"fallback-audio"),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let prepared = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap();
        assert_eq!(prepared.origin, SidecarOrigin::Fallback);
        assert_eq!(prepared.canonical_path, "EPUB/audio/chapter.m4a");
        assert_eq!(prepared.media_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(
            std::fs::read(prepared.staging_path()).unwrap(),
            b"fallback-audio"
        );
    }

    #[test]
    fn cyclic_and_remote_fallbacks_fall_through_to_transcoding() {
        let bytes = synthetic_epub(
            &package(
                r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4" fallback="remote"/>
<item id="remote" href="https://example.com/chapter.mp3" media-type="audio/mpeg" fallback="audio"/>"#,
            ),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", EXCERPT),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let prepared = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap();
        assert_eq!(prepared.origin, SidecarOrigin::Transcoded);
    }

    #[test]
    fn an_installed_transcode_of_the_same_source_is_reused() {
        let bytes = synthetic_epub(
            &package(r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4"/>"#),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", EXCERPT),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let reusable = HashMap::from([(
            blake3::hash(EXCERPT).to_hex().to_string(),
            ReusableSidecar {
                storage_name: "a".repeat(64),
                length: 1234,
            },
        )]);
        let prepared = prepare_audio_sidecar(
            &epub,
            &EpubPath::new("EPUB/audio/chapter.m4a").unwrap(),
            staging.path(),
            TEST_AVAILABLE_BYTES,
            &reusable,
        )
        .unwrap();
        assert_eq!(prepared.origin, SidecarOrigin::Reused);
        assert_eq!(prepared.storage_name, "a".repeat(64));
        assert_eq!(prepared.length, 1234);
        let parts = prepared.into_parts();
        assert_eq!(parts.staging_path, None);
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    fn movie_header_end(mp4: &[u8]) -> usize {
        let mut offset = 0;
        loop {
            let size = u32::from_be_bytes(mp4[offset..offset + 4].try_into().unwrap());
            let kind = &mp4[offset + 4..offset + 8];
            offset += usize::try_from(size).unwrap();
            if kind == b"moov" {
                return offset;
            }
        }
    }

    #[test]
    fn aac_without_audio_data_fails_and_leaves_no_staging_file() {
        let header_only = &EXCERPT[..movie_header_end(EXCERPT)];
        let bytes = synthetic_epub(
            &package(r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4"/>"#),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", header_only),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let error = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap_err();
        assert!(
            matches!(
                error,
                SidecarPreparationError::Decode(_) | SidecarPreparationError::Io(_)
            ),
            "{error:?}"
        );
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
    }

    #[test]
    #[ignore = "writes Navigator's transcoded Moby-Dick narration excerpt fixture"]
    fn export_moby_dick_opus_excerpt_navigator_fixture() {
        let bytes = synthetic_epub(
            &package(r#"<item id="audio" href="audio/chapter.m4a" media-type="audio/mp4"/>"#),
            &[
                ("EPUB/chapter.xhtml", b"<html/>"),
                ("EPUB/audio/chapter.m4a", EXCERPT),
            ],
        );
        let (_directory, epub) = open(&bytes);
        let staging = tempfile::tempdir().unwrap();
        let prepared = prepare(&epub, "EPUB/audio/chapter.m4a", staging.path()).unwrap();
        std::fs::copy(
            prepared.staging_path(),
            "../../../../navigator/tests/fixtures/epub3-samples/moby-dick-excerpt.ogg",
        )
        .unwrap();
    }
}

mod engine_audio {
    use crate::media_overlay::backing::{EngineAudio, READING_ENGINE_AUDIO};
    use crate::media_overlay::transcode::{AudioTarget, SourceAudio};

    const CEF: EngineAudio = EngineAudio {
        plays_aac: false,
        plays_ogg_opus: true,
    };
    const ANDROID_WEBVIEW: EngineAudio = EngineAudio {
        plays_aac: true,
        plays_ogg_opus: false,
    };

    #[test]
    fn each_engine_converts_only_what_it_cannot_play() {
        assert_eq!(
            CEF.conversion(Some(SourceAudio::Aac)),
            Some(AudioTarget::OggOpus)
        );
        assert_eq!(CEF.conversion(Some(SourceAudio::OggOpus)), None);

        assert_eq!(ANDROID_WEBVIEW.conversion(Some(SourceAudio::Aac)), None);
        assert_eq!(
            ANDROID_WEBVIEW.conversion(Some(SourceAudio::OggOpus)),
            Some(AudioTarget::Mp3)
        );
    }

    /// Opus in MP4 is playable nowhere, so neither engine may wave it through.
    #[test]
    fn opus_in_mp4_is_converted_by_every_engine() {
        assert_eq!(
            CEF.conversion(Some(SourceAudio::OpusInMp4)),
            Some(AudioTarget::OggOpus)
        );
        assert_eq!(
            ANDROID_WEBVIEW.conversion(Some(SourceAudio::OpusInMp4)),
            Some(AudioTarget::Mp3)
        );
    }

    /// The container alone cannot say which codec is inside, so a declared one is read.
    #[test]
    fn a_declared_opus_codec_is_considered_even_where_the_container_usually_plays() {
        for engine in [CEF, ANDROID_WEBVIEW] {
            assert!(engine.may_need_conversion("audio/mp4; codecs=opus"));
            assert!(engine.may_need_conversion("audio/mp4; codecs=\"opus\""));
        }
        // Plain MP4 is taken as the AAC it almost always is, which only CEF lacks.
        assert!(CEF.may_need_conversion("audio/mp4"));
        assert!(!ANDROID_WEBVIEW.may_need_conversion("audio/mp4"));
        assert!(!ANDROID_WEBVIEW.may_need_conversion("audio/mp4; codecs=aac"));
        // Every engine plays these, declared codec or not.
        assert!(!CEF.may_need_conversion("audio/mpeg"));
        assert!(!ANDROID_WEBVIEW.may_need_conversion("audio/mpeg"));
    }

    #[test]
    fn an_audio_type_outside_the_two_gaps_is_never_converted() {
        assert_eq!(CEF.conversion(None), None);
        assert_eq!(ANDROID_WEBVIEW.conversion(None), None);
    }

    /// The reuse index is keyed by derivation, so the build's own target has to be the
    /// one its installed sidecars were produced with.
    #[test]
    fn this_build_reuses_sidecars_of_the_target_it_produces() {
        let expected = if cfg!(target_os = "android") {
            AudioTarget::Mp3
        } else {
            AudioTarget::OggOpus
        };
        assert_eq!(READING_ENGINE_AUDIO.target(), expected);
        assert_eq!(
            crate::media_overlay::backing::transcode_derivation(),
            expected.derivation()
        );
    }
}

mod audio_paths {
    use crate::media_overlay::backing::audio_paths_to_back;
    use crate::snapshot::{ResponseManifest, ResponseRule};

    fn manifest(entries: &[(&str, &str, bool)]) -> ResponseManifest {
        use crate::snapshot::{Foreground, ForegroundPreparationState, Measurement};

        let mut resources = std::collections::BTreeMap::new();
        for (path, mime, missing) in entries {
            resources.insert(
                (*path).to_owned(),
                ResponseRule {
                    mime: (*mime).to_owned(),
                    size_bytes: 1,
                    missing: *missing,
                    undeclared: false,
                    fallback_source: None,
                    foreground: Foreground::Unchanged,
                    foreground_preparation: ForegroundPreparationState::Ineligible,
                    measurement: Measurement::Denied,
                },
            );
        }
        ResponseManifest {
            resources,
            undeclared_references: Vec::new(),
        }
    }

    #[test]
    fn narration_is_always_backed_and_never_duplicated() {
        let responses = manifest(&[
            ("EPUB/audio/narration.m4a", "audio/mp4", false),
            ("EPUB/audio/narration.ogg", "audio/ogg", false),
        ]);
        let backed = audio_paths_to_back(vec!["EPUB/audio/narration.m4a".to_owned()], &responses);
        assert_eq!(
            backed.iter().filter(|path| path.ends_with(".m4a")).count(),
            1
        );
        assert_eq!(backed[0], "EPUB/audio/narration.m4a");
    }

    #[test]
    fn only_audio_this_engine_might_not_play_is_backed_beyond_narration() {
        let responses = manifest(&[
            ("EPUB/a.mp3", "audio/mpeg", false),
            ("EPUB/b.wav", "audio/wav", false),
            ("EPUB/c.m4a", "audio/mp4", false),
            ("EPUB/d.opus", "audio/ogg", false),
            ("EPUB/e.png", "image/png", false),
        ]);
        let backed = audio_paths_to_back(Vec::new(), &responses);
        // Every engine plays MP3 and WAV, so they are served straight from the archive.
        assert!(!backed.iter().any(|path| path.ends_with(".mp3")));
        assert!(!backed.iter().any(|path| path.ends_with(".wav")));
        assert!(!backed.iter().any(|path| path.ends_with(".png")));
        let gap = if cfg!(target_os = "android") {
            "EPUB/d.opus"
        } else {
            "EPUB/c.m4a"
        };
        assert_eq!(backed, vec![gap.to_owned()]);
    }

    #[test]
    fn a_resource_with_no_bytes_is_not_backed() {
        let responses = manifest(&[
            ("EPUB/c.m4a", "audio/mp4", true),
            ("EPUB/d.opus", "audio/ogg", true),
        ]);
        assert!(audio_paths_to_back(Vec::new(), &responses).is_empty());
    }
}
