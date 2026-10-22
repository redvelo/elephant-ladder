//! Shared fixtures, request builders, and router harnesses for the host tests.
#![allow(dead_code)]

pub(crate) use crate::TEST_READER_ORIGIN;

// The probe corpus, embedded here so the host's own tests need nothing from the
// runner's crate.
pub(crate) static ACTIVE_PROBE_BYTES: &[u8] = include_bytes!(
    "../../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-active-probe.epub"
);
pub(crate) static REMOTE_POLICY_BYTES: &[u8] = include_bytes!(
    "../../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-remote-policy.epub"
);
pub(crate) use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::Path,
    sync::{Arc, Mutex, RwLock, atomic::AtomicU64, atomic::Ordering},
    time::Duration,
};

pub(crate) use axum::{
    Router,
    body::{Body, Bytes},
    http::{
        HeaderMap, HeaderValue, Request, StatusCode, Uri,
        header::{
            ACCEPT_RANGES, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE,
            CONTENT_SECURITY_POLICY, CONTENT_TYPE, ORIGIN, RANGE,
        },
    },
    routing::get,
};
pub(crate) use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
pub(crate) use epub_stack::{Epub, EpubZip, MemoryResourceProvider};
pub(crate) use http_body_util::BodyExt;
pub(crate) use rusqlite::Connection;
pub(crate) use serde_json::Value;
pub(crate) use sha2::Digest as _;
pub(crate) use tokio::sync::{Notify, Semaphore};
pub(crate) use tower::ServiceExt;
pub(crate) use tracing::instrument::WithSubscriber as _;
pub(crate) use tracing_subscriber::fmt::MakeWriter;
pub(crate) use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

pub(crate) use crate::http::auth::{authorize, authorize_safe_read, constant_time_equal};
pub(crate) use crate::mount::{
    ImportedPublicationState, RangeDecision, decode_publication_path, imported_publication_router,
    measurement_publication_router, parse_range, single_range_header,
};
pub(crate) use crate::reader::{
    content_agent_path, content_agent_sha256, reader_font_bundle_id, static_reader_response,
    valid_asset_path,
};
pub(crate) use crate::search::search_text_fragment;
pub(crate) use crate::snapshot::{
    Foreground, ForegroundPreparationState, Measurement, ResponseManifest, ResponseRule,
};
pub(crate) use crate::storage::PaginationCacheMutationOutcome;
pub(crate) use crate::{
    HostInfo, MAX_DIAGNOSTIC_REQUEST_BYTES, MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES,
    MAX_PAGINATION_RECORD_BYTES, NativeActionError, NativeActions, PreparedHost, ReaderState,
    RunningHost, WebsiteDataPurgeCategoryStatus, WebsiteDataPurgeReport, reader::reader_assets,
    reader_router, session::live_pagination_descriptor, spawn_server,
};
pub(crate) use epub_stack::EpubPath;

pub(crate) const ALICE_BYTES: &[u8] =
    include_bytes!("../../../../../../navigator/tests/fixtures/alice-in-wonderland.epub");

#[derive(Clone, Default)]
pub(crate) struct TraceBuffer(pub(crate) Arc<Mutex<Vec<u8>>>);

pub(crate) struct TraceWriter(pub(crate) Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for TraceBuffer {
    type Writer = TraceWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TraceWriter(self.0.clone())
    }
}

impl Write for TraceWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TraceBuffer {
    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

pub(crate) fn trace_subscriber(buffer: TraceBuffer) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(buffer)
        .finish()
}

pub(crate) fn assert_json_keys(value: &serde_json::Value, expected: &[&str]) {
    let actual = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected.iter().copied().collect());
}

#[derive(Default)]
pub(crate) struct TestDisplayActions {
    pub(crate) front_light_dialogs: Mutex<u32>,
    pub(crate) external_url: Mutex<Option<String>>,
    pub(crate) window_fullscreen: Mutex<bool>,
}

impl NativeActions for TestDisplayActions {
    fn front_light_dialog_available(&self) -> bool {
        true
    }

    fn window_fullscreen_available(&self) -> bool {
        true
    }

    fn set_window_fullscreen(&self, fullscreen: bool) -> Result<(), NativeActionError> {
        *self.window_fullscreen.lock().unwrap() = fullscreen;
        Ok(())
    }

