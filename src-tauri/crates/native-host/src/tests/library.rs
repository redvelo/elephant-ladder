use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn failed_publication_import_releases_its_durable_reservation() {
    let directory = tempfile::tempdir().unwrap();
    let router = fixture_reader_router(directory.path());
    let response = router
        .oneshot(import_request_with(b"not an epub".to_vec()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_staging_empty(directory.path()).await;
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM import_reservations", [], |row| {
                row.get::<_, u32>(0)
            })
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn deletes_an_imported_publication_and_rejects_stale_retries() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active_reader_session = state.lifecycle.active_view();
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
    let opened_session = active_reader_session.load().unwrap();

    let mut missing_document = delete_request(publication_id);
    missing_document
        .headers_mut()
        .remove(crate::LIBRARY_DOCUMENT_HEADER);
    assert_eq!(
        router
            .clone()
            .oneshot(missing_document)
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let deleted = router
        .clone()
        .oneshot(delete_request(publication_id))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(opened_session.revoked.load(Ordering::Acquire));
    assert!(active_reader_session.load().is_none());
    assert!(
        deleted
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    assert_eq!(
        router
            .clone()
            .oneshot(open_request_with_operation(publication_id, 2))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        router
            .oneshot(delete_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resets_website_data_only_for_a_closed_imported_publication() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let router = reader_router(state, None);
    let imported = router.clone().oneshot(import_request()).await.unwrap();
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    let original_origin: String = connection
        .query_row(
            "SELECT origin_id FROM browser_origin_allocations
                 WHERE publication_id = ?1 AND lifecycle_state = 'active'",
            [publication_id],
            |row| row.get(0),
        )
        .unwrap();

    let mut unauthorized = website_data_reset_request(publication_id);
    unauthorized
        .headers_mut()
        .insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let wrong_content_type = Request::builder()
        .method("POST")
        .uri("/native/open-external-url")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "text/plain")
        .body(Body::from(r#"{"url":"https://example.test/"}"#))
        .unwrap();
    assert_eq!(
        router
            .clone()
            .oneshot(wrong_content_type)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let mut wrong_origin = website_data_reset_request(publication_id);
    wrong_origin
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static("http://attacker.test"));
    assert_eq!(
        router.clone().oneshot(wrong_origin).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let mut stale_document = website_data_reset_request(publication_id);
    stale_document.headers_mut().insert(
        crate::LIBRARY_DOCUMENT_HEADER,
        HeaderValue::from_str(&"c".repeat(64)).unwrap(),
    );
    assert_eq!(
        router
            .clone()
            .oneshot(stale_document)
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let mut missing_document = website_data_reset_request(publication_id);
    missing_document
        .headers_mut()
        .remove(crate::LIBRARY_DOCUMENT_HEADER);
    assert_eq!(
        router
            .clone()
            .oneshot(missing_document)
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(website_data_reset_request("publication"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        router
            .clone()
            .oneshot(website_data_reset_request(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let reset = router
        .clone()
        .oneshot(website_data_reset_request(publication_id))
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let reset: serde_json::Value =
        serde_json::from_slice(&reset.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        reset,
        serde_json::json!({
            "status": "reset",
            "physicalPurge": "unsupported",
            "purgeReport": {
                "androidWebStorage": "unsupported",
                "cookies": "unsupported",
                "cacheStorage": "unsupported",
                "workerStorage": "unsupported",
                "httpAuthentication": "unsupported",
                "formData": "unsupported",
                "opfs": "unsupported"
            }
        })
    );
    let replacement_origin: String = connection
        .query_row(
            "SELECT origin_id FROM browser_origin_allocations
                 WHERE publication_id = ?1 AND lifecycle_state = 'active'",
            [publication_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(replacement_origin, original_origin);
    assert_eq!(
        connection
            .query_row(
                "SELECT lifecycle_state FROM browser_origin_allocations WHERE origin_id = ?1",
                [&original_origin],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "purge_pending"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT purge_attempts FROM browser_origin_allocations WHERE origin_id = ?1",
                [&original_origin],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        1
    );
    let purge_report: String = connection
        .query_row(
            "SELECT last_purge_report_json FROM browser_origin_allocations WHERE origin_id = ?1",
            [&original_origin],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&purge_report).unwrap(),
        reset["purgeReport"]
    );

    assert_eq!(
        router
            .clone()
            .oneshot(open_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router
            .oneshot(website_data_reset_request(publication_id))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test(flavor = "current_thread")]
async fn imports_lists_and_deduplicates_a_publication() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let search_indexing = state.search_indexing.clone();
    let snapshots = state.snapshots.clone();
    let lifecycle = state.lifecycle.clone();
    let navigation_policy = crate::ContentNavigationPolicy {
        active_reader_session: state.lifecycle.active_view(),
    };
    assert!(!navigation_policy.allows_origin("http://p-untrusted.localhost:41000"));
    let imported = ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: state.lifecycle.active_view(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(4)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let router = reader_router(state, None);
    let created = router.clone().oneshot(import_request()).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(created["disposition"], "created");
    let publication_id = created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned();

    let existing = router.clone().oneshot(import_request()).await.unwrap();
    assert_eq!(existing.status(), StatusCode::OK);
    let existing: serde_json::Value =
        serde_json::from_slice(&existing.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(existing["disposition"], "existing");
    assert_eq!(existing["publication"]["publicationId"], publication_id);

    let listed = router
        .clone()
        .oneshot(publication_list_request())
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: serde_json::Value =
        serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["publicationId"], publication_id);

    let opened = router
        .clone()
        .oneshot(open_request(&publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let opened_response: serde_json::Value =
        serde_json::from_slice(&opened.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(opened_response["status"], "navigate");
    let opened = pending_reading_bootstrap(&lifecycle);
    assert_eq!(opened["publicationId"], publication_id);
    assert!(opened["initialReadingPosition"].is_null());
    assert_eq!(opened["positionRevision"], 0);
    assert_eq!(
        opened["annotations"],
        serde_json::json!({ "revision": 0, "records": [] })
    );
    let reader_session_id = opened["readerSessionId"].as_str().unwrap();
    let opened_session = navigation_policy.active_reader_session.load().unwrap();
    {
        let _control = lifecycle.try_control();
        let loaded = navigation_policy.active_reader_session.load().unwrap();
        assert!(Arc::ptr_eq(&loaded, &opened_session));
    }
    assert_eq!(
        router
            .clone()
            .oneshot(search_request(&"0".repeat(64), "autho", None))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    wait_for_search_indexing(&search_indexing).await;
    let searched = router
        .clone()
        .oneshot(search_request(reader_session_id, "alice", None))
        .await
        .unwrap();
    assert_eq!(searched.status(), StatusCode::OK);
    let searched: serde_json::Value =
        serde_json::from_slice(&searched.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(searched["status"], "completed");
    assert!(!searched["batch"]["results"].as_array().unwrap().is_empty());
    assert_eq!(
        router
            .clone()
            .oneshot(import_request())
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let pagination_handle = opened["pagination"]["cache"]["handle"].as_str().unwrap();
    let basis = "b".repeat(64);
    let record = "c".repeat(64);
    let write_cache = router
        .clone()
        .oneshot(pagination_cache_write(
            &basis,
            &record,
            &b"cached-pages"[..],
            pagination_handle,
        ))
        .await
        .unwrap();
    assert_eq!(write_cache.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        pagination_cache_record(&router, &basis, &record, pagination_handle).await,
        Some(b"cached-pages".to_vec())
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_read(
                &basis,
                &record,
                "wrong-pagination-handle",
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let resource_path = "/epub/text/chapter-1.xhtml";
    let foreground_origin = opened["publication"]["mount"]["baseUrl"]
        .as_str()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let measurement_origin = opened["measurementOrigin"].as_str().unwrap().to_owned();
    let nested_origin = opened_session.authorities.nested_origin.to_string();
    assert!(navigation_policy.allows_origin(&foreground_origin));
    assert!(navigation_policy.allows_origin(&nested_origin));
    assert!(navigation_policy.allows_origin(&measurement_origin));
    let foreground_authority = foreground_origin.strip_prefix("http://").unwrap();
    let nested_router = crate::mount::nested_publication_router(imported.clone());
    let imported_router = imported_publication_router(imported);
    let resource = imported_router
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            resource_path,
            foreground_authority,
        ))
        .await
        .unwrap();
    assert_eq!(resource.status(), StatusCode::OK);
    let resource = String::from_utf8(
        resource
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(resource.contains(&content_agent_path()));
    assert!(!resource.contains("publication-snapshot.js"));
    assert!(resource.contains(&format!(
        "data-epub-navigator-nested-publication-base-url=\"{nested_origin}\""
    )));
    assert_eq!(
        snapshots
            .lock()
            .unwrap()
            .warm
            .as_ref()
            .unwrap()
            .transforms
            .lock()
            .unwrap()
            .entries
            .len(),
        1
    );
    let nested_authority = nested_origin.strip_prefix("http://").unwrap();
    let nested = nested_router
        .oneshot(imported_publication_request(
            "GET",
            resource_path,
            nested_authority,
        ))
        .await
        .unwrap();
    assert_eq!(nested.status(), StatusCode::OK);
    let nested_csp = nested.headers()[CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(nested_csp.contains("sandbox allow-scripts"));
    assert!(!nested_csp.contains("allow-same-origin"));
    let nested = String::from_utf8(
        nested
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(nested.contains(&content_agent_path()));
    assert!(!nested.contains("data-epub-navigator-nested-publication-base-url"));
    assert_eq!(
        snapshots
            .lock()
            .unwrap()
            .warm
            .as_ref()
            .unwrap()
            .transforms
            .lock()
            .unwrap()
            .entries
            .len(),
        2
    );
    let mut range =
        imported_publication_request("GET", "/epub/images/cover.svg", foreground_authority);
    range
        .headers_mut()
        .insert("range", HeaderValue::from_static("bytes=0-3"));
    let ranged = imported_router.clone().oneshot(range).await.unwrap();
    let source = opened_session
        .entry
        .epub
        .bytes(&epub_stack::EpubPath::new("epub/images/cover.svg").unwrap())
        .unwrap();
    assert_eq!(source.len(), 1126267);
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(ranged.headers()[ACCEPT_RANGES], "bytes");
    assert_eq!(ranged.headers()[CONTENT_RANGE], "bytes 0-3/1126267");
    assert_eq!(
        ranged.into_body().collect().await.unwrap().to_bytes(),
        &source[..4]
    );
    let head = imported_router
        .clone()
        .oneshot(imported_publication_request(
            "HEAD",
            "/epub/images/cover.svg",
            foreground_authority,
        ))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[CONTENT_LENGTH], source.len().to_string());
    assert!(
        head.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    let agent = router
        .clone()
        .oneshot(imported_content_agent_request(&foreground_origin))
        .await
        .unwrap();
    assert_eq!(agent.status(), StatusCode::OK);
    assert_eq!(
        agent.headers()["access-control-allow-origin"],
        foreground_origin
    );

    let create_annotation = serde_json::json!({
        "type": "create-highlight", "requestId": "route-create",
        "basedOnRevision": 0, "selectionRevision": 1, "occurrence": 0,
        "targetJson": "{}", "excerpt": "route excerpt", "sectionLabel": null
    });
    let mut unauthorized_annotation =
        annotation_mutation_request(reader_session_id, &create_annotation);
    unauthorized_annotation.headers_mut().remove(AUTHORIZATION);
    assert_eq!(
        router
            .clone()
            .oneshot(unauthorized_annotation)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let annotation = router
        .clone()
        .oneshot(annotation_mutation_request(
            reader_session_id,
            &create_annotation,
        ))
        .await
        .unwrap();
    assert_eq!(annotation.status(), StatusCode::OK);
    let annotation: serde_json::Value =
        serde_json::from_slice(&annotation.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(annotation["status"], "accepted");
    assert_eq!(annotation["projection"]["revision"], 1);
    assert_eq!(
        annotation["projection"]["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        annotation["projection"]["records"][0]
            .get("resourceHint")
            .is_none()
    );
    let conflict = router
        .clone()
        .oneshot(annotation_mutation_request(
            reader_session_id,
            &create_annotation,
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let conflict: serde_json::Value =
        serde_json::from_slice(&conflict.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(conflict["reason"], "revision-mismatch");
    assert_eq!(
        conflict["authoritative"]["projection"],
        annotation["projection"]
    );

    let position = serde_json::json!({
        "targetJson": "{\"source\":\"EPUB/text/conformance.xhtml\"}",
        "rollProgression": 0.375,
        "occurrenceHint": {
            "occurrence": 0,
            "resource": "EPUB/text/conformance.xhtml"
        }
    });
    for progression in [-0.001, 1.001] {
        let mut invalid = position.clone();
        invalid["rollProgression"] = serde_json::json!(progression);
        assert_eq!(
            router
                .clone()
                .oneshot(reading_position_request(reader_session_id, 0, 1, &invalid))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let saved = router
        .clone()
        .oneshot(reading_position_request(reader_session_id, 0, 1, &position))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved: serde_json::Value =
        serde_json::from_slice(&saved.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(saved["status"], "saved");
    assert_eq!(saved["positionRevision"], 1);
    assert_eq!(
        router
            .clone()
            .oneshot(reading_position_request(reader_session_id, 1, 1, &position))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(reading_position_request(reader_session_id, 0, 2, &position))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(reading_position_request(reader_session_id, 1, 2, &position))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let recovery = pending_reading_bootstrap(&lifecycle);
    assert_eq!(recovery["initialReadingPosition"], position);
    assert_eq!(recovery["positionRevision"], 2);
    assert_eq!(recovery["annotations"], annotation["projection"]);

    let mut preferences = recovery["preferences"].clone();
    preferences["colorScheme"] = serde_json::json!("dark");
    preferences["presentationMode"] = serde_json::json!("document-scroll");
    let saved_preferences = router
        .clone()
        .oneshot(preference_request(reader_session_id, 0, 0, &preferences))
        .await
        .unwrap();
    let preference_status = saved_preferences.status();
    let preference_body = saved_preferences
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(
        preference_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&preference_body)
    );
    let saved_preferences: serde_json::Value = serde_json::from_slice(&preference_body).unwrap();
    assert_eq!(saved_preferences["preferences"], preferences);
    assert_eq!(saved_preferences["revisions"]["global"], 1);
    assert_eq!(saved_preferences["revisions"]["publication"], 1);
    let preference_conflict = router
        .clone()
        .oneshot(preference_request(reader_session_id, 0, 0, &preferences))
        .await
        .unwrap();
    assert_eq!(preference_conflict.status(), StatusCode::CONFLICT);
    let preference_conflict: serde_json::Value = serde_json::from_slice(
        &preference_conflict
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(preference_conflict["reason"], "revision-mismatch");
    assert_eq!(
        preference_conflict["authoritative"]["preferences"],
        preferences
    );
    assert_eq!(
        preference_conflict["authoritative"]["revisions"],
        saved_preferences["revisions"]
    );
    let recovery = pending_reading_bootstrap(&lifecycle);
    assert_eq!(recovery["preferences"], preferences);
    assert_eq!(
        recovery["preferenceRevisions"],
        saved_preferences["revisions"]
    );

    let replacement = router
        .clone()
        .oneshot(open_request(&publication_id))
        .await
        .unwrap();
    assert_eq!(replacement.status(), StatusCode::OK);
    let replacement_response: serde_json::Value =
        serde_json::from_slice(&replacement.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(replacement_response["status"], "navigate");
    assert!(opened_session.revoked.load(Ordering::Acquire));
    let replacement = pending_reading_bootstrap(&lifecycle);
    let replacement_session_id = replacement["readerSessionId"].as_str().unwrap();
    let replacement_origin = replacement["publication"]["mount"]["baseUrl"]
        .as_str()
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let replacement_measurement_origin = replacement["measurementOrigin"]
        .as_str()
        .unwrap()
        .to_owned();
    let replacement_authority = replacement_origin.strip_prefix("http://").unwrap();
    assert_eq!(replacement_origin, foreground_origin);
    assert!(navigation_policy.allows_origin(&foreground_origin));
    assert!(!navigation_policy.allows_origin(&measurement_origin));
    assert!(navigation_policy.allows_origin(&replacement_origin));
    assert!(navigation_policy.allows_origin(&replacement_measurement_origin));
    assert_eq!(replacement["initialReadingPosition"], position);
    assert_eq!(replacement["positionRevision"], 2);
    assert_eq!(replacement["annotations"], annotation["projection"]);
    assert_ne!(replacement_session_id, reader_session_id);
    assert_eq!(
        router
            .clone()
            .oneshot(annotation_mutation_request(
                reader_session_id,
                &serde_json::json!({
                    "type": "delete-annotation", "requestId": "stale-session",
                    "basedOnRevision": 1, "recordId": annotation["recordId"]
                }),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(reading_position_request(reader_session_id, 2, 3, &position))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(close_request(reader_session_id))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        imported_router
            .clone()
            .oneshot(imported_publication_request(
                "GET",
                resource_path,
                foreground_authority,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        imported_router
            .clone()
            .oneshot(imported_publication_request(
                "GET",
                resource_path,
                replacement_authority,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let first_close = router
        .clone()
        .oneshot(close_request(replacement_session_id))
        .await
        .unwrap();
    assert_eq!(first_close.status(), StatusCode::OK);
    let first_close: serde_json::Value =
        serde_json::from_slice(&first_close.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert!(!navigation_policy.allows_origin(&replacement_origin));
    assert!(!navigation_policy.allows_origin(&replacement_measurement_origin));
    assert_eq!(
        imported_router
            .oneshot(imported_publication_request(
                "GET",
                resource_path,
                replacement_authority,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let repeated_close = router
        .oneshot(close_request(replacement_session_id))
        .await
        .unwrap();
    assert_eq!(repeated_close.status(), StatusCode::OK);
    let repeated_close: serde_json::Value = serde_json::from_slice(
        &repeated_close
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(
        repeated_close["navigationPath"],
        first_close["navigationPath"]
    );
    let reopened = fixture_reader_router(directory.path())
        .oneshot(publication_list_request())
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::OK);
    let reopened: serde_json::Value =
        serde_json::from_slice(&reopened.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(reopened.as_array().unwrap().len(), 1);
    assert_eq!(reopened[0]["publicationId"], publication_id);
    assert_eq!(reopened[0]["hasReadingPosition"], true);
    assert!(reopened[0]["lastReadAt"].is_string());
    let restarted_state = fixture_reader_state(directory.path());
    let restarted_lifecycle = restarted_state.lifecycle.clone();
    let restarted_open = reader_router(restarted_state, None)
        .oneshot(open_request(&publication_id))
        .await
        .unwrap();
    assert_eq!(restarted_open.status(), StatusCode::OK);
    let restarted_open: serde_json::Value = serde_json::from_slice(
        &restarted_open
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(restarted_open["status"], "navigate");
    let restarted = pending_reading_bootstrap(&restarted_lifecycle);
    assert_eq!(restarted["initialReadingPosition"], position);
    assert_eq!(restarted["positionRevision"], 2);
    assert_eq!(restarted["annotations"], annotation["projection"]);
    assert_eq!(restarted["publicationOrigin"], replacement_origin);
    assert_staging_empty(directory.path()).await;
}

#[tokio::test(flavor = "current_thread")]
async fn materialized_cover_streams_after_host_restart() {
    let directory = tempfile::tempdir().unwrap();
    let cover_bytes = b"exact jpeg xl cover bytes";
    let epub = cover_epub("image/jxl", "jxl", cover_bytes);
    let publication_id = {
        let state = fixture_reader_state(directory.path());
        let search_indexing = state.search_indexing.clone();
        let router = reader_router(state, None);
        let response = router
            .oneshot(import_request_with(epub.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let value: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(value["publication"]["hasCover"], true);
        wait_for_search_indexing(&search_indexing).await;
        value["publication"]["publicationId"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let router = fixture_reader_router(directory.path());
    let unauthorized = router
        .clone()
        .oneshot(cover_request("GET", &publication_id, false))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = router
        .clone()
        .oneshot(cover_request("GET", &publication_id, true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "image/jxl");
    assert_eq!(
        response.headers()[CONTENT_LENGTH],
        cover_bytes.len().to_string()
    );
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        &cover_bytes[..]
    );

    let head = router
        .oneshot(cover_request("HEAD", &publication_id, true))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[CONTENT_TYPE], "image/jxl");
    assert!(
        head.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
}

#[tokio::test]
async fn a_handed_publication_is_offered_once_and_its_staged_copy_is_removed() {
    let directory = tempfile::tempdir().unwrap();
    let handed = directory.path().join("handed.epub");
    std::fs::write(&handed, ALICE_BYTES).unwrap();
    let actions = Arc::new(OpenedPublicationActions(Mutex::new(vec![
        crate::OpenedPublication {
            file_name: "alice.epub".to_owned(),
            path: handed.clone(),
            owned: true,
            intent: crate::OpenedPublicationIntent::Read,
        },
    ])));
    let state = fixture_reader_state_with_actions(directory.path(), actions);
    let router = reader_router(state, None);

    let (status, pending) = response_json(
        router
            .clone()
            .oneshot(opened_publication_request(
                "/native/opened-publications/pending",
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["fileName"], "alice.epub");
    assert_eq!(pending["sizeBytes"], ALICE_BYTES.len());
    assert_eq!(pending["intent"], "read");
    let open_id = pending["openId"].as_str().unwrap().to_owned();

    let response = router
        .clone()
        .oneshot(opened_publication_request(&format!(
            "/native/opened-publications/{open_id}/bytes"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        "application/epub+zip",
        "the reader posts these bytes straight back to the import route"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), ALICE_BYTES);
    assert!(
        !handed.exists(),
        "a staged platform copy is removed with its read"
    );

    let response = router
        .oneshot(opened_publication_request(&format!(
            "/native/opened-publications/{open_id}/bytes"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_handed_path_that_is_not_a_readable_file_is_never_offered() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(OpenedPublicationActions(Mutex::new(vec![
        crate::OpenedPublication {
            file_name: "missing.epub".to_owned(),
            path: directory.path().join("missing.epub"),
            owned: false,
            intent: crate::OpenedPublicationIntent::Keep,
        },
        crate::OpenedPublication {
            file_name: "../escape.epub".to_owned(),
            path: directory.path().to_path_buf(),
            owned: false,
            intent: crate::OpenedPublicationIntent::Keep,
        },
    ])));
    let state = fixture_reader_state_with_actions(directory.path(), actions);
    let router = reader_router(state, None);

    let response = router
        .oneshot(opened_publication_request(
            "/native/opened-publications/pending",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn dropped_publications_wait_their_turn_and_are_kept_rather_than_read() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.epub");
    let second = directory.path().join("second.epub");
    std::fs::write(&first, ALICE_BYTES).unwrap();
    std::fs::write(&second, ALICE_BYTES).unwrap();
    let dropped = |path: &std::path::Path| crate::OpenedPublication {
        file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        owned: false,
        intent: crate::OpenedPublicationIntent::Keep,
    };
    let actions = Arc::new(OpenedPublicationActions(Mutex::new(vec![
        dropped(&first),
        dropped(&second),
    ])));
    let state = fixture_reader_state_with_actions(directory.path(), actions);
    let router = reader_router(state, None);

    let offered = |after: Option<&str>| {
        let uri = match after {
            None => "/native/opened-publications/pending".to_owned(),
            Some(id) => format!("/native/opened-publications/pending?after={id}"),
        };
        opened_publication_request(&uri)
    };

    let (status, pending) =
        response_json(router.clone().oneshot(offered(None)).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["fileName"], "first.epub");
    assert_eq!(
        pending["intent"], "keep",
        "a drop keeps the book rather than opening it"
    );
    let first_id = pending["openId"].as_str().unwrap().to_owned();

    // The second waits behind the first rather than replacing it.
    let (_, still_first) = response_json(
        router
            .clone()
            .oneshot(offered(Some(&first_id)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(still_first, serde_json::Value::Null);

    let response = router
        .clone()
        .oneshot(opened_publication_request(&format!(
            "/native/opened-publications/{first_id}/bytes"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        first.exists(),
        "a dropped file belongs to the person who dropped it"
    );

    let (status, next) =
        response_json(router.oneshot(offered(Some(&first_id))).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["fileName"], "second.epub");
}

#[tokio::test]
async fn storage_usage_reports_what_each_part_of_the_application_holds() {
    let directory = tempfile::tempdir().unwrap();
    let router = fixture_reader_router(directory.path());
    let (status, imported) =
        response_json(router.clone().oneshot(import_request()).await.unwrap()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(imported["publication"]["publicationId"].is_string());

    let (status, usage) = response_json(
        router
            .oneshot(opened_publication_request("/native/storage/usage"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        usage["booksBytes"].as_u64().unwrap(),
        ALICE_BYTES.len() as u64,
        "the archive knows exactly what it committed: {usage}"
    );
    assert_eq!(usage["dictionariesBytes"], 0);
}

/// The library list is what the interface renders a title and creators from, so the
/// direction each declares has to survive extraction, storage and serialization.
#[tokio::test]
async fn the_publication_list_states_the_direction_its_metadata_declares() {
    let package = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id" dir="ltr"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">direction</dc:identifier><dc:title dir="rtl">CSS: a new adventure!</dc:title><dc:creator dir="rtl">Dave Cramer</dc:creator><dc:creator>Ivan Herman</dc:creator><dc:language>he</dc:language><meta property="dcterms:modified">2026-10-06T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#;
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><p>Text</p></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let archive = synthetic_epub(
        package,
        &[("EPUB/chapter.xhtml", chapter), ("EPUB/nav.xhtml", nav)],
    );

    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let router = reader_router(state, None);
    let created = router
        .clone()
        .oneshot(import_request_with(archive))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let listed = router.oneshot(publication_list_request()).await.unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: serde_json::Value =
        serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let publication = &listed[0];

    // The title declares right-to-left against a left-to-right package, so it wins.
    assert_eq!(publication["titleDirection"], "rtl");
    // The first creator declares it; the second says nothing and inherits the package.
    assert_eq!(publication["creatorDirections"][0], "rtl");
    assert_eq!(publication["creatorDirections"][1], "ltr");
    assert_eq!(
        publication["creators"].as_array().unwrap().len(),
        publication["creatorDirections"].as_array().unwrap().len()
    );
}
