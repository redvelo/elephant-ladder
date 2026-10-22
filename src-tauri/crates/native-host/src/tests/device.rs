use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn front_light_dialog_dispatches_and_requires_reader_authorization() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestDisplayActions::default());
    let router = reader_router(
        fixture_reader_state_with_actions(directory.path(), actions.clone()),
        None,
    );

    let unauthorized = Request::builder()
        .method("POST")
        .uri("/native/display/front-light-dialog")
        .header("host", "reader.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(*actions.front_light_dialogs.lock().unwrap(), 0);

    let request = Request::builder()
        .method("POST")
        .uri("/native/display/front-light-dialog")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(*actions.front_light_dialogs.lock().unwrap(), 1);

    // The device owns the front-light values now, so the host exposes neither a
    // read nor a write surface for them.
    for (method, path, expected) in [
        ("GET", "/native/display", StatusCode::NOT_FOUND),
        (
            "PUT",
            "/native/display/front-light",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "POST",
            "/native/actions/show-brightness-dialog",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "PUT",
            "/native/display/refresh-mode",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
        (
            "POST",
            "/native/display/clean",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let removed_request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router
                .clone()
                .oneshot(removed_request)
                .await
                .unwrap()
                .status(),
            expected,
            "{method} {path}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn window_fullscreen_is_read_and_set_with_reader_authorization() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestDisplayActions::default());
    let router = reader_router(
        fixture_reader_state_with_actions(directory.path(), actions.clone()),
        None,
    );

    for method in ["GET", "POST"] {
        let unauthorized = Request::builder()
            .method(method)
            .uri("/native/window/fullscreen")
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(unauthorized).await.unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "{method}"
        );
    }
    assert!(!*actions.window_fullscreen.lock().unwrap());

    let read = || {
        Request::builder()
            .method("GET")
            .uri("/native/window/fullscreen")
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap()
    };
    let response = router.clone().oneshot(read()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body, serde_json::json!({ "fullscreen": false }));

    let write = |body: &'static str| {
        Request::builder()
            .method("POST")
            .uri("/native/window/fullscreen")
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(write(r#"{"fullscreen":true,"monitor":1}"#))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(!*actions.window_fullscreen.lock().unwrap());

    let response = router
        .clone()
        .oneshot(write(r#"{"fullscreen":true}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(*actions.window_fullscreen.lock().unwrap());

    let response = router.clone().oneshot(read()).await.unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body, serde_json::json!({ "fullscreen": true }));

    let other = tempfile::tempdir().unwrap();
    let unsupported = reader_router(fixture_reader_state(other.path()), None);
    assert_eq!(
        unsupported.clone().oneshot(read()).await.unwrap().status(),
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        unsupported
            .oneshot(write(r#"{"fullscreen":true}"#))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_IMPLEMENTED
    );
}

#[test]
fn external_url_preparation_cannot_expand_past_the_open_limit() {
    let value = format!("https://example.test/{}", "\u{e9}".repeat(800));
    assert!(value.len() < crate::MAX_EXTERNAL_URL_BYTES);
    assert!(crate::device::validate_external_url(&value).is_none());
    let accepted = crate::device::validate_external_url("HTTPS://EXAMPLE.test:443/a/../b").unwrap();
    assert_eq!(accepted.as_str(), "https://example.test/b");
    assert_eq!(
        crate::device::validate_external_url(accepted.as_str()),
        Some(accepted)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn external_urls_are_narrowly_validated_and_require_reader_authorization() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestDisplayActions::default());
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
    let session = lifecycle.load_active().unwrap();
    let request = |body: &str, authorized: bool| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/native/open-external-url")
            .header("host", "reader.test")
            .header(
                "x-elephant-reader-session",
                session.reader_session_id.as_ref(),
            )
            .header(CONTENT_TYPE, "application/json");
        if authorized {
            request = request
                .header(ORIGIN, TEST_READER_ORIGIN)
                .header(AUTHORIZATION, "Bearer secret");
        }
        request.body(Body::from(body.to_owned())).unwrap()
    };

    assert_eq!(
        router
            .clone()
            .oneshot(request(
                r#"{"url":"https://example.test/read?q=1#part"}"#,
                false
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        router
            .clone()
            .oneshot(request(
                r#"{"url":"https://example.test/read?q=1#part"}"#,
                true,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        actions.external_url.lock().unwrap().as_deref(),
        Some("https://example.test/read?q=1#part")
    );

    for url in [
        "next.xhtml",
        "file:///tmp/book",
        "data:text/plain,no",
        "javascript:alert(1)",
        "https://user:secret@example.test/",
        "http://reader.elephant-ladder.localhost/native/display",
        "http://book.elephant-ladder.localhost/EPUB/chapter.xhtml",
        "http://p-publication.localhost/EPUB/chapter.xhtml",
        "http://n-publication.localhost/EPUB/chapter.xhtml",
        "http://m-publication.localhost/EPUB/chapter.xhtml",
    ] {
        let body = serde_json::json!({ "url": url }).to_string();
        assert_eq!(
            router
                .clone()
                .oneshot(request(&body, true))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "{url}"
        );
    }
    for url in [
        "http://localhost:8080/path",
        "https://localhost:8443/path",
        "http://dev.localhost:8080/path",
        "http://127.0.0.1:8080/path",
        "http://192.168.1.20/path",
        "https://10.0.0.20/path",
    ] {
        let body = serde_json::json!({ "url": url }).to_string();
        assert_eq!(
            router
                .clone()
                .oneshot(request(&body, true))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT,
            "{url}"
        );
    }
    assert_eq!(
        router
            .oneshot(request(
                r#"{"url":"https://example.test/","extra":true}"#,
                true,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let unsupported_directory = tempfile::tempdir().unwrap();
    let mut unsupported_state = fixture_reader_state(unsupported_directory.path());
    unsupported_state.lifecycle = lifecycle;
    let unsupported = reader_router(unsupported_state, None)
        .oneshot(request(r#"{"url":"https://example.test/"}"#, true))
        .await
        .unwrap();
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test(flavor = "current_thread")]
async fn external_url_preparation_and_session_fences_have_no_native_effects() {
    let directory = tempfile::tempdir().unwrap();
    let actions = Arc::new(TestDisplayActions::default());
    let mut state = fixture_reader_state(directory.path());
    state.native_actions = actions.clone();
    let lifecycle = state.lifecycle.clone();
    let router = reader_router(state, None);
    let request = |route: &str, id: &str, body: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(format!("/native/{route}-external-url"))
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret")
            .header("x-elephant-reader-session", id)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let body = serde_json::json!({"url": "https://example.test/"});
    for route in ["prepare", "open"] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(route, "absent", body.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
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
    let id = active.reader_session_id.as_ref();
    for (input, normalized, origin) in [
        (
            "HTTPS://ExAmPlE.test:443/a/../read?q=1#part",
            "https://example.test/read?q=1#part",
            "https://example.test",
        ),
        (
            "https://b\u{fc}cher.example:8443/a b",
            "https://xn--bcher-kva.example:8443/a%20b",
            "https://xn--bcher-kva.example:8443",
        ),
        (
            "http://[2001:0db8::1]:80",
            "http://[2001:db8::1]/",
            "http://[2001:db8::1]",
        ),
    ] {
        let response = router
            .clone()
            .oneshot(request("prepare", id, serde_json::json!({"url": input})))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let prepared: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            prepared,
            serde_json::json!({"url": normalized, "origin": origin})
        );
    }
    // A mail link has no origin, so preparation names the one recipient the prompt will show.
    for (input, normalized, address) in [
        (
            "mailto:someone@example.test?subject=Hi",
            "mailto:someone@example.test?subject=Hi",
            "someone@example.test",
        ),
        (
            "mailto:some%2Bone@example.test",
            "mailto:some%2Bone@example.test",
            "some+one@example.test",
        ),
    ] {
        let response = router
            .clone()
            .oneshot(request("prepare", id, serde_json::json!({"url": input})))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{input}");
        let prepared: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            prepared,
            serde_json::json!({"url": normalized, "address": address}),
            "{input}"
        );
    }
    // Cancelling consent means doing nothing after preparation.
    assert!(actions.external_url.lock().unwrap().is_none());
    for route in ["prepare", "open"] {
        for invalid in [
            serde_json::json!({"url": "https://user:secret@example.test/"}),
            serde_json::json!({"url": "https://user@example.test/"}),
            serde_json::json!({"url": "https://:secret@example.test/"}),
            serde_json::json!({"url": "javascript:alert(1)"}),
            // A prompt can only name one recipient honestly, and only an ordinary address.
            serde_json::json!({"url": "mailto:one@example.test,two@example.test"}),
            serde_json::json!({"url": "mailto:"}),
            serde_json::json!({"url": "mailto:nobody"}),
            serde_json::json!({"url": "mailto:some%20one@example.test"}),
            serde_json::json!({"url": "http://p-book.localhost/"}),
            serde_json::json!({"url": "not a URL"}),
            serde_json::json!({"url": "https://example.test/", "extra": true}),
            serde_json::json!({"url": format!("https://example.test/{}", "a".repeat(crate::MAX_EXTERNAL_URL_BYTES))}),
            serde_json::json!({"url": "a".repeat(crate::MAX_EXTERNAL_URL_REQUEST_BYTES)}),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(route, id, invalid))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for token in [None, Some("Bearer wrong")] {
            let mut req = request(route, id, body.clone());
            req.headers_mut().remove(AUTHORIZATION);
            if let Some(token) = token {
                req.headers_mut()
                    .insert(AUTHORIZATION, token.parse().unwrap());
            }
            assert_eq!(
                router.clone().oneshot(req).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        for session in [None, Some("stale"), Some(id)] {
            let mut req = request(route, id, body.clone());
            match session {
                None => {
                    req.headers_mut().remove("x-elephant-reader-session");
                }
                Some(value) if value == id => {
                    req.headers_mut()
                        .append("x-elephant-reader-session", value.parse().unwrap());
                }
                Some(value) => {
                    req.headers_mut()
                        .insert("x-elephant-reader-session", value.parse().unwrap());
                }
            }
            assert_eq!(
                router.clone().oneshot(req).await.unwrap().status(),
                StatusCode::CONFLICT
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
    for route in ["prepare", "open"] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(route, id, body.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
    let replacement = lifecycle.load_active().unwrap();
    let replacement_id = replacement.reader_session_id.as_ref();
    assert_eq!(
        router
            .clone()
            .oneshot(request("prepare", replacement_id, body.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    replacement.revoked.store(true, Ordering::Release);
    for route in ["prepare", "open"] {
        assert_eq!(
            router
                .clone()
                .oneshot(request(route, replacement_id, body.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
    replacement.revoked.store(false, Ordering::Release);
    assert_eq!(
        router
            .clone()
            .oneshot(close_request(replacement_id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for route in ["prepare", "open"] {
        for stale in [id, replacement_id] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(route, stale, body.clone()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CONFLICT
            );
        }
    }
    assert!(actions.external_url.lock().unwrap().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn external_url_dispatch_holds_lifecycle_lock() {
    struct LockedActions {
        lifecycle: Arc<crate::ReaderLifecycle>,
        calls: AtomicU64,
    }
    impl NativeActions for LockedActions {
        fn open_external_url(&self, url: &str) -> Result<(), crate::NativeActionError> {
            assert!(self.lifecycle.control_slot().try_lock().is_err());
            assert_eq!(url, "https://example.test/read");
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let mut state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let actions = Arc::new(LockedActions {
        lifecycle: lifecycle.clone(),
        calls: AtomicU64::new(0),
    });
    state.native_actions = actions.clone();
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
    let active = lifecycle.load_active().unwrap();
    let request = |route: &str, body: serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri(format!("/native/{route}-external-url"))
            .header("host", "reader.test")
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(AUTHORIZATION, "Bearer secret")
            .header(
                "x-elephant-reader-session",
                active.reader_session_id.as_ref(),
            )
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = router
        .clone()
        .oneshot(request(
            "prepare",
            serde_json::json!({"url": "HTTPS://EXAMPLE.test:443/a/../read"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let prepared: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(actions.calls.load(Ordering::Relaxed), 0);
    let approved = serde_json::json!({"url": prepared["url"]});
    assert_eq!(
        router
            .clone()
            .oneshot(request("open", approved.clone()))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(actions.calls.load(Ordering::Relaxed), 1);

    let mut operation = lifecycle.lock().await;
    let pending = tokio::spawn(router.oneshot(request("open", approved)));
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    operation.clear_if(|_| true);
    drop(operation);
    assert_eq!(
        pending.await.unwrap().unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(actions.calls.load(Ordering::Relaxed), 1);
}
