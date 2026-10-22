use super::support::*;

#[test]
fn resource_cache_is_byte_bounded_and_evicts_least_recently_used() {
    let mut cache = crate::snapshot::ResourceCache::default();
    let half = Bytes::from(vec![1; crate::RESOURCE_CACHE_BYTES / 2]);
    let rest = Bytes::from(vec![2; crate::RESOURCE_CACHE_BYTES / 2 + 1]);
    cache.insert("first".to_owned(), half);
    assert!(cache.get("first").is_some());
    assert_eq!(cache.insert("second".to_owned(), rest), 1);
    assert!(cache.bytes <= crate::RESOURCE_CACHE_BYTES);
    assert!(cache.get("first").is_none());
    assert!(cache.get("second").is_some());
    // An oversized resource is refused instead of emptying the cache for it.
    let huge = Bytes::from(vec![3; crate::RESOURCE_CACHE_BYTES + 1]);
    assert_eq!(cache.insert("huge".to_owned(), huge), 0);
    assert!(cache.get("huge").is_none());
    assert!(cache.get("second").is_some());
}

#[test]
fn transformed_cache_is_byte_bounded_and_tracks_hits() {
    let mut cache = crate::snapshot::TransformCache::default();
    let key = |path: &str| crate::snapshot::TransformCacheKey {
        path: path.to_owned(),
        representation: crate::mount::ResponseRepresentation::Source,
        profile: crate::mount::PublicationResponseProfile::SpineLevel,
        nested_publication_base_url: Some(Arc::from("http://nested.test")),
        content_script_nonce: None,
    };
    let first = Bytes::from(vec![1; crate::TRANSFORM_CACHE_BYTES / 2]);
    let second = Bytes::from(vec![2; crate::TRANSFORM_CACHE_BYTES / 2 + 1]);
    assert!(cache.insert(key("first"), first).cached);
    assert!(cache.get(&key("first")).is_some());
    let outcome = cache.insert(key("second"), second);
    assert_eq!(outcome.evictions, 1);
    assert!(cache.bytes <= crate::TRANSFORM_CACHE_BYTES);
    assert!(cache.get(&key("first")).is_none());
    assert!(cache.get(&key("second")).is_some());
    // The nonce is what genuinely partitions one session's instrumented output from another's.
    let mut different_nonce = key("second");
    different_nonce.content_script_nonce = Some(Arc::from("another-nonce"));
    assert!(cache.get(&different_nonce).is_none());

    let oversized = Bytes::from(vec![0; crate::TRANSFORM_CACHE_BYTES + 1]);
    assert!(!cache.insert(key("oversized"), oversized).cached);
    assert!(!cache.entries.contains_key(&key("oversized")));
}

#[test]
fn resource_stream_stops_when_the_receiver_is_dropped() {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    drop(receiver);
    let mut source = Cursor::new(vec![7; 1024 * 1024]);
    crate::mount::stream::stream_reader_range(&mut source, 0, 1024 * 1024, &sender, None).unwrap();
    assert!(source.position() <= crate::RESOURCE_STREAM_CHUNK_BYTES as u64);
}

#[test]
fn maps_every_epub_34_core_image_extension() {
    for (path, expected) in [
        ("cover.gif", "image/gif"),
        ("cover.jpeg", "image/jpeg"),
        ("cover.png", "image/png"),
        ("cover.svg", "image/svg+xml"),
        ("cover.webp", "image/webp"),
        ("cover.avif", "image/avif"),
        ("cover.jxl", "image/jxl"),
    ] {
        assert_eq!(crate::mount::content_type(path), expected);
    }
}

#[test]
fn maps_embedded_fonts_as_measurement_dependencies() {
    for (path, expected) in [
        ("reader.otf", "font/otf"),
        ("reader.ttf", "font/ttf"),
        ("reader.woff", "font/woff"),
        ("reader.woff2", "font/woff2"),
    ] {
        let mime = crate::mount::content_type(path);
        assert_eq!(mime, expected);
        assert!(crate::mount::measurement_dependency_mime(mime));
    }
}