    fn window_fullscreen(&self) -> Result<bool, NativeActionError> {
        Ok(*self.window_fullscreen.lock().unwrap())
    }

    fn show_front_light_dialog(&self) -> Result<(), NativeActionError> {
        *self.front_light_dialogs.lock().unwrap() += 1;
        Ok(())
    }

    fn open_external_url(&self, url: &str) -> Result<(), NativeActionError> {
        *self.external_url.lock().unwrap() = Some(url.to_owned());
        Ok(())
    }
}
pub(crate) fn valid_snapshot(source: &str) -> Value {
    let style = serde_json::json!({
        "display": "block", "position": "static", "visibility": "visible",
        "opacity": "1", "width": "800px", "height": "600px",
        "minWidth": "0px", "minHeight": "0px", "overflow": "visible",
        "overflowX": "visible", "overflowY": "visible", "transform": "none",
        "writingMode": "horizontal-tb", "colorScheme": "normal",
        "marginTop": "0px", "marginRight": "0px", "marginBottom": "0px",
        "marginLeft": "0px", "paddingTop": "0px", "paddingRight": "0px",
        "paddingBottom": "0px", "paddingLeft": "0px"
    });
    let rect = serde_json::json!({
        "x": 0, "y": 0, "top": 0, "right": 800, "bottom": 600,
        "left": 0, "width": 800, "height": 600
    });
    serde_json::json!({
        "readyState": "complete",
        "locationHref": format!("http://publication.test/{source}"),
        "requestOrigin": TEST_READER_ORIGIN,
        "viewport": { "width": 800, "height": 600, "devicePixelRatio": 1 },
        "documentElement": {
            "rect": rect,
            "computedStyle": style,
            "outerHTML": { "status": "available", "utf8Bytes": 13, "value": "<html></html>" }
        },
        "body": { "rect": rect, "computedStyle": style }
    })
}
pub(crate) fn conformance_request(body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/conformance")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

pub(crate) fn buffered_read_archive(large: &[u8]) -> Vec<u8> {
    let chapter = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><p>Text</p></body></html>"#;
    let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head><body><nav xmlns:epub="http://www.idpf.org/2007/ops" epub:type="toc"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#;
    synthetic_epub(
            br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">buffered-reads</dc:identifier><dc:title>Buffered reads</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2026-09-13T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="css" href="styles" media-type="text/css"/><item id="prefixed" href="prefixed.css" media-type="text/css"/><item id="large" href="large.css" media-type="text/css"/><item id="avif" href="probe.avif" media-type="image/avif"/><item id="jxl" href="probe.jxl" media-type="image/jxl"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
            &[
                ("EPUB/chapter.xhtml", chapter),
                ("EPUB/nav.xhtml", nav),
                ("EPUB/styles", BUFFERED_READ_CSS),
                ("EPUB/prefixed.css", PREFIXED_READ_CSS),
                ("EPUB/large.css", large),
                ("EPUB/probe.avif", PROBE_AVIF),
                ("EPUB/probe.jxl", PROBE_JXL),
            ],
        )
}

pub(crate) type RecordedNarration = (
    crate::NarrationPlaybackState,
    String,
    Option<String>,
    Option<u64>,
    Option<u64>,
);

#[derive(Default)]
pub(crate) struct TestNarrationActions {
    pub(crate) playback: Mutex<Vec<Option<RecordedNarration>>>,
    pub(crate) commands: Mutex<Vec<crate::NarrationCommand>>,
}

impl NativeActions for TestNarrationActions {
    fn set_narration_playback(
        &self,
        playback: Option<crate::NarrationPlayback<'_>>,
    ) -> Result<(), crate::NativeActionError> {
        self.playback.lock().unwrap().push(playback.map(|playback| {
            (
                playback.state,
                playback.title.to_owned(),
                playback.section.map(str::to_owned),
                playback.position_ms,
                playback.duration_ms,
            )
        }));
        Ok(())
    }

    fn take_narration_commands(
        &self,
    ) -> Result<Vec<crate::NarrationCommand>, crate::NativeActionError> {
        Ok(std::mem::take(&mut *self.commands.lock().unwrap()))
    }
}

pub(crate) fn fixture_reader_router(staging: &Path) -> Router {
    reader_router(fixture_reader_state(staging), None)
}

pub(crate) async fn wait_for_search_indexing(
    indexing: &Arc<Mutex<std::collections::HashSet<String>>>,
) {
    for _ in 0..800 {
        if indexing
            .lock()
            .expect("search indexing lock poisoned")
            .is_empty()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("search indexing did not finish");
}
pub(crate) fn fixture_reader_state(staging: &Path) -> ReaderState {
    let state =
        fixture_reader_state_with_actions(staging, Arc::new(crate::UnsupportedNativeActions));
    state
        .storage
        .lock()
        .unwrap()
        .compare_and_swap_global_publication_policy(
            0,
            crate::storage::GlobalPublicationPolicy {
                active_content: crate::storage::ActiveContentDefault::Disabled,
                remote_resources: crate::storage::RemoteResourcesDefault::Blocked,
            },
        )
        .unwrap();
    state
}

pub(crate) fn fixture_reader_state_with_actions(
    staging: &Path,
    native_actions: Arc<dyn NativeActions>,
) -> ReaderState {
    ReaderState {
        host_info: HostInfo {
            application: "test".to_owned(),
            diagnostics_level: crate::DiagnosticsLevel::Trace,
        },
        native_actions,
        reader_authority: Arc::from("reader.test"),
        reader_origin: Arc::from(TEST_READER_ORIGIN),
        reader_session_id: Arc::from("reader-session"),
        launch_id: Arc::from("0123456789abcdef0123456789abcdef"),
        token: Arc::from("secret"),
        imported_publication_port: 80,
        nested_publication_port: 82,
        measurement_port: 81,
        contributed: false,
        lifecycle: Arc::new(crate::ReaderLifecycle::new(
            crate::ActiveReaderSessionView::default(),
            Some(crate::reader::NavigationTicket {
                nonce: "launch-nonce".to_owned(),
                bootstrap: crate::reader::NavigationBootstrap::Library {
                    document_id: "b".repeat(64),
                },
            }),
        )),
        library_document_id: Arc::new(RwLock::new(Some(Arc::from("b".repeat(64))))),
        acquisition_slot: Arc::new(Semaphore::new(1)),
        storage: crate::storage::StorageHandle::new(
            crate::storage::Storage::open(staging).unwrap(),
        ),
        search_index: Arc::new(crate::search_index::SearchIndexStore::open(staging).unwrap()),
        search_indexing: Arc::new(Mutex::new(std::collections::HashSet::new())),
        dictionary: Arc::new(crate::dictionary::DictionaryHost::new(
            crate::dictionary::DictionarySet::empty(),
        )),
        dictionary_operations: Arc::new(crate::dictionary::DictionaryOperations::default()),
        dictionary_admission: Arc::new(Semaphore::new(1)),
        preparations: Arc::new(Mutex::new(std::collections::HashMap::new())),
        preparation_admission: Arc::new(Semaphore::new(1)),
        preparation_wait: Duration::from_secs(5),
        snapshots: Arc::new(Mutex::new(crate::SnapshotRegistry::default())),
        latest_open_operation: Arc::new(AtomicU64::new(0)),
        open_sequence_gate: Arc::new(Mutex::new(())),
        pending_annotation_link: Arc::new(Mutex::new(None)),
        pending_opened_publications: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        diagnostics: Arc::new(crate::NativeDiagnostics {
            started: crate::Instant::now(),
            process_sequence: AtomicU64::new(0),
            receipt: Mutex::new(crate::DiagnosticReceipt::default()),
        }),
    }
}

pub(crate) fn diagnostic_request(
    origin: &str,
    authorization: &str,
    body: serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/diagnostics/v1")
        .header("host", "reader.test")
        .header(ORIGIN, origin)
        .header(AUTHORIZATION, authorization)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub(crate) fn annotation_import_request(
    publication_id: &str,
    query: &str,
    body: String,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/native/publications/{publication_id}/annotations/import{query}"
        ))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
        .body(Body::from(body))
        .unwrap()
}

pub(crate) async fn response_json(
    response: axum::response::Response,
) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

pub(crate) struct LinkActions(pub(crate) Mutex<Vec<String>>);

impl NativeActions for LinkActions {
    fn take_annotation_links(&self) -> Result<Vec<String>, crate::NativeActionError> {
        Ok(std::mem::take(&mut *self.0.lock().unwrap()))
    }
}

pub(crate) struct OpenedPublicationActions(pub(crate) Mutex<Vec<crate::OpenedPublication>>);

impl NativeActions for OpenedPublicationActions {
    fn take_opened_publications(
        &self,
    ) -> Result<Vec<crate::OpenedPublication>, crate::NativeActionError> {
        Ok(std::mem::take(&mut *self.0.lock().unwrap()))
    }
}

pub(crate) fn opened_publication_request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", "reader.test")
        .header("x-elephant-request", "trusted-reader")
        .header(AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn import_request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/publications/import")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/epub+zip")
        .header(CONTENT_LENGTH, ALICE_BYTES.len())
        .body(Body::from(ALICE_BYTES))
        .unwrap()
}

