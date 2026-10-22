use super::support::*;
use crate::reader::CONTENT_AGENT;

#[test]
fn selected_cover_cors_headers_are_exact_origin_only() {
    let facts = serde_json::json!({
        "resources": {
            "coverPath": "EPUB/images/cover.png",
            "selections": {
                "cover": { "state": "selected", "resource": 0 }
            },
            "resources": [
                { "address": { "kind": "local", "value": "EPUB/images/cover.png" } }
            ]
        }
    });
    assert_eq!(
        crate::library::selected_cover_path(&facts).as_deref(),
        Some("EPUB/images/cover.png")
    );

    let mut headers = HeaderMap::new();
    crate::library::apply_reader_cover_cors_headers(&mut headers, Some("http://reader.test"));
    assert_eq!(headers["access-control-allow-origin"], "http://reader.test");
    assert_eq!(headers["cross-origin-resource-policy"], "cross-origin");
    assert_eq!(headers["vary"], "Origin");
}

#[test]
fn reader_cover_cors_compares_against_the_trusted_reader_origin_not_the_publication_origin() {
    let facts = serde_json::json!({
        "resources": {
            "coverPath": "EPUB/images/cover.svg",
            "selections": {
                "cover": { "state": "selected", "resource": 0 }
            },
            "resources": [
                { "address": { "kind": "local", "value": "EPUB/images/cover.svg" } }
            ]
        }
    });
    let cover_path = EpubPath::new("EPUB/images/cover.svg").unwrap();
    let reader_origin = "http://reader.elephant-ladder.localhost:37163";
    let publication_origin = "http://p-2dc3b82b35e036dc61332dc5e6ff6bff.localhost:36023";

    // A request whose Origin is the trusted reader origin receives the exception.
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ORIGIN, HeaderValue::from_static(reader_origin));
    let mut response = axum::response::Response::new(axum::body::Body::empty());
    crate::library::apply_reader_cover_cors(
        &mut response,
        &cover_path,
        &request_headers,
        reader_origin,
        &facts,
    );
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        reader_origin
    );
    assert_eq!(
        response.headers()["cross-origin-resource-policy"],
        "cross-origin"
    );

    // A request whose Origin is the publication origin itself must not receive the
    // exception: comparing against the publication origin was the actual regression.
    let mut publication_origin_headers = HeaderMap::new();
    publication_origin_headers.insert(ORIGIN, HeaderValue::from_static(publication_origin));
    let mut response = axum::response::Response::new(axum::body::Body::empty());
    crate::library::apply_reader_cover_cors(
        &mut response,
        &cover_path,
        &publication_origin_headers,
        reader_origin,
        &facts,
    );
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );

    // An unrelated attacker origin must not receive the exception either.
    let mut attacker_headers = HeaderMap::new();
    attacker_headers.insert(ORIGIN, HeaderValue::from_static("http://attacker.test"));
    let mut response = axum::response::Response::new(axum::body::Body::empty());
    crate::library::apply_reader_cover_cors(
        &mut response,
        &cover_path,
        &attacker_headers,
        reader_origin,
        &facts,
    );
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn serves_reader_serif_assets_as_public_noncredentialed_resources() {
    let directory = tempfile::tempdir().unwrap();
    let router = fixture_reader_router(directory.path());
    let stylesheet = router
        .clone()
        .oneshot(reader_font_request("GET", "reader-serif.css", None))
        .await
        .unwrap();
    assert_eq!(stylesheet.status(), StatusCode::OK);
    assert_eq!(
        stylesheet.headers()[CONTENT_TYPE],
        "text/css; charset=utf-8"
    );
    assert_eq!(stylesheet.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        stylesheet.headers()["cross-origin-resource-policy"],
        "cross-origin"
    );
    assert!(!stylesheet.headers().contains_key("vary"));
    assert_eq!(
        stylesheet.headers()[CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let stylesheet = String::from_utf8(
        stylesheet
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(stylesheet.contains("Navigator Reader Serif"));
    assert!(!stylesheet.contains("Literata Variable"));

    let bundle_id = reader_font_bundle_id().unwrap();
    let font_name = reader_assets()
        .get_dir(format!("public/reader-fonts/{bundle_id}/files"))
        .and_then(|fonts| fonts.files().next())
        .and_then(|font| font.path().file_name())
        .and_then(|name| name.to_str())
        .unwrap();
    let font = router
        .clone()
        .oneshot(reader_font_request(
            "GET",
            &format!("files/{font_name}"),
            Some("null"),
        ))
        .await
        .unwrap();
    assert_eq!(font.status(), StatusCode::OK);
    assert_eq!(font.headers()[CONTENT_TYPE], "font/woff2");
    assert_eq!(font.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        font.headers()["cross-origin-resource-policy"],
        "cross-origin"
    );
    let font = font.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&font[..4], b"wOF2");
    for origin in [
        Some("http://publication.test"),
        Some("http://measurement.test"),
        Some("http://attacker.test"),
    ] {
        assert_eq!(
            router
                .clone()
                .oneshot(reader_font_request("HEAD", "reader-serif.css", origin))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        router
            .clone()
            .oneshot(imported_content_agent_request("http://attacker.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let opaque_agent = router
        .clone()
        .oneshot(imported_content_agent_request("null"))
        .await
        .unwrap();
    assert_eq!(opaque_agent.status(), StatusCode::OK);
    assert_eq!(opaque_agent.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        opaque_agent.headers()["cross-origin-resource-policy"],
        "cross-origin"
    );
    let originless_agent = Request::builder()
        .method("GET")
        .uri(content_agent_path())
        .header("host", "reader.test")
        .body(Body::empty())
        .unwrap();
    let originless_agent = router.clone().oneshot(originless_agent).await.unwrap();
    assert_eq!(originless_agent.status(), StatusCode::OK);
    assert_eq!(
        originless_agent.headers()["access-control-allow-origin"],
        "*"
    );
    assert_eq!(
        originless_agent.headers()[CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        originless_agent
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
        CONTENT_AGENT
    );
    for stale_path in [
        "/public/content-agent.js".to_owned(),
        format!("/public/content-agent/{}.js", "0".repeat(64)),
        format!("/public/content-agent/{}.mjs", content_agent_sha256()),
    ] {
        let stale = Request::builder()
            .method("GET")
            .uri(stale_path)
            .header("host", "reader.test")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(stale).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        router
            .clone()
            .oneshot(reader_font_request("GET", "files/not-a-font.css", None,))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let stale = Request::builder()
        .method("GET")
        .uri(format!(
            "/public/reader-fonts/{}/reader-serif.css",
            "0".repeat(64)
        ))
        .header("host", "reader.test")
        .header(ORIGIN, "http://publication.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.oneshot(stale).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

/// The measurement basis must depend on what a publication frame loads and on nothing else.
///
/// An earlier build derived it by hashing every emitted chunk, stylesheet and interface font,
/// so any interface change discarded every measured page on the device. Recomputing it here
/// from its declared inputs fails if that set ever widens again.
#[test]
fn the_measurement_basis_is_the_agent_the_reader_fonts_and_the_native_revision() {
    use sha2::{Digest, Sha256};

    let mut expected = Sha256::new();
    for part in [
        crate::reader::content_agent_sha256(),
        crate::reader::reader_font_bundle_id().expect("the test reader bundle has reader fonts"),
        crate::NATIVE_MEASUREMENT_REVISION,
    ] {
        expected.update((part.len() as u64).to_be_bytes());
        expected.update(part.as_bytes());
    }
    assert_eq!(
        crate::reader::measurement_basis_digest(),
        format!("{:x}", expected.finalize())
    );
    assert_eq!(crate::reader::measurement_basis_digest().len(), 64);
}