#[test]
fn decodes_only_canonical_publication_paths() {
    let path =
        decode_publication_path(&"/EPUB/text/conformance.xhtml".parse::<Uri>().unwrap()).unwrap();
    assert_eq!(path.as_str(), "EPUB/text/conformance.xhtml");
    for hostile in [
        "/EPUB/%2fsecret",
        "/EPUB/%252fsecret",
        "/EPUB/../secret",
        "/EPUB//secret",
        "/EPUB/%zz",
    ] {
        assert!(decode_publication_path(&hostile.parse::<Uri>().unwrap()).is_err());
    }
}

#[test]
fn parses_fixture_ranges() {
    assert_eq!(parse_range(None, 44), RangeDecision::Full);
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=0-3")), 44),
        RangeDecision::Partial { start: 0, end: 3 }
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=999-1000")), 44),
        RangeDecision::Unsatisfiable
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=-4")), 44),
        RangeDecision::Partial { start: 40, end: 43 }
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=-100")), 44),
        RangeDecision::Partial { start: 0, end: 43 }
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=-0")), 44),
        RangeDecision::Unsatisfiable
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=10-")), 44),
        RangeDecision::Partial { start: 10, end: 43 }
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=5-2")), 44),
        RangeDecision::Unsatisfiable
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=0-1,4-5")), 44),
        RangeDecision::Full
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=0-0")), 0),
        RangeDecision::Unsatisfiable
    );
    assert_eq!(
        parse_range(Some(&HeaderValue::from_static("bytes=-1")), 0),
        RangeDecision::Unsatisfiable
    );
}

#[test]
fn single_range_header_ignores_repeated_fields() {
    let mut headers = HeaderMap::new();
    assert!(single_range_header(&headers).is_none());
    headers.append(RANGE, HeaderValue::from_static("bytes=0-1"));
    assert_eq!(single_range_header(&headers).unwrap(), "bytes=0-1");
    headers.append(RANGE, HeaderValue::from_static("bytes=2-3"));
    assert!(single_range_header(&headers).is_none());
}

#[test]
fn origin_agent_cluster_is_limited_to_publication_document_mimes() {
    for mime in [
        "text/html",
        "text/html; charset=utf-8",
        "application/xhtml+xml",
        "application/xhtml+xml; charset=utf-8",
        "image/svg+xml",
    ] {
        let response = crate::mount::representation::publication_response(
            StatusCode::OK,
            mime,
            Bytes::new(),
            0,
            "none",
            None,
            "default-src 'none'",
        );
        assert_eq!(response.headers()["origin-agent-cluster"], "?1", "{mime}");
    }

    for mime in ["application/octet-stream", "image/png", "text/css"] {
        let response = crate::mount::representation::publication_response(
            StatusCode::OK,
            mime,
            Bytes::new(),
            0,
            "bytes",
            None,
            "default-src 'none'",
        );
        assert!(
            !response.headers().contains_key("origin-agent-cluster"),
            "{mime}"
        );
    }
}

