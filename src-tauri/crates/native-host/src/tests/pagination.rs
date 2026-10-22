use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn pagination_cache_bulk_read_is_bounded_ordered_and_authorized() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
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
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    let handle = bootstrap["pagination"]["cache"]["handle"].as_str().unwrap();
    let basis = "a".repeat(64);
    let first = "b".repeat(64);
    let missing = "c".repeat(64);
    let second = "d".repeat(64);
    for (record, value) in [(&first, &b"first"[..]), (&second, &b"second"[..])] {
        assert_eq!(
            router
                .clone()
                .oneshot(pagination_cache_write(&basis, record, value, handle))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }

    let response = router
        .clone()
        .oneshot(pagination_cache_request(
            "POST",
            &basis,
            handle,
            Body::from(serde_json::json!({ "records": [&first, &missing, &second] }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        response,
        serde_json::json!({ "records": ["Zmlyc3Q=", null, "c2Vjb25k"] })
    );

    for body in [
        "{".to_owned(),
        serde_json::json!({ "records": [] }).to_string(),
        serde_json::json!({ "records": [&first, &first] }).to_string(),
        serde_json::json!({
            "records": (0..33).map(|index| format!("{index:064x}")).collect::<Vec<_>>()
        })
        .to_string(),
        serde_json::json!({ "records": [&first], "extra": true }).to_string(),
        serde_json::json!({ "records": ["A".repeat(64)] }).to_string(),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(pagination_cache_request(
                    "POST",
                    &basis,
                    handle,
                    Body::from(body),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let oversized = format!(
        "{{\"records\":[\"{first}\"]{}}}",
        " ".repeat(crate::MAX_PAGINATION_BULK_REQUEST_BYTES)
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "POST",
                &basis,
                handle,
                Body::from(oversized),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "POST",
                &basis,
                "wrong-handle",
                Body::from("{"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut unauthorized = pagination_cache_request("POST", &basis, handle, Body::from("{"));
    unauthorized.headers_mut().remove(AUTHORIZATION);
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let blob = std::fs::read_dir(directory.path().join("archives/blobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(blob).unwrap();
    assert_eq!(
        router
            .oneshot(pagination_cache_request(
                "POST",
                &basis,
                handle,
                Body::from(serde_json::json!({ "records": [&first] }).to_string()),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test(flavor = "current_thread")]
async fn pagination_cache_bulk_mutation_is_bounded_atomic_and_authorized() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let active_reader_session = state.lifecycle.active_view();
    let storage = state.storage.clone();
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
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    let handle = bootstrap["pagination"]["cache"]["handle"].as_str().unwrap();
    let basis = "a".repeat(64);
    let removed = "b".repeat(64);
    let first = "c".repeat(64);
    let second = "d".repeat(64);
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_write(
                &basis,
                &removed,
                &b"old"[..],
                handle
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let mutation = serde_json::json!({
        "operations": [
            { "type": "remove", "record": &removed },
            { "type": "write", "record": &first, "value": "Zmlyc3Q=" },
            { "type": "write", "record": &second, "value": "c2Vjb25k" }
        ]
    });
    let response = router
        .clone()
        .oneshot(pagination_cache_request(
            "PUT",
            &basis,
            handle,
            Body::from(mutation.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );
    for (record, value) in [
        (&removed, None),
        (&first, Some(&b"first"[..])),
        (&second, Some(&b"second"[..])),
    ] {
        assert_eq!(
            pagination_cache_record(&router, &basis, record, handle).await,
            value.map(<[u8]>::to_vec)
        );
    }

    let too_many = (0..33)
        .map(|index| serde_json::json!({ "type": "remove", "record": format!("{index:064x}") }))
        .collect::<Vec<_>>();
    let oversized_value = BASE64_STANDARD.encode(vec![0; MAX_PAGINATION_RECORD_BYTES + 1]);
    for body in [
        "{".to_owned(),
        serde_json::json!({ "operations": [] }).to_string(),
        serde_json::json!({ "operations": [
                { "type": "remove", "record": &first },
                { "type": "write", "record": &first, "value": "eA==" }
            ] })
        .to_string(),
        serde_json::json!({ "operations": [
                { "type": "remove", "record": "A".repeat(64) }
            ] })
        .to_string(),
        serde_json::json!({ "operations": [
                { "type": "write", "record": &first, "value": "not-base64" }
            ] })
        .to_string(),
        serde_json::json!({ "operations": [
                { "type": "write", "record": &first, "value": oversized_value }
            ] })
        .to_string(),
        serde_json::json!({ "operations": too_many }).to_string(),
        serde_json::json!({ "operations": [
                { "type": "remove", "record": &first, "extra": true }
            ] })
        .to_string(),
        serde_json::json!({ "operations": [
                { "type": "remove", "record": &first }
            ], "extra": true })
        .to_string(),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(pagination_cache_request(
                    "PUT",
                    &basis,
                    handle,
                    Body::from(body),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let oversized_request = format!(
        "{{\"operations\":[{{\"type\":\"remove\",\"record\":\"{first}\"}}]}}{}",
        " ".repeat(MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES)
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "PUT",
                &basis,
                handle,
                Body::from(oversized_request),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut unauthorized =
        pagination_cache_request("PUT", &basis, handle, Body::from(mutation.to_string()));
    unauthorized.headers_mut().remove(AUTHORIZATION);
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let mut wrong_origin =
        pagination_cache_request("PUT", &basis, handle, Body::from(mutation.to_string()));
    wrong_origin
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static("http://wrong.test"));
    assert_eq!(
        router.clone().oneshot(wrong_origin).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "PUT",
                &basis,
                "wrong-handle",
                Body::from("{"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut wrong_content_type =
        pagination_cache_request("PUT", &basis, handle, Body::from(mutation.to_string()));
    wrong_content_type
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert_eq!(
        router
            .clone()
            .oneshot(wrong_content_type)
            .await
            .unwrap()
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let session = active_reader_session.load().unwrap();
    let quota_basis = "e".repeat(64);
    let quota_value = vec![7; MAX_PAGINATION_RECORD_BYTES];
    {
        let mut storage = storage.lock().unwrap();
        for index in 0..64 {
            assert_eq!(
                storage
                    .write_pagination_record(
                        &session.publication_id,
                        &session.entry.snapshot_id,
                        &quota_basis,
                        &format!("{index:064x}"),
                        &quota_value,
                    )
                    .unwrap(),
                PaginationCacheMutationOutcome::Applied
            );
        }
    }
    let quota_mutation = serde_json::json!({
        "operations": [
            { "type": "remove", "record": format!("{:064x}", 0) },
            { "type": "write", "record": format!("{:064x}", 64),
              "value": BASE64_STANDARD.encode(&quota_value) },
            { "type": "write", "record": format!("{:064x}", 65),
              "value": BASE64_STANDARD.encode(&quota_value) }
        ]
    });
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "PUT",
                &quota_basis,
                handle,
                Body::from(quota_mutation.to_string()),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_read(
                &quota_basis,
                &format!("{:064x}", 0),
                handle,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let blob = std::fs::read_dir(directory.path().join("archives/blobs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::remove_file(blob).unwrap();
    assert_eq!(
        router
            .oneshot(pagination_cache_request(
                "PUT",
                &basis,
                handle,
                Body::from(mutation.to_string()),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test(flavor = "current_thread")]
async fn each_pagination_method_enforces_its_own_declared_body_limit() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
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
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    let handle = bootstrap["pagination"]["cache"]["handle"].as_str().unwrap();
    let basis = "a".repeat(64);

    // A bulk read carries only record identifiers, so it declares the small
    // limit. Its method router must not inherit the mutation limit, which is
    // several hundred times larger.
    let oversized = serde_json::to_vec(&serde_json::json!({
        "records": vec!["b".repeat(64); crate::MAX_PAGINATION_BULK_RECORDS + 1],
        "padding": "p".repeat(crate::MAX_PAGINATION_BULK_REQUEST_BYTES),
    }))
    .unwrap();
    assert!(oversized.len() > crate::MAX_PAGINATION_BULK_REQUEST_BYTES);
    assert!(oversized.len() < crate::MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES);
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "POST",
                &basis,
                handle,
                Body::from(oversized.clone()),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    // The same body on the mutation method is within its larger limit, so it is
    // rejected on its shape rather than its size.
    assert_eq!(
        router
            .clone()
            .oneshot(pagination_cache_request(
                "PUT",
                &basis,
                handle,
                Body::from(oversized),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}
