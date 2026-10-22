use super::support::*;

#[test]
fn concrete_bootstrap_response_shapes_are_exact() {
    let preferences = crate::storage::ReaderPreferences::default();
    let revisions = crate::storage::PreferenceRevisions {
        global: 1,
        publication: 2,
    };
    let library = crate::reader::LibraryBootstrapResponse {
        display: crate::DisplayCapabilities::default(),
        mode: "library",
        library_document_id: "document".to_owned(),
        reader_session_id: "session".to_owned(),
        launch_id: "launch".to_owned(),
        reader_origin: "http://reader.test".to_owned(),
        asset_origin: "http://reader.test".to_owned(),
        session_token: "token".to_owned(),
        tauri_ipc_enabled: false,
        native_actions: crate::device::NativeActionsAvailability {
            front_light_dialog: false,
            side_buttons: false,
            window_fullscreen: false,
        },
        preferences: preferences.clone(),
        preference_revisions: revisions.clone(),
        reader_typefaces: crate::reader::READER_TYPEFACES.to_vec(),
        diagnostics_level: crate::DiagnosticsLevel::Summary,
        dictionaries: crate::reader::DictionariesSummary {
            installed: 0,
            operation_active: false,
        },
        recovery_navigation_path: "/recover".to_owned(),
        application: crate::reader::ApplicationIdentity {
            name: "test".to_owned(),
            version: "0.0.0-test",
        },
    };
    let annotations = crate::storage::AnnotationProjection {
        revision: 0,
        records: Vec::new(),
    };
    let reading = crate::reader::ReadingBootstrapResponse {
        display: crate::DisplayCapabilities::default(),
        mode: "reading",
        library_document_id: None,
        reader_session_id: "session".to_owned(),
        launch_id: "launch".to_owned(),
        reader_origin: "http://reader.test".to_owned(),
        publication_origin: "http://publication.test".to_owned(),
        asset_origin: "http://reader.test".to_owned(),
        measurement_origin: "http://measurement.test".to_owned(),
        session_token: "token".to_owned(),
        tauri_ipc_enabled: false,
        native_actions: crate::device::NativeActionsAvailability {
            front_light_dialog: false,
            side_buttons: false,
            window_fullscreen: false,
        },
        publication_id: "publication".to_owned(),
        publication: serde_json::json!({"facts": {}, "mount": {}}),
        initial_reading_position: Some(crate::ReadingPosition {
            target_json: "{\"source\":\"EPUB/chapter.xhtml\"}".to_owned(),
            occurrence_hint: None,
            roll_progression: Some(0.375),
        }),
        position_revision: 1,
        annotations: annotations.public(),
        annotation_identifiers: vec!["urn:blake3:ab".to_owned()],
        preferences,
        preference_revisions: revisions,
        policy: crate::policy::ResolvedPublicationPolicy {
            active_content: crate::ResolvedActiveContentPolicy::Disabled,
            remote_resources: crate::ResolvedRemoteResourcesPolicy::Blocked,
        },
        reader_typefaces: crate::reader::READER_TYPEFACES.to_vec(),
        pagination: serde_json::json!({"state": "unavailable"}),
        media_overlay: crate::media_overlay::MediaOverlayCapability::Absent {},
        dictionary: None,
        diagnostics_level: crate::DiagnosticsLevel::Trace,
        recovery_navigation_path: "/recover".to_owned(),
    };

    assert_json_keys(
        &serde_json::from_str(&crate::reader::serialize_bootstrap(&library).unwrap()).unwrap(),
        &[
            "application",
            "assetOrigin",
            "diagnosticsLevel",
            "dictionaries",
            "display",
            "launchId",
            "libraryDocumentId",
            "mode",
            "nativeActions",
            "preferenceRevisions",
            "preferences",
            "readerOrigin",
            "readerTypefaces",
            "readerSessionId",
            "recoveryNavigationPath",
            "sessionToken",
            "tauriIpcEnabled",
        ],
    );
    let reading: serde_json::Value =
        serde_json::from_str(&crate::reader::serialize_bootstrap(&reading).unwrap()).unwrap();
    assert_eq!(
        reading["initialReadingPosition"],
        serde_json::json!({
            "targetJson": "{\"source\":\"EPUB/chapter.xhtml\"}",
            "occurrenceHint": null,
            "rollProgression": 0.375
        })
    );
    assert_json_keys(
        &reading,
        &[
            "annotationIdentifiers",
            "annotations",
            "assetOrigin",
            "diagnosticsLevel",
            "dictionary",
            "display",
            "initialReadingPosition",
            "launchId",
            "libraryDocumentId",
            "measurementOrigin",
            "mediaOverlay",
            "mode",
            "nativeActions",
            "pagination",
            "policy",
            "positionRevision",
            "preferenceRevisions",
            "preferences",
            "publication",
            "publicationId",
            "publicationOrigin",
            "readerOrigin",
            "readerTypefaces",
            "readerSessionId",
            "recoveryNavigationPath",
            "sessionToken",
            "tauriIpcEnabled",
        ],
    );
    assert_eq!(
        reading["annotations"],
        serde_json::json!({"revision": 0, "records": []})
    );
    assert_eq!(
        crate::reader::serialize_bootstrap(&serde_json::json!({"text": "<tag>"})).unwrap(),
        r#"{"text":"\u003ctag\u003e"}"#
    );
}

