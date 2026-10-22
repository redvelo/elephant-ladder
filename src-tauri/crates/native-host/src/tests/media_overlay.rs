use super::support::*;
use crate::mount::ImportedPublicationState;

#[test]
fn media_overlay_description_round_trips_through_import_and_reopen() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language>
    <meta property="media:duration">0:00:03</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
    <item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
    <item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#;
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
    <seq id="seq1" epub:textref="chapter.xhtml">
      <par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="1.5s"/></par>
      <par id="par2"><text src="chapter.xhtml#s2"/><audio src="audio/chapter.mp3" clipBegin="1.5s" clipEnd="3s"/></par>
    </seq>
  </body>
</smil>"#;
    let nav =
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p><p id="s2">Some years ago.</p></body></html>"#;
    let bytes = synthetic_epub(
        package,
        &[
            ("EPUB/nav.xhtml", nav),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
            ("EPUB/audio/chapter.mp3", b"id3-stub-audio-bytes"),
        ],
    );
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        u64::MAX,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    assert!(matches!(
        derived.media_overlay.playback,
        crate::media_overlay::MediaOverlayPlayback::Available { .. }
    ));
    let imported = storage
        .commit_import_snapshot_with_policy_relevance(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            crate::PRODUCTION_READING_REVISION,
            "Moby",
            crate::storage::PolicyAwareReadingProducts {
                facts: &derived.facts,
                package: &derived.package,
                responses: &derived.responses,
                media_overlay: &derived.media_overlay,
                media_overlay_timelines: &derived.media_overlay_timelines,
                prepared_sidecars: derived.prepared_sidecars,
                policy_relevance: derived.policy_relevance,
            },
        )
        .unwrap();

    // Reopening with the current reading revision returns the persisted description
    // byte-for-byte equal to what was derived at import time.
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            crate::PRODUCTION_READING_REVISION,
            None,
        )
        .unwrap()
    else {
        panic!("media overlay publication must be ready");
    };
    assert_eq!(selected.media_overlay, derived.media_overlay);
    assert_eq!(
        selected.media_overlay_timeline_files.len(),
        derived.media_overlay_timelines.len()
    );
    for timeline in &derived.media_overlay_timelines {
        let stored = &selected.media_overlay_timeline_files[&timeline.overlay_index];
        assert_eq!(
            std::fs::read(&stored.path).unwrap(),
            timeline.to_result_json().unwrap(),
            "stored timeline bytes are the exact response body"
        );
    }

    // A timeline file that went missing routes to re-derivation, exactly as a missing sidecar
    // does, rather than failing the open or serving a publication with broken narration.
    let missing = selected.media_overlay_timeline_files[&0].path.clone();
    let restored = std::fs::read(&missing).unwrap();
    std::fs::remove_file(&missing).unwrap();
    assert!(matches!(
        storage
            .select_publication(
                &imported.publication.publication_id,
                crate::PRODUCTION_READING_REVISION,
                None,
            )
            .unwrap(),
        crate::storage::PublicationSelection::NeedsDerivation(_)
    ));
    std::fs::write(&missing, &restored).unwrap();
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            crate::PRODUCTION_READING_REVISION,
            None,
        )
        .unwrap()
    else {
        panic!("restoring the timeline file must make the publication ready again");
    };

    // The compressed audio resource was materialized into a committed sidecar row and file.
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    let (canonical_path, storage_name, length_bytes): (String, String, u64) = connection
        .query_row(
            "SELECT canonical_path, storage_name, length_bytes
                 FROM media_overlay_sidecars WHERE snapshot_id = ?1",
            [&selected.snapshot_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(canonical_path, "EPUB/audio/chapter.mp3");
    assert_eq!(length_bytes, b"id3-stub-audio-bytes".len() as u64);
    let sidecar_bytes =
        std::fs::read(directory.path().join("media-sidecars").join(storage_name)).unwrap();
    assert_eq!(sidecar_bytes, b"id3-stub-audio-bytes");

    // An older reading revision (as if the schema predates this feature) forces
    // re-derivation rather than serving a stale or absent description.
    let needs_derivation = storage
        .select_publication(&imported.publication.publication_id, "reading-v12", None)
        .unwrap();
    assert!(matches!(
        needs_derivation,
        crate::storage::PublicationSelection::NeedsDerivation(_)
    ));
}

#[test]
fn a_failed_audio_backing_leaves_other_timelines_playable_and_the_book_readable() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language>
    <meta property="media:duration">0:00:02</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="one" href="one.xhtml" media-type="application/xhtml+xml" media-overlay="one-overlay"/>
    <item id="two" href="two.xhtml" media-type="application/xhtml+xml" media-overlay="two-overlay"/>
    <item id="one-overlay" href="one.smil" media-type="application/smil+xml"/>
    <item id="two-overlay" href="two.smil" media-type="application/smil+xml"/>
    <item id="one-audio" href="audio/one.mp3" media-type="audio/mpeg"/>
    <item id="two-audio" href="audio/two.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="one"/><itemref idref="two"/></spine>
</package>"#;
    let smil = |chapter: &str| {
        format!(
            r#"<smil xmlns="http://www.w3.org/ns/SMIL" version="3.0">
  <body><par id="par1"><text src="{chapter}.xhtml#s1"/><audio src="audio/{chapter}.mp3" clipBegin="0s" clipEnd="1s"/></par></body>
</smil>"#
        )
    };
    let (one_smil, two_smil) = (smil("one"), smil("two"));
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="one.xhtml">One</a></li><li><a href="two.xhtml">Two</a></li></ol></nav></body></html>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p></body></html>"#;
    let bytes = synthetic_epub(
        package,
        &[
            ("EPUB/nav.xhtml", nav),
            ("EPUB/one.xhtml", chapter),
            ("EPUB/two.xhtml", chapter),
            ("EPUB/one.smil", one_smil.as_bytes()),
            ("EPUB/two.smil", two_smil.as_bytes()),
            ("EPUB/audio/one.mp3", b"first-audio-bytes!!!"),
            ("EPUB/audio/two.mp3", b"second-audio-bytes!!"),
        ],
    );
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        20,
        &std::collections::HashMap::new(),
    )
    .unwrap();

    assert_eq!(derived.prepared_sidecars.len(), 1);
    assert_eq!(
        derived.prepared_sidecars[0].canonical_path,
        "EPUB/audio/one.mp3"
    );
    assert!(matches!(
        derived.media_overlay.playback,
        crate::media_overlay::MediaOverlayPlayback::Available { .. }
    ));
    assert!(matches!(
        derived.media_overlay_timelines[0].availability,
        crate::media_overlay::MediaOverlayTimelineAvailability::Available {}
    ));
    assert!(matches!(
        derived.media_overlay_timelines[1].availability,
        crate::media_overlay::MediaOverlayTimelineAvailability::Unavailable {
            reason: crate::media_overlay::MediaOverlayTimelineIssue::NoPlayableSegments
        }
    ));
    assert_eq!(
        crate::media_overlay::derive_media_overlay_capability(
            &derived.media_overlay,
            &derived.media_overlay_timelines
        ),
        crate::media_overlay::MediaOverlayCapability::Available {
            playable_occurrence_indexes: vec![0],
            unavailable_occurrence_count: 1,
        }
    );

    let imported = storage
        .commit_import_snapshot_with_policy_relevance(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            crate::PRODUCTION_READING_REVISION,
            "Partial",
            crate::storage::PolicyAwareReadingProducts {
                facts: &derived.facts,
                package: &derived.package,
                responses: &derived.responses,
                media_overlay: &derived.media_overlay,
                media_overlay_timelines: &derived.media_overlay_timelines,
                prepared_sidecars: derived.prepared_sidecars,
                policy_relevance: derived.policy_relevance,
            },
        )
        .unwrap();
    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            crate::PRODUCTION_READING_REVISION,
            None,
        )
        .unwrap()
    else {
        panic!("a publication with one failed audio backing must stay readable");
    };
    assert_eq!(selected.media_overlay, derived.media_overlay);
    assert_eq!(
        selected.media_overlay_timeline_files.len(),
        derived.media_overlay_timelines.len()
    );
    for timeline in &derived.media_overlay_timelines {
        let stored = &selected.media_overlay_timeline_files[&timeline.overlay_index];
        assert_eq!(
            std::fs::read(&stored.path).unwrap(),
            timeline.to_result_json().unwrap(),
            "stored timeline bytes are the exact response body"
        );
    }
}

