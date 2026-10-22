use super::support::*;
use crate::http::session_headers;

#[test]
fn accepts_only_strict_non_authorizing_trace_ids() {
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::TRACE_HEADER,
        HeaderValue::from_static("0123456789abcdef0123456789abcdef"),
    );
    assert_eq!(
        crate::session::request_trace_id(&headers).unwrap(),
        "0123456789abcdef0123456789abcdef"
    );
    headers.insert(
        crate::TRACE_HEADER,
        HeaderValue::from_static("0123456789ABCDEF0123456789ABCDEF"),
    );
    assert!(crate::session::request_trace_id(&headers).is_err());
}

#[test]
fn accepts_only_positive_open_operation_numbers() {
    let mut headers = HeaderMap::new();
    assert!(crate::session::request_open_operation(&headers).is_err());
    headers.insert(crate::OPEN_OPERATION_HEADER, HeaderValue::from_static("42"));
    assert_eq!(
        crate::session::request_open_operation(&headers).unwrap(),
        42
    );
    headers.insert(crate::OPEN_OPERATION_HEADER, HeaderValue::from_static("0"));
    assert!(crate::session::request_open_operation(&headers).is_err());
}

#[test]
fn authenticates_only_the_exact_reader_request() {
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static(TEST_READER_ORIGIN));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
    assert_eq!(
        authorize(&headers, TEST_READER_ORIGIN, "secret", true),
        Ok(())
    );
    headers.insert("origin", HeaderValue::from_static("http://127.0.0.1:49152"));
    assert_eq!(
        authorize(&headers, TEST_READER_ORIGIN, "secret", true),
        Err("invalid origin")
    );
}

#[test]
fn authenticates_safe_reads_without_requiring_an_origin_header() {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
    headers.insert(
        "x-elephant-request",
        HeaderValue::from_static("trusted-reader"),
    );
    assert_eq!(
        authorize_safe_read(&headers, TEST_READER_ORIGIN, "secret"),
        Ok(())
    );

    headers.remove("x-elephant-request");
    headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
    assert_eq!(
        authorize_safe_read(&headers, TEST_READER_ORIGIN, "secret"),
        Ok(())
    );
    headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    headers.insert(
        "x-elephant-request",
        HeaderValue::from_static("trusted-reader"),
    );
    assert_eq!(
        authorize_safe_read(&headers, TEST_READER_ORIGIN, "secret"),
        Err("invalid fetch site")
    );

    headers.remove("sec-fetch-site");
    headers.insert(ORIGIN, HeaderValue::from_static("http://attacker.test"));
    assert_eq!(
        authorize_safe_read(&headers, TEST_READER_ORIGIN, "secret"),
        Err("invalid origin")
    );
}