pub(crate) fn import_request_with(bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/publications/import")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/epub+zip")
        .header(CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .unwrap()
}

pub(crate) fn cover_request(method: &str, publication_id: &str, authorized: bool) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/native/publications/{publication_id}/cover"))
        .header("host", "reader.test")
        .header("x-elephant-request", "trusted-reader");
    if authorized {
        request = request.header(AUTHORIZATION, "Bearer secret");
    }
    request.body(Body::empty()).unwrap()
}

pub(crate) fn pagination_cache_request(
    method: &str,
    basis: &str,
    handle: &str,
    body: Body,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/native/pagination-cache/{basis}"))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(crate::PAGINATION_HANDLE_HEADER, handle);
    if method == "POST" || method == "PUT" {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    request.body(body).unwrap()
}

/// Writes one record through the bulk mutation route, which is the only write
/// path the host exposes.
pub(crate) fn pagination_cache_write(
    basis: &str,
    record: &str,
    value: &[u8],
    handle: &str,
) -> Request<Body> {
    use base64::Engine as _;
    let value = base64::engine::general_purpose::STANDARD.encode(value);
    pagination_cache_request(
        "PUT",
        basis,
        handle,
        Body::from(
            serde_json::json!({
                "operations": [{ "type": "write", "record": record, "value": value }],
            })
            .to_string(),
        ),
    )
}

/// The one record a bulk read returned, or `None` when the cache does not hold it.
pub(crate) async fn pagination_cache_record(
    router: &Router,
    basis: &str,
    record: &str,
    handle: &str,
) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let response = router
        .clone()
        .oneshot(pagination_cache_read(basis, record, handle))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    body["records"][0].as_str().map(|value| {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .unwrap()
    })
}

