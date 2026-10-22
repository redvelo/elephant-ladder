use super::support::*;

#[test]
fn live_pagination_keeps_measurement_and_preparation_independent() {
    let responses = ResponseManifest {
        undeclared_references: Vec::new(),
        resources: BTreeMap::from([
            (
                "EPUB/fixed.svg".to_owned(),
                ResponseRule {
                    mime: "image/svg+xml".to_owned(),
                    size_bytes: 10,
                    missing: false,
                    undeclared: false,
                    fallback_source: None,
                    foreground: Foreground::Unchanged,
                    foreground_preparation: ForegroundPreparationState::Eligible,
                    measurement: Measurement::Unchanged,
                },
            ),
            (
                "EPUB/text.xhtml".to_owned(),
                ResponseRule {
                    mime: "application/xhtml+xml".to_owned(),
                    size_bytes: 10,
                    missing: false,
                    undeclared: false,
                    fallback_source: None,
                    foreground: Foreground::Xhtml,
                    foreground_preparation: ForegroundPreparationState::Ineligible,
                    measurement: Measurement::InstrumentedXhtml,
                },
            ),
        ]),
    };

    let descriptor = live_pagination_descriptor(
        &responses,
        "http://measurement.test",
        "pagination-handle-for-test-0000000000000000",
    );
    let rules = serde_json::to_value(&responses).unwrap();
    let xhtml_rule = rules["resources"]["EPUB/text.xhtml"].as_object().unwrap();
    assert_eq!(
        xhtml_rule.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "foreground",
            "foregroundPreparation",
            "measurement",
            "mime",
            "sizeBytes"
        ]
    );
    assert_eq!(
        descriptor["responses"]["eligibleResourcePaths"],
        serde_json::json!(["EPUB/text.xhtml"])
    );
    assert_eq!(
        descriptor["foregroundPreparation"]["eligibleResourcePaths"],
        serde_json::json!(["EPUB/fixed.svg"])
    );
    assert_eq!(
        descriptor["cache"]["handle"],
        "pagination-handle-for-test-0000000000000000"
    );
    assert_eq!(
        descriptor["cache"]["measurementArtifactDigest"]
            .as_str()
            .map(str::len),
        Some(64)
    );
}