#[tokio::test(flavor = "current_thread")]
async fn trusted_navigation_rotates_single_use_recovery_tickets() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    *state
        .library_document_id
        .write()
        .expect("library document lock poisoned") = None;
    let library_document_id = state.library_document_id.clone();
    let router = reader_router(state, None);

    let default = router
        .clone()
        .oneshot(reader_navigation_request("/missing"))
        .await
        .unwrap();
    assert_eq!(default.status(), StatusCode::NOT_FOUND);
    let default_csp = default.headers()[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(default_csp.contains("img-src 'self' data: blob:;"));
    assert!(default_csp.contains("frame-src 'none';"));

    let first = router
        .clone()
        .oneshot(reader_navigation_request("/window/launch-nonce/start"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_csp = first.headers()[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(first_csp.contains("img-src 'self' data: blob:;"));
    assert!(first_csp.contains("frame-src 'none';"));
    let first = bootstrap_from_document(first).await;
    assert_eq!(first["mode"], "library");
    assert_eq!(first["launchId"], "0123456789abcdef0123456789abcdef");
    assert_eq!(
        first["readerTypefaces"],
        serde_json::json!(["reader-serif", "reader-sans"])
    );
    assert_eq!(first["preferences"]["publicationTypeface"], "reader-serif");
    let first_document_id = first["libraryDocumentId"].as_str().unwrap();
    let first_recovery = first["recoveryNavigationPath"].as_str().unwrap();
    assert_eq!(
        library_document_id
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_ref(),
        first_document_id
    );

    assert_eq!(
        router
            .clone()
            .oneshot(reader_navigation_request("/window/launch-nonce/start"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        router
            .clone()
            .oneshot(reader_navigation_request("/"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let second = router
        .clone()
        .oneshot(reader_navigation_request(first_recovery))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = bootstrap_from_document(second).await;
    assert_eq!(second["mode"], "library");
    assert_eq!(second["launchId"], first["launchId"]);
    assert_ne!(second["libraryDocumentId"], first["libraryDocumentId"]);
    assert_ne!(
        second["recoveryNavigationPath"],
        first["recoveryNavigationPath"]
    );
    assert_eq!(
        router
            .oneshot(reader_navigation_request(first_recovery))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_window_recreation_preserves_the_foreground_origin() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let navigation_policy = crate::ContentNavigationPolicy {
        active_reader_session: state.lifecycle.active_view(),
    };
    let recovery = crate::NavigationRecovery {
        reader_origin: state.reader_origin.clone(),
        lifecycle: state.lifecycle.clone(),
        imported_publication_port: state.imported_publication_port,
        nested_publication_port: state.nested_publication_port,
        measurement_port: state.measurement_port,
    };
    let router = reader_router(state, None);
    let imported = router.clone().oneshot(import_request()).await.unwrap();
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let opened = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let before = pending_reading_bootstrap(&lifecycle);
    let before_session = before["readerSessionId"].as_str().unwrap().to_owned();
    let original_session = recovery.lifecycle.load_active().unwrap();
    let before_nested = original_session.authorities.nested_origin.to_string();
    let before_publication = before["publicationOrigin"].as_str().unwrap().to_owned();
    let before_measurement = before["measurementOrigin"].as_str().unwrap().to_owned();
    assert!(navigation_policy.allows_origin(&before_publication));
    assert!(navigation_policy.allows_origin(&before_nested));
    assert!(navigation_policy.allows_origin(&before_measurement));

    let recovery_url = recovery.recreate_url().await.unwrap();
    assert!(original_session.revoked.load(Ordering::Acquire));
    let after = pending_reading_bootstrap(&lifecycle);
    let replacement_session = recovery.lifecycle.load_active().unwrap();
    let after_nested = replacement_session.authorities.nested_origin.to_string();
    assert_ne!(after["readerSessionId"], before_session);
    assert_eq!(after["publicationOrigin"], before_publication);
    assert_ne!(after["measurementOrigin"], before_measurement);
    assert_ne!(after_nested, before_nested);
    assert!(navigation_policy.allows_origin(&before_publication));
    assert!(!navigation_policy.allows_origin(&before_nested));
    assert!(!navigation_policy.allows_origin(&before_measurement));
    assert!(navigation_policy.allows_origin(after["publicationOrigin"].as_str().unwrap()));
    assert!(navigation_policy.allows_origin(&after_nested));
    assert!(navigation_policy.allows_origin(after["measurementOrigin"].as_str().unwrap()));

    let path = recovery_url
        .strip_prefix(TEST_READER_ORIGIN)
        .unwrap()
        .to_owned();
    let recreated = router
        .oneshot(reader_navigation_request(&path))
        .await
        .unwrap();
    assert_eq!(recreated.status(), StatusCode::OK);
    let recreated_csp = recreated.headers()[CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    let after_publication = after["publicationOrigin"].as_str().unwrap();
    let after_measurement = after["measurementOrigin"].as_str().unwrap();
    assert!(recreated_csp.contains(&format!("img-src 'self' data: blob: {after_publication};")));
    assert!(recreated_csp.contains(&format!(
        "frame-src {after_publication} {after_measurement};"
    )));
    assert!(recreated_csp.contains(&before_publication));
    assert!(!recreated_csp.contains(&before_measurement));
    let recreated = bootstrap_from_document(recreated).await;
    assert_eq!(recreated["mode"], "reading");
    assert_eq!(recreated["readerSessionId"], after["readerSessionId"]);
}

#[tokio::test(flavor = "current_thread")]
async fn bootstrap_reports_front_light_dialog_availability() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestDisplayActions::default());
    let response = reader_router(
        fixture_reader_state_with_actions(directory.path(), actions),
        None,
    )
    .oneshot(
        Request::builder()
            .uri("/window/launch-nonce/start")
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains(
        r#""nativeActions":{"frontLightDialog":true,"sideButtons":false,"windowFullscreen":true}"#
    ));
    assert!(body.contains(r#""display":{"technology":"unknown"}"#));
    assert!(body.contains(r#""diagnosticsLevel":"trace""#));
}

#[tokio::test(flavor = "current_thread")]
async fn bootstrap_reports_eink_without_front_light_dialog() {
    struct EinkActions;
    impl NativeActions for EinkActions {
        fn display_capabilities(&self) -> crate::DisplayCapabilities {
            crate::DisplayCapabilities {
                technology: crate::DisplayTechnology::Eink,
                ..Default::default()
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let response = reader_router(
        fixture_reader_state_with_actions(directory.path(), Arc::new(EinkActions)),
        None,
    )
    .oneshot(
        Request::builder()
            .uri("/window/launch-nonce/start")
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(r#""display":{"technology":"eink"}"#));
    assert!(body.contains(
        r#""nativeActions":{"frontLightDialog":false,"sideButtons":false,"windowFullscreen":false}"#
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn bootstrap_carries_what_the_eink_device_reports_about_itself() {
    struct ReportingActions;
    impl NativeActions for ReportingActions {
        fn display_capabilities(&self) -> crate::DisplayCapabilities {
            crate::DisplayCapabilities {
                technology: crate::DisplayTechnology::Eink,
                refresh_mode: Some("regal_plus".to_string()),
                speed_mode_while_dragging: Some(true),
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let response = reader_router(
        fixture_reader_state_with_actions(directory.path(), Arc::new(ReportingActions)),
        None,
    )
    .oneshot(
        Request::builder()
            .uri("/window/launch-nonce/start")
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains(
        r#""display":{"technology":"eink","refreshMode":"regal_plus","speedModeWhileDragging":true}"#
    ));
}