/// Reads one record through the bulk read route.
pub(crate) fn pagination_cache_read(basis: &str, record: &str, handle: &str) -> Request<Body> {
    pagination_cache_request(
        "POST",
        basis,
        handle,
        Body::from(serde_json::json!({ "records": [record] }).to_string()),
    )
}

pub(crate) fn cover_epub(mime: &str, extension: &str, cover: &[u8]) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut output);
        zip.start_file(
            "mimetype",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        let options = SimpleFileOptions::default();
        zip.start_file("META-INF/container.xml", options).unwrap();
        zip.write_all(
                br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#,
            )
            .unwrap();
        zip.start_file("EPUB/package.opf", options).unwrap();
        zip.write_all(
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">cover-test</dc:identifier>
    <dc:title>Cover test</dc:title>
    <dc:language>en</dc:language>
    <meta property="dcterms:modified">2026-08-30T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover" href="cover.{extension}" media-type="{mime}" properties="cover-image"/>
  </manifest>
  <spine><itemref idref="chapter"/></spine>
</package>"#
            )
            .as_bytes(),
        )
        .unwrap();
        zip.start_file("EPUB/nav.xhtml", options).unwrap();
        zip.write_all(
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Contents</title></head><body><nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops"><ol><li><a href="chapter.xhtml">Chapter</a></li></ol></nav></body></html>"#,
            )
            .unwrap();
        zip.start_file("EPUB/chapter.xhtml", options).unwrap();
        zip.write_all(
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body><p>Chapter</p></body></html>"#,
            )
            .unwrap();
        zip.start_file(format!("EPUB/cover.{extension}"), options)
            .unwrap();
        zip.write_all(cover).unwrap();
        zip.finish().unwrap();
    }
    output.into_inner()
}