#[test]
fn preparation_jobs_join_and_only_newer_operations_retry_failure() {
    let registry = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let (first, first_participation) = crate::session::preparation_job(&registry, "book", 7);
    assert_eq!(
        first_participation,
        crate::session::PreparationParticipation::Started
    );

    let (joined, joined_participation) = crate::session::preparation_job(&registry, "book", 7);
    assert_eq!(
        joined_participation,
        crate::session::PreparationParticipation::Joined
    );
    assert!(Arc::ptr_eq(&first, &joined));

    first
        .completion
        .send_replace(crate::session::PreparationJobState::Failed);
    let (retained, retained_participation) = crate::session::preparation_job(&registry, "book", 7);
    assert_eq!(
        retained_participation,
        crate::session::PreparationParticipation::Joined
    );
    assert!(Arc::ptr_eq(&first, &retained));

    let (replacement, replacement_participation) =
        crate::session::preparation_job(&registry, "book", 8);
    assert_eq!(
        replacement_participation,
        crate::session::PreparationParticipation::Started
    );
    assert!(!Arc::ptr_eq(&first, &replacement));
    first
        .completion
        .send_replace(crate::session::PreparationJobState::Ready);
    assert!(Arc::ptr_eq(
        registry.lock().unwrap().get("book").unwrap(),
        &replacement
    ));
    assert_eq!(
        *replacement.completion.borrow(),
        crate::session::PreparationJobState::Running
    );
}
#[tokio::test(flavor = "current_thread")]
async fn open_requires_both_policy_decisions_before_creating_a_session() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state_with_actions(
        directory.path(),
        Arc::new(crate::UnsupportedNativeActions),
    );
    let active_reader_session = state.lifecycle.active_view();
    let router = reader_router(state, None);
    let imported = router
        .clone()
        .oneshot(import_request_with(REMOTE_POLICY_BYTES.to_vec()))
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();

    let decision = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(decision.status(), StatusCode::CONFLICT);
    let decision: serde_json::Value =
        serde_json::from_slice(&decision.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(decision["status"], "policy-decision-required");
    assert_eq!(decision["required"]["activeContent"], true);
    assert_eq!(decision["required"]["remoteResources"], true);
    assert_eq!(decision["relevance"]["activeContent"], "detected");
    assert_eq!(decision["relevance"]["remoteResources"], "detected");
    assert!(active_reader_session.load().is_none());

    let saved = router
        .clone()
        .oneshot(publication_policy_request(
            "PUT",
            format!("/native/publications/{publication_id}/policy"),
            Some(serde_json::json!({
                "baseRevisions": { "global": 0, "publication": 0 },
                "policy": {
                    "activeContent": "disabled",
                    "remoteResources": "blocked"
                }
            })),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        router
            .oneshot(open_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let session = active_reader_session.load().unwrap();
    assert_eq!(
        session.policy,
        crate::ResolvedPublicationPolicy {
            active_content: crate::ResolvedActiveContentPolicy::Disabled,
            remote_resources: crate::ResolvedRemoteResourcesPolicy::Blocked,
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn active_content_first_open_decisions_persist_without_repeat_prompts() {
    for remote in [false, true] {
        for choice in ["enabled", "disabled"] {
            let package = format!(
                r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata><title xmlns="http://purl.org/dc/elements/1.1/">Policy</title><identifier xmlns="http://purl.org/dc/elements/1.1/">policy</identifier><meta property="dcterms:modified">2026-09-12T00:00:00Z</meta></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml" properties="scripted{}"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
                if remote { " remote-resources" } else { "" }
            );
            let bytes = synthetic_epub(
                    package.as_bytes(),
                    &[(
                        "EPUB/chapter.xhtml",
                        br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Policy</title><script>window.test = true;</script></head><body><p>Policy</p></body></html>"#,
                    )],
                );
            let directory = tempfile::tempdir().unwrap();
            let state = fixture_reader_state_with_actions(
                directory.path(),
                Arc::new(crate::UnsupportedNativeActions),
            );
            let active = state.lifecycle.active_view();
            let search_indexing = state.search_indexing.clone();
            let router = reader_router(state, None);
            let imported = router
                .clone()
                .oneshot(import_request_with(bytes))
                .await
                .unwrap();
            assert_eq!(imported.status(), StatusCode::CREATED);
            let imported: serde_json::Value =
                serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            let id = imported["publication"]["publicationId"].as_str().unwrap();
            let decision = router.clone().oneshot(open_request(id)).await.unwrap();
            assert_eq!(decision.status(), StatusCode::CONFLICT);
            let decision: serde_json::Value =
                serde_json::from_slice(&decision.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(decision["status"], "policy-decision-required");
            assert_eq!(decision["required"]["activeContent"], true);
            assert_eq!(decision["required"]["remoteResources"], remote);
            assert!(active.load().is_none());

            // A partial decision must not create a session while remote consent is pending.
            for revision in 0..=u32::from(remote) {
                let saved = router
                        .clone()
                        .oneshot(publication_policy_request(
                            "PUT",
                            format!("/native/publications/{id}/policy"),
                            Some(serde_json::json!({
                                "baseRevisions": { "global": 0, "publication": revision },
                                "policy": {
                                    "activeContent": choice,
                                    "remoteResources": if revision == 0 { None } else { Some("blocked") }
                                }
                            })),
                            true,
                            true,
                        ))
                        .await
                        .unwrap();
                assert_eq!(saved.status(), StatusCode::OK);
                assert!(active.load().is_none());
                if remote && revision == 0 {
                    let pending = router.clone().oneshot(open_request(id)).await.unwrap();
                    assert_eq!(pending.status(), StatusCode::CONFLICT);
                    let pending: serde_json::Value = serde_json::from_slice(
                        &pending.into_body().collect().await.unwrap().to_bytes(),
                    )
                    .unwrap();
                    assert_eq!(pending["required"]["activeContent"], false);
                    assert_eq!(pending["required"]["remoteResources"], true);
                    assert!(active.load().is_none());
                }
            }
            drop(router);
            wait_for_search_indexing(&search_indexing).await;

            // Recreate the host twice to verify persisted choices, not session-local consent.
            for _ in 0..2 {
                let state = fixture_reader_state_with_actions(
                    directory.path(),
                    Arc::new(crate::UnsupportedNativeActions),
                );
                let active = state.lifecycle.active_view();
                let search_indexing = state.search_indexing.clone();
                assert!(active.load().is_none());
                let opened = reader_router(state, None)
                    .oneshot(open_request(id))
                    .await
                    .unwrap();
                assert_eq!(opened.status(), StatusCode::OK);
                let session = active.load().unwrap();
                assert_eq!(
                    session.policy.active_content,
                    if choice == "enabled" {
                        crate::ResolvedActiveContentPolicy::Enabled
                    } else {
                        crate::ResolvedActiveContentPolicy::Disabled
                    }
                );
                assert_eq!(
                    session.policy.remote_resources,
                    crate::ResolvedRemoteResourcesPolicy::Blocked
                );
                wait_for_search_indexing(&search_indexing).await;
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_measurement_uses_fresh_session_authority_and_nonce() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let measurement_state = crate::mount::MeasurementPublicationState {
        reader_origin: Arc::from(TEST_READER_ORIGIN),
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: state.lifecycle.active_view(),
        stream_admission: Arc::new(Semaphore::new(4)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let reader = reader_router(state, None);
    let archive = cover_epub("image/png", "png", b"image");
    let imported = reader
        .clone()
        .oneshot(import_request_with(archive))
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let opened = reader
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_response: serde_json::Value =
        serde_json::from_slice(&opened.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(opened_response["status"], "navigate");
    let opened = pending_reading_bootstrap(&lifecycle);
    assert_eq!(opened["pagination"]["state"], "available");
    assert!(
        opened["pagination"]["responses"]["eligibleResourcePaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "EPUB/chapter.xhtml")
    );
    assert!(
        opened["pagination"]["foregroundPreparation"]["eligibleResourcePaths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == "EPUB/chapter.xhtml")
    );
    let measurement_origin = opened["measurementOrigin"].as_str().unwrap();
    assert_eq!(
        opened["pagination"]["responses"]["mount"]["baseUrl"],
        format!("{measurement_origin}/")
    );
    let measurement_authority = measurement_origin.strip_prefix("http://").unwrap();
    let measurement = measurement_publication_router(measurement_state);
    let first = measurement
        .clone()
        .oneshot(measurement_request(
            "GET",
            "/EPUB/chapter.xhtml",
            measurement_authority,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[CACHE_CONTROL], "no-store");
    assert_eq!(first.headers()[ACCEPT_RANGES], "none");
    let first_csp = first.headers()[CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(first_csp.contains("script-src 'nonce-"));
    assert!(first_csp.contains("script-src-attr 'none'"));
    assert!(first_csp.contains(&format!("frame-ancestors {TEST_READER_ORIGIN}")));
    let first_body = String::from_utf8(
        first
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(first_body.contains(&content_agent_path()));
    assert!(first_body.contains("crossorigin=\"anonymous\""));
    let nonce = first_csp
        .split("script-src 'nonce-")
        .nth(1)
        .unwrap()
        .split('\'')
        .next()
        .unwrap();
    assert!(first_body.contains(&format!("nonce=\"{nonce}\"")));

    let second = measurement
        .clone()
        .oneshot(measurement_request(
            "GET",
            "/EPUB/chapter.xhtml",
            measurement_authority,
        ))
        .await
        .unwrap();
    let second_csp = second.headers()[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(!second_csp.contains(&format!("nonce-{nonce}'")));

    let agent = reader
        .clone()
        .oneshot(imported_content_agent_request(measurement_origin))
        .await
        .unwrap();
    assert_eq!(agent.status(), StatusCode::OK);
    assert_eq!(
        agent.headers()["access-control-allow-origin"],
        measurement_origin
    );

    let replacement = reader
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::OK);
    let replacement_response: serde_json::Value =
        serde_json::from_slice(&replacement.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let replacement = pending_reading_bootstrap(&lifecycle);
    let replacement_origin = replacement["publication"]["mount"]["baseUrl"]
        .as_str()
        .unwrap()
        .trim_end_matches('/');
    let replacement_measurement = replacement["measurementOrigin"].as_str().unwrap();
    assert_eq!(
        measurement
            .clone()
            .oneshot(measurement_request(
                "GET",
                "/EPUB/chapter.xhtml",
                measurement_authority,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let reading = reader
        .clone()
        .oneshot(
            Request::builder()
                .uri(replacement_response["navigationPath"].as_str().unwrap())
                .header("host", "reader.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reading.status(), StatusCode::OK);
    let reading_csp = reading.headers()[CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(reading_csp.contains(&format!(
        "frame-src {replacement_origin} {replacement_measurement}"
    )));
    assert!(reading_csp.contains(&format!("img-src 'self' data: blob: {replacement_origin};")));
    assert!(!reading_csp.contains(measurement_origin));
    let reading = bootstrap_from_document(reading).await;
    assert_eq!(reading["mode"], "reading");
    assert_eq!(reading["pagination"]["state"], "available");
    let recovery_path = reading["recoveryNavigationPath"].as_str().unwrap();
    let recovered = reader
        .oneshot(reader_navigation_request(recovery_path))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    assert_eq!(
        recovered.headers()[CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap(),
        reading_csp
    );
    let recovered = bootstrap_from_document(recovered).await;
    assert_eq!(recovered["readerSessionId"], reading["readerSessionId"]);
    assert_eq!(recovered["publicationOrigin"], reading["publicationOrigin"]);
    assert_eq!(recovered["measurementOrigin"], reading["measurementOrigin"]);
    assert_ne!(
        recovered["recoveryNavigationPath"],
        reading["recoveryNavigationPath"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_or_mismatched_annotations_prevent_open_before_session_creation() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = created["publication"]["publicationId"].as_str().unwrap();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "INSERT INTO annotation_projections (publication_id, projection_revision)
                 VALUES (?1, 1)",
            [publication_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO annotations
                    (annotation_id, publication_id, record_order, record_type, occurrence,
                     target_json, resource_hint, created, modified)
                 VALUES ('bad!id', ?1, 1, 'highlight', 0, '{}', 'EPUB/chapter.xhtml',
                         '2026-09-25T00:00:00.000Z', '2026-09-25T00:00:00.000Z')",
            [publication_id],
        )
        .unwrap();
    drop(connection);

    let response = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(active.load().is_none());

    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "UPDATE annotations SET annotation_id = 'record-1',
                    resource_hint = 'EPUB/wrong.xhtml'
                 WHERE publication_id = ?1",
            [publication_id],
        )
        .unwrap();
    drop(connection);
    let response = router.oneshot(open_request(publication_id)).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(active.load().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn stale_open_waits_outside_the_session_gate_and_reselects_when_ready() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let session_gate = state.lifecycle.control_slot();
    let preparations = state.preparations.clone();
    let preparation_admission = state.preparation_admission.clone();
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        router
            .clone()
            .oneshot(open_request(&publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let original_session = active.load().as_ref().unwrap().reader_session_id.clone();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "UPDATE publication_snapshots SET reading_revision = ?1
                 WHERE snapshot_id = (
                    SELECT current_snapshot_id FROM publications WHERE publication_id = ?2
                 )",
            rusqlite::params!["0".repeat(64), publication_id],
        )
        .unwrap();
    drop(connection);

    let admission = preparation_admission.acquire_owned().await.unwrap();
    let opening = tokio::spawn(
        router
            .clone()
            .oneshot(open_request_with_operation(&publication_id, 2)),
    );
    let job = loop {
        if let Some(job) = preparations.lock().unwrap().get(&publication_id).cloned() {
            break job;
        }
        tokio::task::yield_now().await;
    };
    let session_operation = tokio::time::timeout(Duration::from_millis(100), session_gate.lock())
        .await
        .expect("open must release the reader session gate while preparing");
    assert_eq!(
        active.load().as_ref().unwrap().reader_session_id.as_ref(),
        original_session.as_ref()
    );
    let completion = job.completion.subscribe();
    drop(admission);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::session::wait_for_preparation(completion)
        )
        .await
        .unwrap(),
        crate::session::PreparationJobState::Ready
    );
    drop(session_operation);
    let opened = tokio::time::timeout(Duration::from_secs(5), opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    assert_ne!(
        active.load().as_ref().unwrap().reader_session_id.as_ref(),
        original_session.as_ref()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn preparation_timeout_returns_preparing_then_removes_the_completed_job() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = fixture_reader_state(directory.path());
    state.preparation_wait = Duration::ZERO;
    let preparations = state.preparations.clone();
    let preparation_admission = state.preparation_admission.clone();
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "UPDATE publication_snapshots SET reading_revision = ?1
                 WHERE snapshot_id = (
                    SELECT current_snapshot_id FROM publications WHERE publication_id = ?2
                 )",
            rusqlite::params!["0".repeat(64), publication_id],
        )
        .unwrap();
    drop(connection);

    let admission = preparation_admission.acquire_owned().await.unwrap();
    let response = router
        .clone()
        .oneshot(open_request_with_operation(&publication_id, 2))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(response.headers().get("retry-after").is_none());
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body, serde_json::json!({ "status": "preparing" }));
    let job = preparations
        .lock()
        .unwrap()
        .get(&publication_id)
        .unwrap()
        .clone();
    let completion = job.completion.subscribe();
    drop(admission);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::session::wait_for_preparation(completion)
        )
        .await
        .unwrap(),
        crate::session::PreparationJobState::Ready
    );
    assert!(preparations.lock().unwrap().get(&publication_id).is_none());
    assert_eq!(
        router
            .oneshot(open_request_with_operation(&publication_id, 2))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test(flavor = "current_thread")]
async fn warm_snapshot_reopen_skips_persisted_json_and_cold_load_still_rejects_it() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let snapshots = state.snapshots.clone();
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        router
            .clone()
            .oneshot(open_request(&publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    connection
        .execute(
            "UPDATE publication_snapshots SET responses_json = '{malformed'",
            [],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        router
            .clone()
            .oneshot(open_request(&publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    snapshots.lock().unwrap().warm = None;
    assert_eq!(
        router
            .oneshot(open_request(&publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cold_open_ignores_missing_and_stale_search() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let snapshots = state.snapshots.clone();
    let search_indexing = state.search_indexing.clone();
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = created["publication"]["publicationId"].as_str().unwrap();
    wait_for_search_indexing(&search_indexing).await;
    let snapshot_id: String =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE))
            .unwrap()
            .query_row(
                "SELECT current_snapshot_id FROM publications WHERE publication_id = ?1",
                [publication_id],
                |row| row.get(0),
            )
            .unwrap();
    let index = directory
        .path()
        .join("search-indexes")
        .join(format!("{snapshot_id}.sqlite"));
    assert!(index.is_file());

    for stale in [
        "UPDATE search_indexes SET revision = 'search-v1'",
        "UPDATE search_indexes SET revision = 'search-v0'",
        "",
    ] {
        if stale.is_empty() {
            std::fs::remove_file(&index).unwrap();
        } else {
            let connection = Connection::open(&index).unwrap();
            connection.execute(stale, []).unwrap();
            drop(connection);
        }
        snapshots.lock().unwrap().warm = None;
        assert_eq!(
            router
                .clone()
                .oneshot(open_request(publication_id))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}