#[test]
fn validates_tokens_and_reader_assets() {
    assert!(constant_time_equal(b"same", b"same"));
    assert!(!constant_time_equal(b"same", b"some"));
    // A prefix, an extension, and the empty value must never compare equal to a
    // credential, whatever the comparison does about length.
    assert!(!constant_time_equal(b"secret", b"sec"));
    assert!(!constant_time_equal(b"sec", b"secret"));
    assert!(!constant_time_equal(b"secret", b""));
    assert!(!constant_time_equal(b"", b"secret"));
    assert!(constant_time_equal(b"", b""));
    assert!(valid_asset_path("assets/index.js"));
    assert!(!valid_asset_path("../index.html"));

    let asset_name = reader_assets()
        .get_dir("assets")
        .and_then(|assets| assets.files().next())
        .and_then(|asset| asset.path().file_name())
        .and_then(|name| name.to_str())
        .expect("reader build must contain a generated asset");
    let mut asset = static_reader_response(&format!("assets/{asset_name}"));
    assert_eq!(asset.status(), StatusCode::OK);
    session_headers(asset.headers_mut(), None);
    assert_eq!(
        asset.headers()[CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        asset.headers()["cross-origin-resource-policy"],
        "same-origin"
    );
    assert_eq!(
        asset.headers()["permissions-policy"],
        "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()"
    );
    let document = static_reader_response("index.html");
    assert_eq!(document.headers()[CACHE_CONTROL], "no-store");
    assert!(reader_assets().get_file("THIRD_PARTY_NOTICES.md").is_some());
    // The two assets a publication frame loads, which the measurement basis is derived from.
    let fonts = crate::reader::reader_font_bundle_id().expect("a versioned reader font bundle");
    assert!(
        reader_assets()
            .get_file(format!("public/reader-fonts/{fonts}/reader-serif.css"))
            .is_some()
    );
    assert_eq!(content_agent_sha256().len(), 64);
}

#[test]
fn allocates_a_reader_authority() {
    let app_data = std::env::temp_dir().join(format!(
        "elephant-ladder-host-test-{}",
        crate::generate_session_token().unwrap()
    ));
    let host = PreparedHost::bind(&app_data, crate::HostExtensions::default()).unwrap();
    let policy = host.content_navigation_policy();
    assert!(
        host.launch_url()
            .starts_with("http://reader.elephant-ladder.localhost:")
    );
    assert!(host.launch_url().contains("/window/"));
    assert!(!policy.allows_origin("http://p-untrusted.localhost:41000"));
    let _ = std::fs::remove_dir_all(app_data);
}

#[test]
fn installation_origin_ports_are_stable_and_conflicts_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let first = PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
    let port = first.reader_state.imported_publication_port;
    let reader_port = first.reader.local_addr().unwrap().port();
    let connection =
        Connection::open(directory.path().join(crate::storage::LIBRARY_DATABASE)).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT foreground_port, reader_port
                     FROM installation_origin_config WHERE singleton_key = 1",
                [],
                |row| Ok((row.get::<_, u16>(0)?, row.get::<_, u16>(1)?)),
            )
            .unwrap(),
        (port, reader_port)
    );
    drop(connection);
    drop(first);

    let second = PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
    assert_eq!(second.reader_state.imported_publication_port, port);
    assert_eq!(second.reader.local_addr().unwrap().port(), reader_port);
    let running = second.start(
        HostInfo {
            application: "port-rebind-test".to_owned(),
            diagnostics_level: crate::DiagnosticsLevel::Summary,
        },
        Arc::new(crate::UnsupportedNativeActions),
    );
    let mut connection = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    connection
        .write_all(b"GET / HTTP/1.1\r\nHost: invalid.test\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = Vec::new();
    connection.read_to_end(&mut response).unwrap();
    assert!(response.starts_with(b"HTTP/1.1 "));
    running.stop();

    let rebound = PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
    assert_eq!(rebound.reader_state.imported_publication_port, port);
    assert_eq!(rebound.reader.local_addr().unwrap().port(), reader_port);
    drop(rebound);

    let occupied = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let error = PreparedHost::bind(directory.path(), crate::HostExtensions::default())
        .err()
        .unwrap();
    assert!(error.contains("could not bind persisted imported-publication port"));
    drop(occupied);
    let third = PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
    assert_eq!(third.reader_state.imported_publication_port, port);
    drop(third);

    let occupied = std::net::TcpListener::bind(("127.0.0.1", reader_port)).unwrap();
    let error = PreparedHost::bind(directory.path(), crate::HostExtensions::default())
        .err()
        .unwrap();
    assert!(error.contains("could not bind persisted reader port"));
    drop(occupied);
    let fourth = PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
    assert_eq!(fourth.reader.local_addr().unwrap().port(), reader_port);
}

#[tokio::test(flavor = "current_thread")]
async fn closes_listener_on_host_shutdown() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let running = RunningHost {
        servers: vec![spawn_server(
            listener,
            Router::new().route("/", get(|| async { "ok" })),
            "shutdown-test",
        )],
        runtime: None,
        storage: None,
        navigation_recovery: None,
    };
    let connection = tokio::net::TcpStream::connect(address).await.unwrap();
    drop(connection);

    running.stop();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}