pub(crate) fn synthetic_epub(package: &[u8], resources: &[(&str, &[u8])]) -> Vec<u8> {
    synthetic_epub_with(package, resources, CompressionMethod::Deflated)
}

pub(crate) fn synthetic_epub_with(
    package: &[u8],
    resources: &[(&str, &[u8])],
    compression: CompressionMethod,
) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut output);
        zip.start_file(
            "mimetype",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        let options = SimpleFileOptions::default().compression_method(compression);
        zip.start_file("META-INF/container.xml", options).unwrap();
        zip.write_all(
                br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
        zip.start_file("EPUB/package.opf", options).unwrap();
        zip.write_all(package).unwrap();
        for (path, bytes) in resources {
            zip.start_file(*path, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
    output.into_inner()
}

pub(crate) fn publication_list_request() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/native/publications")
        .header("host", "reader.test")
        .header(AUTHORIZATION, "Bearer secret")
        .header("x-elephant-request", "trusted-reader")
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn open_request(publication_id: &str) -> Request<Body> {
    open_request_with_operation(publication_id, 1)
}

pub(crate) fn delete_request(publication_id: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(format!("/native/publications/{publication_id}"))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn website_data_reset_request(publication_id: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/native/publications/{publication_id}/website-data/reset"
        ))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn open_request_with_operation(publication_id: &str, operation: u64) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/native/publications/{publication_id}/open"))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(crate::OPEN_OPERATION_HEADER, operation)
        .header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn close_request(reader_session_id: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/reader/close")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "readerSessionId": reader_session_id }).to_string(),
        ))
        .unwrap()
}

pub(crate) fn reading_position_request(
    reader_session_id: &str,
    base_revision: u32,
    sequence: u32,
    position: &serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri("/native/reading-position")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "readerSessionId": reader_session_id,
                "baseRevision": base_revision,
                "sequence": sequence,
                "position": position,
            })
            .to_string(),
        ))
        .unwrap()
}

pub(crate) fn preference_request(
    reader_session_id: &str,
    global_revision: u32,
    publication_revision: u32,
    preferences: &serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri("/native/preferences")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "readerSessionId": reader_session_id,
                "baseRevisions": {
                    "global": global_revision,
                    "publication": publication_revision,
                },
                "preferences": preferences,
            })
            .to_string(),
        ))
        .unwrap()
}

pub(crate) fn publication_policy_request(
    method: &str,
    path: String,
    body: Option<serde_json::Value>,
    authorized: bool,
    library_scoped: bool,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "reader.test");
    if method == "GET" {
        request = request.header("x-elephant-request", "trusted-reader");
    } else {
        request = request
            .header(ORIGIN, TEST_READER_ORIGIN)
            .header(CONTENT_TYPE, "application/json");
    }
    if authorized {
        request = request.header(AUTHORIZATION, "Bearer secret");
    }
    if library_scoped {
        request = request.header(crate::LIBRARY_DOCUMENT_HEADER, "b".repeat(64));
    }
    request
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

pub(crate) fn annotation_mutation_request(
    reader_session_id: &str,
    mutation: &serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/annotations/mutations")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "readerSessionId": reader_session_id,
                "mutation": mutation,
            })
            .to_string(),
        ))
        .unwrap()
}

pub(crate) fn search_request(
    reader_session_id: &str,
    query: &str,
    continuation: Option<&str>,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/search")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "readerSessionId": reader_session_id,
                "request": {
                    "requestId": "search-request",
                    "query": query,
                    "scope": { "type": "publication" },
                    "initiator": "query-entry",
                    "continuation": continuation,
                    "limit": 20
                }
            })
            .to_string(),
        ))
        .unwrap()
}