#[test]
fn publication_deletion_removes_media_sidecars_no_other_snapshot_uses() {
    fn import_narrated(
        storage: &mut crate::storage::Storage,
        directory: &std::path::Path,
        identifier: &str,
    ) -> (String, String) {
        let package = format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>{identifier}</dc:title><dc:identifier id="uid">{identifier}</dc:identifier><dc:language>en</dc:language>
    <meta property="media:duration">0:00:01</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
    <item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
    <item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#
        );
        let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" version="3.0">
  <body><par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="1s"/></par></body>
</smil>"#;
        let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
        let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p></body></html>"#;
        let bytes = synthetic_epub(
            package.as_bytes(),
            &[
                ("EPUB/nav.xhtml", nav),
                ("EPUB/chapter.xhtml", chapter),
                ("EPUB/overlay.smil", smil),
                ("EPUB/audio/chapter.mp3", b"id3-stub-audio-bytes"),
            ],
        );
        let blob = storage.install_blob(&bytes).unwrap();
        let derived = crate::snapshot::derive_publication(
            &blob.path,
            directory,
            u64::MAX,
            &std::collections::HashMap::new(),
        )
        .unwrap();
        let imported = storage
            .commit_import_snapshot_with_policy_relevance(
                crate::storage::InstalledImport {
                    archive: &blob,
                    cover: None,
                },
                crate::PRODUCTION_READING_REVISION,
                identifier,
                crate::storage::PolicyAwareReadingProducts {
                    facts: &derived.facts,
                    package: &derived.package,
                    responses: &derived.responses,
                    media_overlay: &derived.media_overlay,
                    media_overlay_timelines: &derived.media_overlay_timelines,
                    prepared_sidecars: derived.prepared_sidecars,
                    policy_relevance: derived.policy_relevance,
                },
            )
            .unwrap();
        let publication_id = imported.publication.publication_id;
        let storage_name = Connection::open(directory.join(crate::storage::LIBRARY_DATABASE))
                .unwrap()
                .query_row(
                    "SELECT storage_name FROM media_overlay_sidecars
                     JOIN publications ON publications.current_snapshot_id = media_overlay_sidecars.snapshot_id
                     WHERE publications.publication_id = ?1",
                    [&publication_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
        (publication_id, storage_name)
    }
    fn delete(storage: &mut crate::storage::Storage, publication_id: &str) {
        assert!(matches!(
            storage.begin_publication_deletion(publication_id).unwrap(),
            crate::storage::BeginPublicationDeletionOutcome::Started { .. }
        ));
        storage
            .complete_publication_deletion(publication_id)
            .unwrap();
    }

    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let (first, first_sidecar) = import_narrated(&mut storage, directory.path(), "first");
    let (second, second_sidecar) = import_narrated(&mut storage, directory.path(), "second");
    assert_eq!(first_sidecar, second_sidecar);
    let sidecar = directory.path().join("media-sidecars").join(&first_sidecar);

    delete(&mut storage, &first);
    assert!(sidecar.is_file());
    delete(&mut storage, &second);
    assert!(!sidecar.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn narration_playback_routes_reach_native_media_controls_for_the_active_session() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestNarrationActions::default());
    let mut state = fixture_reader_state(directory.path());
    state.native_actions = actions.clone();
    let lifecycle = state.lifecycle.clone();
    let router = reader_router(state, None);
    let imported = router.clone().oneshot(import_request()).await.unwrap();
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(open_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(*actions.playback.lock().unwrap(), vec![None]);
    actions.playback.lock().unwrap().clear();
    let session = lifecycle.load_active().unwrap();
    let report = |body: &str, session_id: &str| {
        Request::builder()
            .method("POST")
            .uri("/native/reader/narration")
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret")
            .header(CONTENT_TYPE, "application/json")
            .header("x-elephant-reader-session", session_id)
            .body(Body::from(body.to_owned()))
            .unwrap()
    };
    let commands = |session_id: &str| {
        Request::builder()
            .method("GET")
            .uri("/native/reader/narration/commands")
            .header("host", "reader.test")
            .header(AUTHORIZATION, "Bearer secret")
            .header("x-elephant-request", "trusted-reader")
            .header("x-elephant-reader-session", session_id)
            .body(Body::empty())
            .unwrap()
    };
    let stale = "0".repeat(64);

    let playing = r#"{"state":"playing","title":"Moby-Dick","section":"Chapter 1. Loomings.","positionMs":1500,"durationMs":860500,"rate":1.25}"#;
    let paused = r#"{"state":"paused","title":"Moby-Dick","section":null,"positionMs":2000,"durationMs":860500,"rate":1}"#;
    let stopped = r#"{"state":"stopped","title":"","section":null,"positionMs":null,"durationMs":null,"rate":1}"#;
    let active = session.reader_session_id.as_ref();
    for (body, session_id, status) in [
        (playing, active, StatusCode::NO_CONTENT),
        (paused, active, StatusCode::NO_CONTENT),
        (stopped, active, StatusCode::NO_CONTENT),
        (
            r#"{"state":"loading","title":"Moby-Dick","section":null,"positionMs":null,"durationMs":null,"rate":1}"#,
            active,
            StatusCode::BAD_REQUEST,
        ),
        (
            "{\"state\":\"playing\",\"title\":\"Moby\\nDick\",\"section\":null,\"positionMs\":null,\"durationMs\":null,\"rate\":1}",
            active,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"state":"playing","title":"Moby-Dick","section":null,"positionMs":900000,"durationMs":860500,"rate":1}"#,
            active,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"state":"playing","title":"Moby-Dick","section":null,"positionMs":null,"durationMs":null,"rate":9}"#,
            active,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"state":"playing","title":"Moby-Dick","section":null,"positionMs":null,"durationMs":null,"rate":1,"extra":1}"#,
            active,
            StatusCode::BAD_REQUEST,
        ),
        (playing, stale.as_str(), StatusCode::CONFLICT),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(report(body, session_id))
                .await
                .unwrap()
                .status(),
            status,
            "{body}"
        );
    }
    assert_eq!(
        *actions.playback.lock().unwrap(),
        vec![
            Some((
                crate::NarrationPlaybackState::Playing,
                "Moby-Dick".to_owned(),
                Some("Chapter 1. Loomings.".to_owned()),
                Some(1500),
                Some(860500),
            )),
            Some((
                crate::NarrationPlaybackState::Paused,
                "Moby-Dick".to_owned(),
                None,
                Some(2000),
                Some(860500),
            )),
            None,
        ]
    );

    *actions.commands.lock().unwrap() = vec![
        crate::NarrationCommand::Pause,
        crate::NarrationCommand::Play,
    ];
    let response = router
        .clone()
        .oneshot(commands(session.reader_session_id.as_ref()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body, serde_json::json!({ "commands": ["pause", "play"] }));
    assert_eq!(
        router
            .clone()
            .oneshot(commands(&stale))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test(flavor = "current_thread")]
async fn media_overlay_routes_and_sidecar_audio_follow_the_active_session() {
    let archive =
        std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-mo.epub").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let storage = state.storage.clone();
    let active = state.lifecycle.active_view();
    let foreground = crate::mount::imported_publication_router(ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    });
    let reader = reader_router(state, None);
    let created = reader
        .clone()
        .oneshot(import_request_with(archive))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        reader
            .clone()
            .oneshot(open_request(&id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let session = active.load().unwrap();
    assert!(matches!(
        session.entry.media_overlay_capability,
        crate::media_overlay::MediaOverlayCapability::Available { .. }
    ));

    let audio_path = "OPS/audio/mobydick_001_002_melville.mp4";
    let sidecar = &session.entry.media_sidecars[audio_path];
    let audio = std::fs::read(&sidecar.path).unwrap();
    let rule = &session.entry.responses.resources[audio_path];
    assert_eq!(rule.mime, "audio/mp4");
    let mime = if cfg!(any(target_os = "android", target_os = "ios")) {
        assert_eq!(&audio[4..8], b"ftyp");
        assert_eq!(sidecar.media_type, None);
        assert_eq!(rule.size_bytes, audio.len() as u64);
        "audio/mp4"
    } else {
        assert_eq!(&audio[..4], b"OggS");
        assert_eq!(sidecar.media_type.as_deref(), Some("audio/ogg"));
        assert_ne!(rule.size_bytes, audio.len() as u64);
        "audio/ogg"
    };
    let authority = session.authorities.foreground_authority.to_string();
    assert_audio_route(&foreground, &authority, audio_path, &audio, mime).await;
    let mut stylesheet = imported_publication_request("GET", "/OPS/css/stylesheet.css", &authority);
    stylesheet
        .headers_mut()
        .insert("sec-fetch-dest", HeaderValue::from_static("audio"));
    let stylesheet = foreground.clone().oneshot(stylesheet).await.unwrap();
    assert_eq!(
        stylesheet.headers()["cross-origin-resource-policy"],
        "same-origin"
    );

    let session_id = session.reader_session_id.to_string();
    let overlay_request = |path: String, token: &str| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("host", "reader.test")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header("x-elephant-request", "trusted-reader")
            .body(Body::empty())
            .unwrap()
    };
    let base = format!("/native/reader-sessions/{session_id}/media-overlays");
    let description = reader
        .clone()
        .oneshot(overlay_request(base.clone(), "secret"))
        .await
        .unwrap();
    assert_eq!(description.status(), StatusCode::OK);
    assert_eq!(description.headers()[CACHE_CONTROL], "no-store");
    let description: serde_json::Value =
        serde_json::from_slice(&description.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(description["playback"]["state"], "available");
    let overlay_count = description["playback"]["overlays"]
        .as_array()
        .unwrap()
        .len();
    let timeline = reader
        .clone()
        .oneshot(overlay_request(format!("{base}/0"), "secret"))
        .await
        .unwrap();
    assert_eq!(timeline.status(), StatusCode::OK);
    let timeline: serde_json::Value =
        serde_json::from_slice(&timeline.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(timeline["state"], "available");
    assert_eq!(timeline["timeline"]["overlayIndex"], 0);
    for (path, token, status) in [
        (
            format!("{base}/{overlay_count}"),
            "secret",
            StatusCode::NOT_FOUND,
        ),
        (format!("{base}/+0"), "secret", StatusCode::NOT_FOUND),
        (base.clone(), "wrong", StatusCode::UNAUTHORIZED),
        (
            format!("/native/reader-sessions/{}/media-overlays", "0".repeat(64)),
            "secret",
            StatusCode::CONFLICT,
        ),
        (
            "/native/reader-sessions/short/media-overlays".to_owned(),
            "secret",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = reader
            .clone()
            .oneshot(overlay_request(path.clone(), token))
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
    }
    session.revoked.store(true, Ordering::Release);
    assert_eq!(
        reader
            .clone()
            .oneshot(overlay_request(base, "secret"))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let storage = storage.lock().unwrap();
    assert!(matches!(
        storage
            .select_publication(&id, crate::PRODUCTION_READING_REVISION, None)
            .unwrap(),
        crate::storage::PublicationSelection::Ready(_)
    ));
    std::fs::remove_file(&session.entry.media_sidecars[audio_path].path).unwrap();
    assert!(matches!(
        storage
            .select_publication(&id, crate::PRODUCTION_READING_REVISION, None)
            .unwrap(),
        crate::storage::PublicationSelection::NeedsDerivation(_)
    ));
}

#[tokio::test]
async fn every_media_overlay_gate_outcome_keeps_the_publication_readable() {
    use crate::media_overlay::MediaOverlayCapability;
    use crate::media_overlay::derive::tests::{
        absent_epub, malformed_missing_text_epub, playable_epub, text_only_epub,
    };

    for (name, archive, playback) in [
        ("absent", absent_epub(), "absent"),
        ("malformed", malformed_missing_text_epub(), "unavailable"),
        ("text-only", text_only_epub(), "unavailable"),
        ("playable", playable_epub(), "available"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let state = fixture_reader_state(directory.path());
        let storage = state.storage.clone();
        let active = state.lifecycle.active_view();
        let reader = reader_router(state, None);
        let id = import_and_open(&reader, archive, 1).await;
        let session = active.load().unwrap();
        match playback {
            "absent" => assert!(
                matches!(
                    session.entry.media_overlay_capability,
                    MediaOverlayCapability::Absent {}
                ),
                "{name}"
            ),
            "unavailable" => assert!(
                matches!(
                    session.entry.media_overlay_capability,
                    MediaOverlayCapability::Unavailable {}
                ),
                "{name}"
            ),
            _ => assert!(
                matches!(
                    session.entry.media_overlay_capability,
                    MediaOverlayCapability::Available { .. }
                ),
                "{name}"
            ),
        }

        let base = format!(
            "/native/reader-sessions/{}/media-overlays",
            session.reader_session_id
        );
        let description = reader
            .clone()
            .oneshot(media_overlay_request(base))
            .await
            .unwrap();
        assert_eq!(description.status(), StatusCode::OK, "{name}");
        let description: serde_json::Value =
            serde_json::from_slice(&description.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(description["playback"]["state"], playback, "{name}");

        let storage = storage.lock().unwrap();
        assert!(
            matches!(
                storage
                    .select_publication(&id, crate::PRODUCTION_READING_REVISION, None)
                    .unwrap(),
                crate::storage::PublicationSelection::Ready(_)
            ),
            "{name}"
        );
    }
}

#[tokio::test]
async fn replacing_the_reader_session_fences_the_previous_narration() {
    use crate::media_overlay::derive::tests::{absent_epub, playable_epub};

    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let foreground = crate::mount::imported_publication_router(ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    });
    let reader = reader_router(state, None);
    let audio_path = "/EPUB/audio/chapter.mp3";

    let playable = import_publication(&reader, playable_epub()).await;
    let absent = import_publication(&reader, absent_epub()).await;
    open_publication(&reader, &playable, 1).await;
    let first = active.load().unwrap();
    let first_authority = first.authorities.foreground_authority.to_string();
    let first_base = format!(
        "/native/reader-sessions/{}/media-overlays",
        first.reader_session_id
    );
    let audio = foreground
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            audio_path,
            &first_authority,
        ))
        .await
        .unwrap();
    assert_eq!(audio.status(), StatusCode::OK);

    open_publication(&reader, &absent, 2).await;
    let second = active.load().unwrap();
    assert_ne!(second.reader_session_id, first.reader_session_id);
    for path in [first_base.clone(), format!("{first_base}/0")] {
        assert_eq!(
            reader
                .clone()
                .oneshot(media_overlay_request(path.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT,
            "{path}"
        );
    }
    let second_authority = second.authorities.foreground_authority.to_string();
    let stale_authority = if first_authority == second_authority {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    };
    assert_eq!(
        foreground
            .clone()
            .oneshot(imported_publication_request(
                "GET",
                audio_path,
                &first_authority,
            ))
            .await
            .unwrap()
            .status(),
        stale_authority
    );
    assert_eq!(
        foreground
            .clone()
            .oneshot(imported_publication_request(
                "GET",
                audio_path,
                &second_authority,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_older_reading_revision_rebuilds_playable_sidecars_on_open() {
    let audio =
        std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-excerpt.m4a").unwrap();
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language>
    <meta property="media:duration">0:00:02</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
    <item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
    <item id="audio" href="audio/chapter.m4a" media-type="audio/mp4"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#;
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" version="3.0">
  <body><par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.m4a" clipBegin="0s" clipEnd="2s"/></par></body>
</smil>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p></body></html>"#;
    let audio_path = "EPUB/audio/chapter.m4a";
    let archive = synthetic_epub(
        package,
        &[
            ("EPUB/nav.xhtml", nav),
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/overlay.smil", smil),
            (audio_path, &audio),
        ],
    );
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let foreground = crate::mount::imported_publication_router(ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    });
    let reader = reader_router(state, None);
    let id = import_publication(&reader, archive).await;

    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "UPDATE publication_snapshots SET reading_revision = 'reading-v13'
                 WHERE publication_id = ?1",
            [&id],
        )
        .unwrap();
    drop(connection);

    open_publication(&reader, &id, 1).await;
    let session = active.load().unwrap();
    assert!(matches!(
        session.entry.media_overlay_capability,
        crate::media_overlay::MediaOverlayCapability::Available { .. }
    ));
    let sidecar = &session.entry.media_sidecars[audio_path];
    let bytes = std::fs::read(&sidecar.path).unwrap();
    assert_eq!(bytes.len() as u64, sidecar.length);
    let mime = if cfg!(any(target_os = "android", target_os = "ios")) {
        assert_eq!(bytes, audio);
        assert_eq!(sidecar.media_type, None);
        "audio/mp4"
    } else {
        assert_eq!(&bytes[..4], b"OggS");
        assert_eq!(sidecar.media_type.as_deref(), Some("audio/ogg"));
        "audio/ogg"
    };
    let stored_media_type: Option<String> =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE))
            .unwrap()
            .query_row(
                "SELECT media_overlay_sidecars.media_type FROM media_overlay_sidecars
                     JOIN publication_snapshots USING (snapshot_id)
                     WHERE publication_snapshots.publication_id = ?1
                       AND publication_snapshots.reading_revision = ?2
                       AND media_overlay_sidecars.canonical_path = ?3",
                rusqlite::params![id, crate::PRODUCTION_READING_REVISION, audio_path],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(stored_media_type, sidecar.media_type);

    let authority = session.authorities.foreground_authority.to_string();
    let response = foreground
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            &format!("/{audio_path}"),
            &authority,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], mime);
}

#[tokio::test]
async fn content_document_audio_the_engine_cannot_play_is_converted_too() {
    use axum::http::header::CONTENT_TYPE;

    const EXCERPT: &[u8] =
        include_bytes!("../../../../../tests/fixtures/media-overlays/moby-dick-excerpt.m4a");

    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="uid">content-audio</dc:identifier><dc:title>Content audio</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-10-05T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="clip" href="audio/clip.m4a" media-type="audio/mp4"/></manifest><spine><itemref idref="chapter"/></spine></package>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><audio src="audio/clip.m4a" controls="controls"></audio></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let archive = synthetic_epub(
        package,
        &[
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/audio/clip.m4a", EXCERPT),
        ],
    );

    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let foreground = crate::mount::imported_publication_router(ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    });
    let reader = reader_router(state, None);
    let created = reader
        .clone()
        .oneshot(import_request_with(archive))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = created["publication"]["publicationId"].as_str().unwrap();
    assert_eq!(
        reader.oneshot(open_request(id)).await.unwrap().status(),
        StatusCode::OK
    );
    let session = active.load().unwrap();
    let authority = session.authorities.foreground_authority.as_ref();

    // This publication has no media overlays at all, so nothing about narration applies.
    let audio_path = "EPUB/audio/clip.m4a";
    assert_eq!(
        session.entry.responses.resources[audio_path].mime,
        "audio/mp4"
    );

    let served = foreground
        .oneshot(imported_publication_request(
            "GET",
            "/EPUB/audio/clip.m4a",
            authority,
        ))
        .await
        .unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    let body = served.headers()[CONTENT_TYPE].clone();
    let bytes = served.into_body().collect().await.unwrap().to_bytes();
    if cfg!(any(target_os = "android", target_os = "ios")) {
        // The Android System WebView decodes AAC, so the archive entry is served as is.
        assert_eq!(body, "audio/mp4");
        assert_eq!(bytes.as_ref(), EXCERPT);
        assert!(!session.entry.media_sidecars.contains_key(audio_path));
    } else {
        // CEF decodes no AAC, so the content document is handed Ogg Opus instead.
        assert_eq!(body, "audio/ogg");
        assert_eq!(&bytes[..4], b"OggS");
        let sidecar = &session.entry.media_sidecars[audio_path];
        assert_eq!(sidecar.media_type.as_deref(), Some("audio/ogg"));
        assert_eq!(bytes.len() as u64, sidecar.length);
    }
}

#[tokio::test]
async fn mp3_sidecar_audio_serves_ranges_from_stored_and_deflated_entries() {
    let audio =
        std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-excerpt.mp3").unwrap();
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata><dc:title>T</dc:title><dc:identifier id="uid">id</dc:identifier><dc:language>en</dc:language>
    <meta property="media:duration">0:00:06</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chap" href="chapter.xhtml" media-type="application/xhtml+xml" media-overlay="overlay"/>
    <item id="overlay" href="overlay.smil" media-type="application/smil+xml"/>
    <item id="audio" href="audio/chapter.mp3" media-type="audio/mpeg"/>
  </manifest>
  <spine><itemref idref="chap"/></spine>
</package>"#;
    let smil = br#"<smil xmlns="http://www.w3.org/ns/SMIL" version="3.0">
  <body>
    <par id="par1"><text src="chapter.xhtml#s1"/><audio src="audio/chapter.mp3" clipBegin="0s" clipEnd="3s"/></par>
    <par id="par2"><text src="chapter.xhtml#s2"/><audio src="audio/chapter.mp3" clipBegin="3s" clipEnd="6s"/></par>
  </body>
</smil>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p id="s1">Call me Ishmael.</p><p id="s2">Some years ago.</p></body></html>"#;
    let audio_path = "EPUB/audio/chapter.mp3";
    for compression in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let archive = synthetic_epub_with(
            package,
            &[
                ("EPUB/nav.xhtml", nav),
                ("EPUB/chapter.xhtml", chapter),
                ("EPUB/overlay.smil", smil),
                (audio_path, &audio),
            ],
            compression,
        );
        let directory = tempfile::tempdir().unwrap();
        let state = fixture_reader_state(directory.path());
        let active = state.lifecycle.active_view();
        let foreground = crate::mount::imported_publication_router(ImportedPublicationState {
            asset_origin: Arc::from(TEST_READER_ORIGIN),
            active_reader_session: active.clone(),
            snapshots: state.snapshots.clone(),
            stream_admission: Arc::new(Semaphore::new(1)),
            transform_admission: Arc::new(Semaphore::new(1)),
        });
        let reader = reader_router(state, None);
        let created = reader
            .clone()
            .oneshot(import_request_with(archive))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED, "{compression:?}");
        let created: serde_json::Value =
            serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let id = created["publication"]["publicationId"].as_str().unwrap();
        assert_eq!(
            reader
                .clone()
                .oneshot(open_request(id))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let session = active.load().unwrap();
        assert!(matches!(
            session.entry.media_overlay_capability,
            crate::media_overlay::MediaOverlayCapability::Available { .. }
        ));
        let sidecar = &session.entry.media_sidecars[audio_path];
        assert_eq!(sidecar.media_type, None);
        assert_eq!(std::fs::read(&sidecar.path).unwrap(), audio);
        assert_eq!(
            session.entry.responses.resources[audio_path].mime,
            "audio/mpeg"
        );
        let authority = session.authorities.foreground_authority.to_string();
        assert_audio_route(&foreground, &authority, audio_path, &audio, "audio/mpeg").await;

        std::fs::OpenOptions::new()
            .write(true)
            .open(&sidecar.path)
            .unwrap()
            .set_len(audio.len() as u64 / 2)
            .unwrap();
        let truncated = foreground
            .clone()
            .oneshot(imported_publication_request(
                "GET",
                &format!("/{audio_path}"),
                &authority,
            ))
            .await
            .unwrap();
        assert_eq!(truncated.status(), StatusCode::OK);
        assert!(truncated.into_body().collect().await.is_err());
    }
}

#[test]
fn exhausted_sidecar_budget_leaves_the_publication_readable() {
    let bytes = std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-mo.epub").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let derived = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        0,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    assert!(derived.prepared_sidecars.is_empty());
    assert!(matches!(
        derived.media_overlay.playback,
        crate::media_overlay::MediaOverlayPlayback::Unavailable { .. }
    ));
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[test]
fn a_rederived_publication_reuses_its_installed_transcode() {
    let bytes = std::fs::read("../../../tests/fixtures/media-overlays/moby-dick-mo.epub").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut storage = crate::storage::Storage::open(directory.path()).unwrap();
    let blob = storage.install_blob(&bytes).unwrap();
    let first = crate::snapshot::derive_publication(
        &blob.path,
        directory.path(),
        u64::MAX,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    assert_eq!(first.metrics.sidecar_transcoded_count, 1);
    let storage_name = first.prepared_sidecars[0].storage_name.clone();
    let imported = storage
        .commit_import_snapshot_with_policy_relevance(
            crate::storage::InstalledImport {
                archive: &blob,
                cover: None,
            },
            crate::PRODUCTION_READING_REVISION,
            "Moby",
            crate::storage::PolicyAwareReadingProducts {
                facts: &first.facts,
                package: &first.package,
                responses: &first.responses,
                media_overlay: &first.media_overlay,
                media_overlay_timelines: &first.media_overlay_timelines,
                prepared_sidecars: first.prepared_sidecars,
                policy_relevance: first.policy_relevance,
            },
        )
        .unwrap();

    let reusable = storage
        .reusable_media_transcodes(crate::media_overlay::backing::transcode_derivation())
        .unwrap();
    let second =
        crate::snapshot::derive_publication(&blob.path, directory.path(), u64::MAX, &reusable)
            .unwrap();
    assert_eq!(second.metrics.sidecar_transcoded_count, 0);
    assert_eq!(
        second.prepared_sidecars[0].origin,
        crate::media_overlay::backing::SidecarOrigin::Reused
    );
    assert_eq!(second.prepared_sidecars[0].storage_name, storage_name);

    let crate::storage::PublicationSelection::Ready(selected) = storage
        .select_publication(
            &imported.publication.publication_id,
            crate::PRODUCTION_READING_REVISION,
            None,
        )
        .unwrap()
    else {
        panic!("transcoded publication must be ready");
    };
    assert_eq!(
        selected.media_sidecars["OPS/audio/mobydick_001_002_melville.mp4"]
            .media_type
            .as_deref(),
        Some("audio/ogg")
    );
}

#[test]
fn reader_csp_admits_only_listed_media_origins() {
    let csp = crate::reader::reader_csp(
        &["http://publication.test", "http://measurement.test"],
        &["http://publication.test"],
        &["http://publication.test"],
    );
    let media = csp
        .split(';')
        .map(str::trim)
        .find(|directive| directive.starts_with("media-src"))
        .unwrap();
    assert_eq!(media, "media-src 'self' blob: http://publication.test");
    assert!(crate::reader::reader_csp(&[], &[], &[]).contains("media-src 'self' blob:;"));
}

#[test]
fn declared_media_mime_is_normalized_for_audio_and_video_only() {
    let normalize = |value: &str| {
        crate::snapshot::normalized_media_mime(
            &epub_stack::resource::MediaType::new(value).unwrap(),
        )
    };
    assert_eq!(normalize("Audio/MP4").as_deref(), Some("audio/mp4"));
    assert_eq!(
        normalize("audio/mp4; Codecs=mp4a.40.2; a=b").as_deref(),
        Some("audio/mp4; a=b; codecs=mp4a.40.2")
    );
    assert_eq!(normalize("video/webm").as_deref(), Some("video/webm"));
    assert_eq!(normalize("text/css"), None);
    assert_eq!(normalize("audio/mp4; codecs=\"a,b\""), None);
}
