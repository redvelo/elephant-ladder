use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn trusted_diagnostics_require_active_reader_auth_and_exact_private_schema() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let router = reader_router(state, None);
    let valid = serde_json::json!({
        "version": 1,
        "firstSequence": 1,
        "records": [{ "event": "readerStatus", "status": "ready" }]
    });
    assert_eq!(
        router
            .clone()
            .oneshot(diagnostic_request(
                TEST_READER_ORIGIN,
                "Bearer secret",
                valid.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

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
    assert_eq!(
        router
            .clone()
            .oneshot(diagnostic_request(
                TEST_READER_ORIGIN,
                "Bearer secret",
                valid.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        router
            .clone()
            .oneshot(diagnostic_request(
                TEST_READER_ORIGIN,
                "Bearer secret",
                valid.clone()
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let second = serde_json::json!({
        "version": 1,
        "firstSequence": 2,
        "records": [{
            "event": "paginationRun",
            "action": "terminal",
            "run": 1,
            "outcome": "complete",
            "durationMs": 10,
            "measuredOccurrences": 2,
            "totalOccurrences": 2,
            "measuredPageCount": 4,
            "totalPageCount": 4,
            "measuredTargets": 3,
            "totalTargets": 3,
            "pageCountAttempts": 2,
            "timedOutOccurrences": 0,
            "measurementMs": 6
        }]
    });
    assert_eq!(
        router
            .clone()
            .oneshot(diagnostic_request(
                TEST_READER_ORIGIN,
                "Bearer secret",
                second
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    for (origin, authorization, body, expected) in [
        (
            "http://publication.test",
            "Bearer secret",
            valid.clone(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            TEST_READER_ORIGIN,
            "Bearer wrong",
            valid.clone(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            TEST_READER_ORIGIN,
            "Bearer secret",
            serde_json::json!({
                "version": 1,
                "firstSequence": 3,
                "records": [{ "event": "readerStatus", "status": "ready", "title": "secret" }]
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            TEST_READER_ORIGIN,
            "Bearer secret",
            serde_json::json!({
                "version": 1,
                "firstSequence": 3,
                "records": [{ "event": "message", "text": "arbitrary" }]
            }),
            StatusCode::BAD_REQUEST,
        ),
        (
            TEST_READER_ORIGIN,
            "Bearer secret",
            serde_json::json!({
                "version": 1,
                "firstSequence": 3,
                "records": (0..17).map(|_| serde_json::json!({
                    "event": "readerStatus", "status": "ready"
                })).collect::<Vec<_>>()
            }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(diagnostic_request(origin, authorization, body))
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    let oversized = format!(
        "{}{}",
        serde_json::json!({
            "version": 1,
            "firstSequence": 3,
            "records": [{ "event": "readerStatus", "status": "ready" }]
        }),
        " ".repeat(MAX_DIAGNOSTIC_REQUEST_BYTES)
    );
    let oversized = Request::builder()
        .method("POST")
        .uri("/native/diagnostics/v1")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(oversized))
        .unwrap();
    assert_eq!(
        router.oneshot(oversized).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn network_status_diagnostics_round_trip() {
    let value = serde_json::json!({ "event": "readerStatus", "status": "network" });
    let record: crate::diagnostics::DiagnosticRecord =
        serde_json::from_value(value.clone()).unwrap();
    assert!(record.is_valid());
    assert_eq!(serde_json::to_value(record).unwrap(), value);
}

#[test]
fn font_diagnostics_require_exact_target_family_and_frame_pairing() {
    let parse = |record: serde_json::Value| {
        serde_json::from_value::<crate::diagnostics::DiagnosticBatch>(serde_json::json!({
            "version": 1,
            "firstSequence": 1,
            "records": [record]
        }))
        .unwrap()
        .records
        .remove(0)
    };
    assert!(
        parse(serde_json::json!({
            "event": "fontEvidence",
            "target": "interface",
            "family": "inter-variable",
            "frame": null,
            "configured": true,
            "declared": true,
            "loaded": true,
            "check": true
        }))
        .is_valid()
    );
    assert!(
        parse(serde_json::json!({
            "event": "fontEvidence",
            "target": "publication",
            "family": "navigator-reader-serif",
            "frame": 7,
            "configured": true,
            "declared": true,
            "loaded": true,
            "check": true
        }))
        .is_valid()
    );
    assert!(
        !parse(serde_json::json!({
            "event": "fontEvidence",
            "target": "publication",
            "family": "inter-variable",
            "frame": 7,
            "configured": true,
            "declared": true,
            "loaded": true,
            "check": true
        }))
        .is_valid()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn network_activity_auth_state_roundtrip_and_session_fences() {
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let observer = crate::NetworkActivityObserver::new(lifecycle.active_view());
    let router = reader_router(state, None);
    let request = |id: &str| {
        Request::builder()
            .uri("/native/reader/network-activity")
            .header("host", "reader.test")
            .header("sec-fetch-site", "same-origin")
            .header(AUTHORIZATION, "Bearer secret")
            .header("x-elephant-reader-session", id)
            .body(Body::empty())
            .unwrap()
    };
    assert!(observer.current_session_id().is_none());
    assert_eq!(
        router
            .clone()
            .oneshot(request("missing"))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
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
    let active = lifecycle.load_active().unwrap();
    let id = observer.current_session_id().unwrap();
    let sink = observer
        .bind_frame_origin(&active.authorities.foreground_origin)
        .unwrap();
    assert_eq!(sink.session_id(), id);
    assert!(sink.is_active());
    assert!(
        observer
            .bind_frame_origin(&active.authorities.nested_origin)
            .is_some()
    );
    assert!(
        observer
            .bind_frame_origin(&active.authorities.measurement_origin)
            .is_none()
    );
    assert!(observer.bind_frame_origin("https://remote.test").is_none());
    for (name, value) in [
        ("authorization", Some("Bearer wrong")),
        ("authorization", None),
        ("sec-fetch-site", Some("cross-site")),
        ("sec-fetch-site", None),
        ("origin", Some("https://remote.test")),
    ] {
        let mut req = request(&id);
        req.headers_mut().remove(name);
        if let Some(value) = value {
            req.headers_mut().insert(name, value.parse().unwrap());
        }
        let response = router.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    }
    for duplicate in [false, true] {
        let mut req = request(&id);
        if duplicate {
            req.headers_mut()
                .append("x-elephant-reader-session", id.parse().unwrap());
        } else {
            req.headers_mut().remove("x-elephant-reader-session");
        }
        assert_eq!(
            router.clone().oneshot(req).await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }
    for (available, timestamp) in [(false, None), (true, Some(1234)), (false, Some(1234))] {
        observer.clone().set_available(available);
        if let Some(timestamp) = timestamp {
            sink.observe_at(timestamp);
            sink.observe_at(timestamp - 1);
        }
        for _ in 0..2 {
            let response = router.clone().oneshot(request(&id)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
            let body: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(
                body,
                serde_json::json!({"sessionId": id, "available": available, "lastObservedAt": timestamp})
            );
        }
    }
    let recovery = crate::NavigationRecovery {
        reader_origin: Arc::from(TEST_READER_ORIGIN),
        lifecycle: lifecycle.clone(),
        imported_publication_port: 80,
        nested_publication_port: 82,
        measurement_port: 81,
    };
    recovery.recreate_url().await.unwrap();
    assert!(!sink.is_active());
    sink.observe_at(9999);
    assert_eq!(*active.network_activity.lock().unwrap(), Some(1234));
    assert_eq!(
        router.clone().oneshot(request(&id)).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let replacement_id = observer.current_session_id().unwrap();
    assert_ne!(replacement_id, id);
    let response = router
        .clone()
        .oneshot(request(&replacement_id))
        .await
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["lastObservedAt"], serde_json::Value::Null);
    let replacement = lifecycle.load_active().unwrap();
    replacement.revoked.store(true, Ordering::Release);
    assert!(observer.current_session_id().is_none());
    assert!(
        observer
            .bind_frame_origin(&replacement.authorities.foreground_origin)
            .is_none()
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(&replacement_id))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    observer.shutdown();
    observer.set_available(true);
    assert!(observer.current_session_id().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn http_tracing_uses_safe_routes_and_reports_cancellation() {
    let completed_buffer = TraceBuffer::default();
    let completed_router = Router::new()
        .route(
            "/native/test/{value}",
            get(|| async { StatusCode::NO_CONTENT }),
        )
        .layer(axum::middleware::from_fn_with_state(
            crate::http::HttpObservation {
                role: crate::http::HttpServerRole::Reader,
            },
            crate::http::observe_http,
        ));
    let response = completed_router
        .oneshot(
            Request::builder()
                .uri("/native/test/private-path?query=private-query")
                .body(Body::empty())
                .unwrap(),
        )
        .with_subscriber(trace_subscriber(completed_buffer.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let completed = completed_buffer.text();
    assert!(completed.contains("http_request_finished"));
    assert!(completed.contains("/native/test/{value}"));
    assert!(!completed.contains("private-path"));
    assert!(!completed.contains("private-query"));

    let cancelled_buffer = TraceBuffer::default();
    let entered = Arc::new(Notify::new());
    let cancelled_router = Router::new()
        .route(
            "/native/wait",
            get({
                let entered = entered.clone();
                move || {
                    let entered = entered.clone();
                    async move {
                        entered.notify_one();
                        std::future::pending::<axum::response::Response>().await
                    }
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            crate::http::HttpObservation {
                role: crate::http::HttpServerRole::Reader,
            },
            crate::http::observe_http,
        ));
    let cancellation_dispatch = tracing::Dispatch::new(trace_subscriber(cancelled_buffer.clone()));
    let request = cancelled_router
        .oneshot(
            Request::builder()
                .uri("/native/wait")
                .body(Body::empty())
                .unwrap(),
        )
        .with_subscriber(cancellation_dispatch.clone());
    let task = tokio::spawn(request);
    entered.notified().await;
    tracing::dispatcher::with_default(&cancellation_dispatch, || task.abort());
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(cancelled_buffer.text().contains("http_request_cancelled"));
}