pub(crate) fn occurrence_search_request(
    reader_session_id: &str,
    query: &str,
    occurrence: u32,
    resource: &str,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/search")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "readerSessionId": reader_session_id,
                "request": {
                    "requestId": "occurrence-search-request",
                    "query": query,
                    "scope": {
                        "type": "occurrence",
                        "occurrence": occurrence,
                        "resource": resource
                    },
                    "initiator": "query-entry",
                    "continuation": null,
                    "limit": 20
                }
            })
            .to_string(),
        ))
        .unwrap()
}

pub(crate) fn imported_content_agent_request(origin: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(content_agent_path())
        .header("host", "reader.test")
        .header(ORIGIN, origin)
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn reader_font_request(method: &str, path: &str, origin: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(format!(
            "/public/reader-fonts/{}/{path}",
            reader_font_bundle_id().unwrap()
        ))
        .header("host", "reader.test");
    if let Some(origin) = origin {
        request = request.header(ORIGIN, origin);
    }
    request.body(Body::empty()).unwrap()
}

pub(crate) async fn assert_audio_route(
    foreground: &Router,
    authority: &str,
    audio_path: &str,
    audio: &[u8],
    mime: &str,
) {
    let length = audio.len();
    let uri = format!("/{audio_path}");
    let tail_start = format!("bytes={}-", length - 12);
    let past_end = format!("bytes={length}-");
    for (method, ranges, status, body) in [
        ("GET", vec![], StatusCode::OK, Some(0..length)),
        (
            "GET",
            vec!["bytes=10-19"],
            StatusCode::PARTIAL_CONTENT,
            Some(10..20),
        ),
        (
            "GET",
            vec!["bytes=-16"],
            StatusCode::PARTIAL_CONTENT,
            Some(length - 16..length),
        ),
        (
            "GET",
            vec![tail_start.as_str()],
            StatusCode::PARTIAL_CONTENT,
            Some(length - 12..length),
        ),
        (
            "GET",
            vec![past_end.as_str()],
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
        ),
        (
            "GET",
            vec!["bytes=0-1", "bytes=2-3"],
            StatusCode::OK,
            Some(0..length),
        ),
        (
            "GET",
            vec!["bytes=0-0"],
            StatusCode::PARTIAL_CONTENT,
            Some(0..1),
        ),
        (
            "GET",
            vec!["bytes=-0"],
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
        ),
        (
            "GET",
            vec!["bytes=19-10"],
            StatusCode::RANGE_NOT_SATISFIABLE,
            None,
        ),
        // A HEAD response reports the same range a GET would; only the body is empty.
        (
            "HEAD",
            vec!["bytes=10-19"],
            StatusCode::PARTIAL_CONTENT,
            Some(10..20),
        ),
    ] {
        let mut request = imported_publication_request(method, &uri, authority);
        request
            .headers_mut()
            .insert(ORIGIN, HeaderValue::from_static(TEST_READER_ORIGIN));
        for range in &ranges {
            request
                .headers_mut()
                .append(RANGE, HeaderValue::from_str(range).unwrap());
        }
        let response = foreground.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{method} {ranges:?}");
        assert!(
            !response
                .headers()
                .keys()
                .any(|name| name.as_str().starts_with("access-control-")),
            "{method} {ranges:?}"
        );
        let Some(expected) = body else {
            assert_eq!(
                response.headers()[CONTENT_RANGE],
                format!("bytes */{length}")
            );
            continue;
        };
        assert_eq!(response.headers()[CONTENT_TYPE], mime);
        assert_eq!(response.headers()[ACCEPT_RANGES], "bytes");
        if status == StatusCode::PARTIAL_CONTENT {
            assert_eq!(
                response.headers()[CONTENT_RANGE],
                format!("bytes {}-{}/{length}", expected.start, expected.end - 1)
            );
        }
        // The reported length is what the matching GET body would be, for both
        // methods; only the body itself differs.
        assert_eq!(
            response.headers()[CONTENT_LENGTH],
            (expected.end - expected.start).to_string(),
            "{method} {ranges:?}"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let expected_body: &[u8] = if method == "HEAD" {
            &[]
        } else {
            &audio[expected.clone()]
        };
        assert!(bytes.as_ref() == expected_body, "{method} {ranges:?}");
    }

    for (dest, policy) in [
        (Some("audio"), "cross-origin"),
        (Some("video"), "cross-origin"),
        (None, "same-origin"),
        (Some("document"), "same-origin"),
    ] {
        let mut request = imported_publication_request("GET", &uri, authority);
        request
            .headers_mut()
            .insert(RANGE, HeaderValue::from_static("bytes=0-1"));
        if let Some(dest) = dest {
            request
                .headers_mut()
                .insert("sec-fetch-dest", HeaderValue::from_static(dest));
        }
        let response = foreground.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            policy,
            "{dest:?}"
        );
        assert!(
            !response
                .headers()
                .keys()
                .any(|name| name.as_str().starts_with("access-control-"))
        );
    }
}

pub(crate) async fn import_and_open(reader: &Router, archive: Vec<u8>, operation: u64) -> String {
    let id = import_publication(reader, archive).await;
    open_publication(reader, &id, operation).await;
    id
}

pub(crate) async fn import_publication(reader: &Router, archive: Vec<u8>) -> String {
    let created = reader
        .clone()
        .oneshot(import_request_with(archive))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    created["publication"]["publicationId"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(crate) async fn open_publication(reader: &Router, id: &str, operation: u64) {
    assert_eq!(
        reader
            .clone()
            .oneshot(open_request_with_operation(id, operation))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

pub(crate) fn media_overlay_request(path: String) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(path)
        .header("host", "reader.test")
        .header(AUTHORIZATION, "Bearer secret")
        .header("x-elephant-request", "trusted-reader")
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn imported_publication_request(
    method: &str,
    path: &str,
    authority: &str,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", authority)
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn measurement_request(method: &str, path: &str, authority: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", authority)
        .body(Body::empty())
        .unwrap()
}

pub(crate) fn reader_navigation_request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("host", "reader.test")
        .body(Body::empty())
        .unwrap()
}

pub(crate) async fn bootstrap_from_document(
    response: axum::response::Response,
) -> serde_json::Value {
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
    let start = body
        .find(r#"<script id="native-bootstrap" type="application/json">"#)
        .unwrap()
        + r#"<script id="native-bootstrap" type="application/json">"#.len();
    let end = body[start..].find("</script>").unwrap() + start;
    serde_json::from_str(&body[start..end]).unwrap()
}

pub(crate) fn pending_reading_bootstrap(
    lifecycle: &Arc<crate::ReaderLifecycle>,
) -> serde_json::Value {
    let control = lifecycle.try_control();
    let crate::reader::NavigationBootstrap::Reading(reading) =
        &control.navigation_ticket.as_ref().unwrap().bootstrap
    else {
        panic!("pending ticket is not a reading bootstrap");
    };
    serde_json::json!({
        "readerSessionId": reading.reader_session_id,
        "publicationId": reading.publication_id,
        "publicationOrigin": reading.publication_origin,
        "measurementOrigin": reading.measurement_origin,
        "publication": reading.publication,
        "initialReadingPosition": reading.initial_reading_position,
        "positionRevision": reading.position_revision,
        "annotations": reading.annotations.public(),
        "preferences": reading.preferences,
        "preferenceRevisions": reading.preference_revisions,
        "pagination": reading.pagination,
    })
}

pub(crate) async fn staging_is_empty(path: &Path) -> bool {
    let mut entries = tokio::fs::read_dir(path).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "part")
        {
            return false;
        }
    }
    true
}

pub(crate) async fn assert_staging_empty(path: &Path) {
    assert!(staging_is_empty(path).await);
}
pub(crate) fn publication_request(method: &str, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", "publication.test")
        .body(Body::empty())
        .unwrap()
}

pub(crate) const BUFFERED_READ_CSS: &[u8] = b"p{color:red}";
pub(crate) const PREFIXED_READ_CSS: &[u8] = b"p{-epub-hyphens:auto;color:red}";
// The engine-support probe already carries a minimal sample of every image format this
// host may be asked to restate, so the same committed bytes serve both.
pub(crate) const PROBE_AVIF: &[u8] = include_bytes!(
    "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.avif"
);
pub(crate) const PROBE_PNG: &[u8] = include_bytes!(
    "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.png"
);
pub(crate) const PROBE_JXL: &[u8] = include_bytes!(
    "../../../../../tests/fixtures/engine-support/elephant-engine-support-src/EPUB/media/probe.jxl"
);