/// A response whose body has not been drained must not pin the shared archive reader.
#[tokio::test(flavor = "current_thread")]
async fn an_undrained_resource_response_does_not_block_another_read() {
    use epub_stack::EpubPath;
    // Larger than the streaming channel's buffered chunks, smaller than the buffering bound.
    let large = vec![b' '; 1024 * 1024 + 1];
    let archive = buffered_read_archive(&large);
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
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
    let admission = Arc::new(Semaphore::new(2));
    let context = || crate::mount::ResourceStreamContext {
        csp: crate::mount::measurement_dependency_csp(),
        admission: admission.clone(),
        session: Some(session.clone()),
        request_origin: None,
        reader_cover_path: None,
        reader_cover_origin: None,
        sidecar: None,
    };
    let undrained = crate::mount::stream_imported_resource(
        session.entry.epub.clone(),
        EpubPath::new("EPUB/large.css").unwrap(),
        &session.entry.responses.resources["EPUB/large.css"],
        false,
        None,
        context(),
    )
    .await;
    assert_eq!(undrained.status(), StatusCode::OK);
    // Pull one frame so a streaming producer is certainly running, then stop reading: a
    // producer that holds the archive across the drain is now blocked on a full channel.
    let mut pending = undrained.into_body();
    let frame = pending.frame().await.unwrap().unwrap();
    assert!(!frame.into_data().unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The bytes must arrive, not merely the response: a streaming read blocks while its
    // body is produced, so only collecting the body observes a pinned archive.
    let second = tokio::time::timeout(Duration::from_secs(5), async {
        let response = crate::mount::stream_imported_resource(
            session.entry.epub.clone(),
            EpubPath::new("EPUB/styles").unwrap(),
            &session.entry.responses.resources["EPUB/styles"],
            false,
            None,
            context(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        response.into_body().collect().await.unwrap().to_bytes()
    })
    .await;
    // Release the pending body first: a regression then fails here rather than blocking
    // runtime shutdown behind a read that still holds the archive.
    drop(pending);
    let second = second.expect("an undrained response must not block another resource read");
    assert_eq!(second.as_ref(), BUFFERED_READ_CSS);
}

#[test]
fn publication_validator_covers_instrumentation_inputs() {
    let validator = |instrumentation| {
        crate::mount::publication_validator(
            "snapshot",
            "EPUB/text.xhtml",
            "xhtml",
            crate::mount::PublicationResponseProfile::SpineLevel,
            instrumentation,
        )
    };
    assert_eq!(validator("agent-a"), validator("agent-a"));
    assert_ne!(validator("agent-a"), validator("agent-b"));
    assert_ne!(validator(""), validator("agent-a"));
}

#[tokio::test(flavor = "current_thread")]
async fn publication_responses_revalidate_against_a_snapshot_scoped_validator() {
    use axum::http::header::{ETAG, IF_NONE_MATCH};
    let large = vec![b' '; 1024];
    let archive = buffered_read_archive(&large);
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let imported = ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let foreground = crate::mount::imported_publication_router(imported);
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
    let first = foreground
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            "/EPUB/styles",
            authority,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[CACHE_CONTROL], "private, no-cache");
    let validator = first.headers()[ETAG].to_str().unwrap().to_owned();

    let mut conditional = imported_publication_request("GET", "/EPUB/styles", authority);
    conditional
        .headers_mut()
        .insert(IF_NONE_MATCH, HeaderValue::from_str(&validator).unwrap());
    let revalidated = foreground.clone().oneshot(conditional).await.unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(revalidated.headers()[ETAG], validator.as_str());
    assert_eq!(revalidated.headers()[CACHE_CONTROL], "private, no-cache");
    assert!(
        revalidated
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .is_empty()
    );

    // One resource's validator must never satisfy another resource.
    let mut mismatched = imported_publication_request("GET", "/EPUB/large.css", authority);
    mismatched
        .headers_mut()
        .insert(IF_NONE_MATCH, HeaderValue::from_str(&validator).unwrap());
    assert_eq!(
        foreground
            .clone()
            .oneshot(mismatched)
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // A revalidation passes the same session gate as a full response.
    session
        .revoked
        .store(true, std::sync::atomic::Ordering::Release);
    let mut revoked = imported_publication_request("GET", "/EPUB/styles", authority);
    revoked
        .headers_mut()
        .insert(IF_NONE_MATCH, HeaderValue::from_str(&validator).unwrap());
    assert_eq!(
        foreground.oneshot(revoked).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

/// The reader's stand-in image and the page frame fetch the same bytes moments apart and
/// cannot share a browser cache entry, so the second read must not inflate the entry again.
#[tokio::test(flavor = "current_thread")]
async fn a_cached_resource_is_served_without_the_streaming_permit() {
    let large = vec![7_u8; 4096];
    let archive = buffered_read_archive(&large);
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
    let admission = Arc::new(Semaphore::new(1));
    let imported = ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: admission.clone(),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let foreground = crate::mount::imported_publication_router(imported);
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
    let first = foreground
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            "/EPUB/large.css",
            authority,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let bytes = first.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.len(), large.len());

    // Hold the only streaming permit: a cached resource must not wait for admission.
    let held = admission.try_acquire_owned().unwrap();
    let cached = tokio::time::timeout(
        Duration::from_secs(5),
        foreground.oneshot(imported_publication_request(
            "GET",
            "/EPUB/large.css",
            authority,
        )),
    )
    .await
    .expect("a cached resource must not wait for streaming admission")
    .unwrap();
    assert_eq!(cached.status(), StatusCode::OK);
    assert_eq!(
        cached.into_body().collect().await.unwrap().to_bytes(),
        bytes
    );
    drop(held);
}

#[tokio::test(flavor = "current_thread")]
async fn native_fixed_image_roll_representations_preserve_raw_archive_and_session_gates() {
    for (fixture, has_images) in [
        ("lay-roll-images-in-spine", true),
        ("lay-roll-images-mixed", true),
        ("lay-roll-embedded-images", false),
        ("lay-roll-embedded-images-svg", false),
    ] {
        let archive = std::fs::read(format!(
            "{}/../../../../navigator/tests/fixtures/epub34/{fixture}.epub",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let state = fixture_reader_state(directory.path());
        let active = state.lifecycle.active_view();
        let imported = ImportedPublicationState {
            asset_origin: Arc::from(TEST_READER_ORIGIN),
            active_reader_session: active.clone(),
            snapshots: state.snapshots.clone(),
            stream_admission: Arc::new(Semaphore::new(1)),
            transform_admission: Arc::new(Semaphore::new(1)),
        };
        let foreground = crate::mount::imported_publication_router(imported.clone());
        let reader = reader_router(state, None);
        let created = reader
            .clone()
            .oneshot(import_request_with(archive.clone()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED, "{fixture}");
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
        let images = session
            .entry
            .responses
            .resources
            .iter()
            .filter(|(_, rule)| matches!(rule.foreground, crate::snapshot::Foreground::FixedImage))
            .collect::<Vec<_>>();
        assert_eq!(!images.is_empty(), has_images, "{fixture}");
        for (path, rule) in images {
            let raw_uri = format!("/{path}");
            let wrapper_uri = format!("{raw_uri}?{}", crate::IMAGE_DOCUMENT_QUERY);
            let authority = session.authorities.foreground_authority.as_ref();
            let raw = session
                .entry
                .epub
                .bytes(&epub_stack::EpubPath::new(path).unwrap())
                .unwrap();
            let mut range_request = imported_publication_request("GET", &raw_uri, authority);
            range_request.headers_mut().insert(
                axum::http::header::RANGE,
                HeaderValue::from_static("bytes=0-3"),
            );
            let ranged = foreground.clone().oneshot(range_request).await.unwrap();
            assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                ranged.headers()[CONTENT_RANGE],
                format!("bytes 0-3/{}", raw.len())
            );
            assert_eq!(
                ranged
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .as_ref(),
                &raw[..4]
            );
            assert_eq!(
                foreground
                    .clone()
                    .oneshot(imported_publication_request(
                        "GET",
                        &wrapper_uri,
                        "wrong.test"
                    ))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
            // The reader paints an entering page's own bytes while its frame is still
            // uncomposited, so a spine image is readable by the reader origin alone.
            let mut reader_request = imported_publication_request("GET", &raw_uri, authority);
            reader_request.headers_mut().insert(
                axum::http::header::ORIGIN,
                HeaderValue::from_static(TEST_READER_ORIGIN),
            );
            let reader_image = foreground.clone().oneshot(reader_request).await.unwrap();
            assert_eq!(reader_image.status(), StatusCode::OK);
            assert_eq!(
                reader_image.headers()["access-control-allow-origin"],
                TEST_READER_ORIGIN
            );
            assert_eq!(
                reader_image.headers()["cross-origin-resource-policy"],
                "cross-origin"
            );
            let mut foreign_request = imported_publication_request("GET", &raw_uri, authority);
            foreign_request.headers_mut().insert(
                axum::http::header::ORIGIN,
                HeaderValue::from_static("http://evil.test"),
            );
            let foreign = foreground.clone().oneshot(foreign_request).await.unwrap();
            assert_eq!(
                foreign.headers()["cross-origin-resource-policy"],
                "same-origin"
            );
            assert!(
                !foreign
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
            for method in ["GET", "HEAD"] {
                let response = foreground
                    .clone()
                    .oneshot(imported_publication_request(method, &raw_uri, authority))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()[CONTENT_TYPE], rule.mime);
                assert_eq!(response.headers()[CONTENT_LENGTH], raw.len().to_string());
                assert_eq!(response.headers()[ACCEPT_RANGES], "bytes");
                let body = response.into_body().collect().await.unwrap().to_bytes();
                if method == "GET" {
                    assert_eq!(blake3::hash(&body), blake3::hash(&raw));
                } else {
                    assert!(body.is_empty());
                }

                let mut request = imported_publication_request(method, &wrapper_uri, authority);
                request
                    .headers_mut()
                    .insert("range", HeaderValue::from_static("bytes=0-3"));
                let response = foreground.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.headers()[CONTENT_TYPE],
                    "application/xhtml+xml; charset=utf-8"
                );
                assert_eq!(response.headers()[ACCEPT_RANGES], "none");
                assert!(!response.headers().contains_key(CONTENT_RANGE));
                let length = response.headers()[CONTENT_LENGTH]
                    .to_str()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let csp = response.headers()[CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .to_owned();
                assert!(csp.contains(&crate::reader::content_agent_url(TEST_READER_ORIGIN)));
                let body = response.into_body().collect().await.unwrap().to_bytes();
                if method == "GET" {
                    assert_eq!(body.len(), length);
                    let text = std::str::from_utf8(&body).unwrap();
                    assert!(text.contains("data-epub-navigator-image-document=\"1\""));
                    assert!(text.contains(&crate::reader::content_agent_path()));
                    assert!(!text.contains(crate::IMAGE_DOCUMENT_QUERY));
                    assert!(!text.contains("name=\"viewport\""));
                } else {
                    assert!(body.is_empty());
                }
            }
            for query in [
                "other=1",
                "epub-navigator-image-document=0",
                "epub-navigator-image-document=1&other=1",
                "epub-navigator-image-document=%31",
            ] {
                assert_eq!(
                    foreground
                        .clone()
                        .oneshot(imported_publication_request(
                            "GET",
                            &format!("{raw_uri}?{query}"),
                            authority
                        ))
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::BAD_REQUEST
                );
            }
            let nested = crate::mount::imported_publication_resource_for_profile(
                imported.clone(),
                session.clone(),
                crate::mount::PublicationResponseProfile::ContainerConstrained,
                axum::http::Method::GET,
                wrapper_uri.parse().unwrap(),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(nested.status(), StatusCode::FORBIDDEN);
        }
        let hash = blake3::hash(&archive).to_hex().to_string();
        assert_eq!(
            foreground
                .clone()
                .oneshot(imported_publication_request(
                    "GET",
                    "/missing.png?epub-navigator-image-document=1",
                    &session.authorities.foreground_authority,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        for (path, rule) in &session.entry.responses.resources {
            if rule.mime == "image/png"
                && !rule.undeclared
                && !matches!(rule.foreground, crate::snapshot::Foreground::FixedImage)
            {
                assert_eq!(
                    foreground
                        .clone()
                        .oneshot(imported_publication_request(
                            "GET",
                            &format!("/{path}?{}", crate::IMAGE_DOCUMENT_QUERY),
                            &session.authorities.foreground_authority,
                        ))
                        .await
                        .unwrap()
                        .status(),
                    StatusCode::FORBIDDEN
                );
            }
        }
        assert_eq!(
            std::fs::read(directory.path().join("archives/blobs").join(hash)).unwrap(),
            archive
        );
        session
            .revoked
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            foreground
                .oneshot(imported_publication_request(
                    "GET",
                    "/anything.png?epub-navigator-image-document=1",
                    &session.authorities.foreground_authority
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[test]
fn image_wrapper_escapes_canonical_resource_url() {
    let path = epub_stack::EpubPath::new("EPUB/a &\"<>'.png").unwrap();
    let wrapper = crate::mount::image_document_source(&path);
    assert!(wrapper.contains("src=\"/EPUB/a%20%26%22%3C%3E%27%2Epng\""));
    assert_eq!(wrapper.matches("<img ").count(), 1);
    assert!(!wrapper.contains("<script"));
}

/// EPUB keeps prefixed properties the engine drops, and a stylesheet cannot be restated
/// in the document because its relative URLs resolve against the sheet. Every plane that
/// serves one therefore serves it restated, and leaves a sheet without them untouched.
#[tokio::test]
async fn undecodable_core_image_types_are_restated_as_png_for_the_engine_that_asked() {
    use axum::http::header::ETAG;

    const WEBVIEW_ACCEPT: &str = "image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8";
    const DESKTOP_ACCEPT: &str = "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8";

    let archive = buffered_read_archive(&[b' '; 16]);
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let active = state.lifecycle.active_view();
    let imported = ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let measurement_state = crate::mount::MeasurementPublicationState {
        reader_origin: Arc::from(TEST_READER_ORIGIN),
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let foreground = crate::mount::imported_publication_router(imported);
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

    let request = |path: &str, accept: &str| {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("host", authority)
            .header("accept", accept)
            .body(Body::empty())
            .unwrap()
    };

    // An engine that never names either format is served PNG for both.
    for path in ["/EPUB/probe.avif", "/EPUB/probe.jxl"] {
        let restated = foreground
            .clone()
            .oneshot(request(path, WEBVIEW_ACCEPT))
            .await
            .unwrap();
        assert_eq!(restated.status(), StatusCode::OK, "{path}");
        assert_eq!(restated.headers()[CONTENT_TYPE], "image/png", "{path}");
        assert_eq!(restated.headers()["vary"], "Accept", "{path}");
        let body = restated.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..8], b"\x89PNG\r\n\x1a\n", "{path}");
    }

    // An engine that names AVIF is served the authored bytes, and still needs the PNG
    // for JPEG XL, which no engine decodes.
    let authored = foreground
        .clone()
        .oneshot(request("/EPUB/probe.avif", DESKTOP_ACCEPT))
        .await
        .unwrap();
    assert_eq!(authored.status(), StatusCode::OK);
    assert_eq!(authored.headers()[CONTENT_TYPE], "image/avif");
    assert_eq!(
        authored
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        PROBE_AVIF
    );
    let restated = foreground
        .clone()
        .oneshot(request("/EPUB/probe.jxl", DESKTOP_ACCEPT))
        .await
        .unwrap();
    assert_eq!(restated.status(), StatusCode::OK);
    assert_eq!(restated.headers()[CONTENT_TYPE], "image/png");

    // The two representations of one resource must not share a validator, or a cache
    // would hand one engine the other's bytes.
    let adapted_validator = foreground
        .clone()
        .oneshot(request("/EPUB/probe.avif", WEBVIEW_ACCEPT))
        .await
        .unwrap()
        .headers()[ETAG]
        .clone();
    let authored_validator = foreground
        .clone()
        .oneshot(request("/EPUB/probe.avif", DESKTOP_ACCEPT))
        .await
        .unwrap()
        .headers()[ETAG]
        .clone();
    assert_ne!(adapted_validator, authored_validator);

    // Pagination measures the same pixels the reader is shown.
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    let measurement_origin = bootstrap["measurementOrigin"].as_str().unwrap();
    let measured = measurement_publication_router(measurement_state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/EPUB/probe.avif")
                .header("host", measurement_origin.strip_prefix("http://").unwrap())
                .header("accept", WEBVIEW_ACCEPT)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(measured.status(), StatusCode::OK);
    assert_eq!(measured.headers()[CONTENT_TYPE], "image/png");
}

#[tokio::test(flavor = "current_thread")]
async fn manifest_stylesheets_restate_prefixed_declarations_on_every_plane() {
    let archive = buffered_read_archive(&[b' '; 16]);
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let active = state.lifecycle.active_view();
    let imported = ImportedPublicationState {
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        snapshots: state.snapshots.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let measurement_state = crate::mount::MeasurementPublicationState {
        reader_origin: Arc::from(TEST_READER_ORIGIN),
        asset_origin: Arc::from(TEST_READER_ORIGIN),
        active_reader_session: active.clone(),
        stream_admission: Arc::new(Semaphore::new(1)),
        transform_admission: Arc::new(Semaphore::new(1)),
    };
    let foreground = crate::mount::imported_publication_router(imported);
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

    let restated = foreground
        .clone()
        .oneshot(imported_publication_request(
            "GET",
            "/EPUB/prefixed.css",
            authority,
        ))
        .await
        .unwrap();
    assert_eq!(restated.status(), StatusCode::OK);
    assert!(
        restated.headers()[CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let body = restated.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        body.as_ref(),
        b"p{-epub-hyphens:auto;hyphens: auto;color:red}"
    );

    let authored = foreground
        .oneshot(imported_publication_request(
            "GET",
            "/EPUB/styles",
            authority,
        ))
        .await
        .unwrap();
    assert_eq!(authored.status(), StatusCode::OK);
    assert_eq!(
        authored
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        BUFFERED_READ_CSS
    );

    // Pagination is measured against the same declarations the reader is served.
    let bootstrap = pending_reading_bootstrap(&lifecycle);
    let measurement_origin = bootstrap["measurementOrigin"].as_str().unwrap();
    let measured = measurement_publication_router(measurement_state)
        .oneshot(measurement_request(
            "GET",
            "/EPUB/prefixed.css",
            measurement_origin.strip_prefix("http://").unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(measured.status(), StatusCode::OK);
    assert_eq!(
        measured
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        b"p{-epub-hyphens:auto;hyphens: auto;color:red}"
    );
}

#[tokio::test]
async fn the_manifest_decides_what_is_served_and_what_a_foreign_type_is_served_instead() {
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><img src="star.psd" alt="star"/><img src="loose.png" alt="loose"/></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let archive = synthetic_epub(
        br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">manifest-authority</dc:identifier><dc:title>Manifest authority</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-10-06T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="star" href="star.psd" media-type="image/vnd.adobe.photoshop" fallback="star-png"/><item id="star-png" href="star.png" media-type="image/png"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
        &[
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/star.psd", b"8BPS not an image a browser decodes"),
            ("EPUB/star.png", PROBE_PNG),
            ("EPUB/loose.png", PROBE_PNG),
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
    let get = |path: &str| {
        foreground
            .clone()
            .oneshot(imported_publication_request("GET", path, authority))
    };

    // The container holds it, the manifest does not list it, so it is not a publication resource.
    assert_eq!(
        get("/EPUB/loose.png").await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    let declared = get("/EPUB/star.png").await.unwrap();
    assert_eq!(declared.status(), StatusCode::OK);
    assert_eq!(declared.headers()[CONTENT_TYPE], "image/png");

    // A foreign type is answered at its own URL with the first core media type behind it.
    let foreign = get("/EPUB/star.psd").await.unwrap();
    assert_eq!(foreign.status(), StatusCode::OK);
    assert_eq!(foreign.headers()[CONTENT_TYPE], "image/png");
    let body = foreign.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], PROBE_PNG);
}

#[tokio::test]
async fn an_href_the_manifest_could_not_resolve_withholds_nothing() {
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><img src="/media/star.png" alt="star"/></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    // OCF forbids a path-absolute href, so the declaration resolves to nothing, but a browser
    // still follows the URL to the entry the manifest meant.
    let archive = synthetic_epub(
        br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">unresolved-href</dc:identifier><dc:title>Unresolved href</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-10-06T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="star" href="/media/star.png" media-type="image/png"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
        &[
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/nav.xhtml", nav),
            ("media/star.png", PROBE_PNG),
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
    let served = foreground
        .oneshot(imported_publication_request(
            "GET",
            "/media/star.png",
            session.authorities.foreground_authority.as_ref(),
        ))
        .await
        .unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(served.headers()[CONTENT_TYPE], "image/png");
}

#[tokio::test]
async fn a_note_is_read_out_of_a_document_this_publication_serves_as_xhtml() {
    use axum::http::header::AUTHORIZATION;

    let chapter = br##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Chapter</title></head><body><p>Text<a epub:type="noteref" href="notes.xhtml#n1">1</a></p></body></html>"##;
    let notes = br##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Notes</title></head><body><aside epub:type="footnote" id="n1" dir="rtl">Note <em>one</em>.</aside></body></html>"##;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    let archive = synthetic_epub(
        br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">notes</dc:identifier><dc:title>Notes</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-10-06T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="notes" href="notes.xhtml" media-type="application/xhtml+xml"/><item id="css" href="styles.css" media-type="text/css"/></manifest><spine><itemref idref="chapter"/><itemref idref="notes"/></spine></package>"#,
        &[
            ("EPUB/chapter.xhtml", chapter),
            ("EPUB/notes.xhtml", notes),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/styles.css", b"p{color:red}"),
        ],
    );
    let directory = tempfile::tempdir().unwrap();
    let state = fixture_reader_state(directory.path());
    let active = state.lifecycle.active_view();
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
        reader
            .clone()
            .oneshot(open_request(id))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let session_id = active.load().unwrap().reader_session_id.to_string();
    let note = |resource: &str, fragment: &str, token: &str| {
        Request::builder()
            .method("GET")
            .uri(format!(
                "/native/reader-sessions/{session_id}/note?resource={resource}&fragment={fragment}"
            ))
            .header("host", "reader.test")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header("x-elephant-request", "trusted-reader")
            .body(Body::empty())
            .unwrap()
    };

    let found = reader
        .clone()
        .oneshot(note("EPUB%2Fnotes.xhtml", "n1", "secret"))
        .await
        .unwrap();
    assert_eq!(found.status(), StatusCode::OK);
    let found: serde_json::Value =
        serde_json::from_slice(&found.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(found["markup"], "Note <em>one</em>.");
    assert_eq!(found["direction"], "rtl");
    assert_eq!(found["language"], serde_json::Value::Null);

    for (resource, fragment, token, status) in [
        // A stylesheet is served, but it is not a document a reference can point into.
        ("EPUB%2Fstyles.css", "n1", "secret", StatusCode::NOT_FOUND),
        (
            "EPUB%2Fmissing.xhtml",
            "n1",
            "secret",
            StatusCode::NOT_FOUND,
        ),
        (
            "EPUB%2Fnotes.xhtml",
            "absent",
            "secret",
            StatusCode::NOT_FOUND,
        ),
        ("..%2Fescape.xhtml", "n1", "secret", StatusCode::BAD_REQUEST),
        (
            "EPUB%2Fnotes.xhtml",
            "n1",
            "wrong",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        assert_eq!(
            reader
                .clone()
                .oneshot(note(resource, fragment, token))
                .await
                .unwrap()
                .status(),
            status,
            "{resource} #{fragment}"
        );
    }
}
