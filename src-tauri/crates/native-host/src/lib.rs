mod annotation_exchange;
mod annotation_links;
mod annotation_transfer;
mod app_settings;
mod device;
mod diagnostics;
mod dictionary;
mod host;
mod http;
// What a contribution needs from the host. Nothing outside this crate consumes
// it except a non-product build's own crate.
pub use device::{
    DiagnosticsLevel, DisplayCapabilities, DisplayTechnology, HostInfo, NarrationCommand,
    NarrationPlayback, NarrationPlaybackState, NativeActionError, NativeActions,
    NativeActionsAvailability, OpenedPublication, OpenedPublicationIntent,
    UnsupportedNativeActions, WebsiteDataPurgeCategoryStatus, WebsiteDataPurgeReport,
};
pub use host::extension::HostExtensions;
pub use host::limits::IMAGE_DOCUMENT_QUERY;
pub use host::limits::LIBRARY_DOCUMENT_HEADER;
pub use host::limits::MAX_CONFORMANCE_REQUEST_BYTES;
pub use host::limits::OPEN_OPERATION_HEADER;
pub use host::limits::PAGINATION_HANDLE_HEADER;
pub use host::limits::RESOURCE_STREAM_CONCURRENCY;
pub use host::limits::TRACE_HEADER;
pub use host::limits::TRANSFORM_CACHE_BYTES;
pub(crate) use host::limits::{
    ACQUISITION_TIMEOUT, HTTP_REQUEST_SEQUENCE, MAX_ACQUISITION_BYTES, MAX_ANALYZED_RESOURCES,
    MAX_ANNOTATION_MUTATION_REQUEST_BYTES, MAX_BUFFERED_RESOURCE_BYTES,
    MAX_DIAGNOSTIC_BATCH_RECORDS, MAX_DIAGNOSTIC_COUNT, MAX_DIAGNOSTIC_DURATION_MS,
    MAX_DIAGNOSTIC_REQUEST_BYTES, MAX_EXTERNAL_MAIL_ADDRESS_BYTES, MAX_EXTERNAL_URL_BYTES,
    MAX_EXTERNAL_URL_REQUEST_BYTES, MAX_FACTS_SNAPSHOT_BYTES, MAX_PAGINATION_BULK_ENCODED_BYTES,
    MAX_PAGINATION_BULK_MUTATION_REQUEST_BYTES, MAX_PAGINATION_BULK_RAW_BYTES,
    MAX_PAGINATION_BULK_RECORDS, MAX_PAGINATION_BULK_REQUEST_BYTES, MAX_PAGINATION_RECORD_BYTES,
    MAX_PREFERENCES_REQUEST_BYTES, MAX_PUBLICATION_POLICY_REQUEST_BYTES,
    MAX_READING_POSITION_REQUEST_BYTES, MAX_READING_POSITION_TARGET_BYTES,
    MAX_RESOURCE_ANALYSIS_BYTES, MAX_RESPONSE_MANIFEST_BYTES, MAX_RESPONSE_MIME_BYTES,
    MAX_RESPONSE_RESOURCES, MAX_SEARCH_CONTINUATION_BYTES, MAX_SEARCH_DOCUMENTS,
    MAX_SEARCH_ENTRIES, MAX_SEARCH_ENTRIES_BYTES, MAX_SEARCH_ENTRY_TEXT_BYTES,
    MAX_SEARCH_EXCERPT_BYTES, MAX_SEARCH_EXCERPT_CODE_POINTS, MAX_SEARCH_ID_BYTES,
    MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_QUERY_CODE_POINTS, MAX_SEARCH_REQUEST_BYTES,
    MAX_SEARCH_RESULT_ROWS, MAX_SEARCH_RESULTS_PER_BATCH, MAX_SEARCH_SNAPSHOT_BYTES,
    MAX_SEARCH_STRING_BYTES, MAX_SMIL_NESTING, MAX_SMIL_NODES, MAX_TOTAL_ANALYSIS_BYTES,
    MAX_TRANSFORM_SOURCE_BYTES, MAX_TRANSFORMED_DOCUMENT_BYTES, NATIVE_MEASUREMENT_REVISION,
    PRODUCTION_READING_REVISION, READER_HOST, RESOURCE_CACHE_BYTES, RESOURCE_CACHE_ENTRIES,
    RESOURCE_STREAM_CHANNEL_CHUNKS, RESOURCE_STREAM_CHUNK_BYTES, RESPONSE_CONTENT_TYPES,
    SEARCH_REVISION, TRANSFORM_CACHE_ENTRIES, TRUSTED_READER_REQUEST_HEADER,
    TRUSTED_READER_REQUEST_VALUE,
};
pub(crate) use host::limits::{
    MAX_ADAPTED_IMAGE_BYTES, MAX_STYLESHEET_BYTES, adapted_image_decode_budget,
};
pub use host::{
    PreparedHost, ReaderState, RunningHost, RunningServer, bind_listener, spawn_server,
};
pub(crate) use host::{SnapshotRegistrySlot, open_dictionaries};
pub use http::auth;
pub use http::router::reader_router;
pub(crate) use http::router::request_library_document;
pub use http::{HttpObservation, HttpServerRole, not_found, observe_http};
pub use instrumentation::{
    InstrumentationError, SvgInstrumentationOptions, XhtmlInstrumentationOptions, instrument_svg,
    instrument_xhtml,
};
pub use mount::csp::container_constrained_csp;
pub use mount::{
    PublicationResource, ResourceRefused, admit_publication_resource, publication_request_refused,
};
pub use mount::{
    PublicationResponseProfile, ResourceStreamContext, blocked_publication_worker,
    decode_publication_path, image_document_source, instrument_svg_response,
    instrument_xhtml_response, nested_publication_base_url, publication_document_transformation,
    publication_representation, single_range_header, stream_imported_resource,
};
pub use opened_publications::offer_opened_publication;
pub use policy::{
    ResolvedActiveContentPolicy, ResolvedPublicationPolicy, ResolvedRemoteResourcesPolicy,
};
pub use reader::{ContentNavigationPolicy, NavigationRecovery};
pub use reader::{
    content_agent_path, content_agent_url, opaque_public_javascript, public_javascript, reader_csp,
    reader_font_base_url, serialize_bootstrap,
};
pub use reader_lifecycle::ActiveReaderSessionView;
pub use reading::ReadingPosition;
pub use search::search_analysis_limits;
pub use snapshot::SnapshotRegistry;
pub use snapshot::{
    Foreground, ForegroundPreparationState, Measurement, ResponseManifest, ResponseRule,
    response_manifest, validate_reading_products,
};
pub use storage::fsync_directory;
pub use storage::{
    ActiveContentDefault, EffectiveActiveContentPolicy, EffectiveRemoteResourcesPolicy,
    PolicyRelevance, PublicationPolicyOverrides, RemoteResourcesDefault,
};
pub use storage::{PreferenceRevisions, ReaderPreferences, StoredPreferences};
mod instrumentation;
mod journal;
mod library;
mod media_overlay;
pub mod mount;
mod network_activity;
mod notes;
mod opened_publications;
mod pagination;
pub mod policy;
mod publication_details;
mod reader;
pub mod reader_lifecycle;
mod reading;
mod reading_activity;
mod search;
mod search_index;
mod session;
pub mod snapshot;
mod staging;
pub mod storage;
mod storage_usage;

pub use annotation_links::{ANNOTATION_LINK_SCHEME, MAX_ANNOTATION_LINK_BYTES};
use std::{
    collections::BTreeMap,
    io,
    path::Path as FsPath,
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::Semaphore;

use diagnostics::{DiagnosticReceipt, NativeDiagnostics};
use http::auth::{
    authorize, constant_time_equal, generate_session_token, has_exact_header, random_id,
};
use http::{Trusted, TrustedJson, TrustedRead, TrustedWrite};
use library::{
    StagedCover, apply_reader_cover_cors, apply_reader_cover_cors_headers,
    apply_reader_page_image_cors, publication_title, selected_cover_path, stage_selected_cover,
};
use mount::content_type;
pub use network_activity::{NetworkActivityObserver, SessionNetworkActivitySink};
use policy::{publication_policy_relevance, resolve_publication_policy};
use reader::NavigationTicket;
use reader_lifecycle::ReaderLifecycle;
use reading::{ReadingPositionOccurrenceHint, reading_order_local_resources};
use session::{
    ActiveReaderSession, duration_milliseconds, log_performance, valid_reader_session_id,
};
use snapshot::{
    SnapshotEntry, TransformCacheKey, TransformedOutcome, derive_publication, load_snapshot_entry,
};

#[cfg(test)]
mod tests;

/// The origin a contributor's tests bind to.
#[doc(hidden)]
pub const TEST_READER_ORIGIN: &str = "http://reader.test";

/// A reader state for a contributor's tests, assembled the way `bind` assembles
/// one. It exists so the extension seam can be tested from the crate that owns
/// the contribution, without publishing this state's internals.
fn test_reader_state_with_actions(
    staging: &FsPath,
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

pub fn test_reader_state(staging: &FsPath) -> ReaderState {
    let state = test_reader_state_with_actions(staging, Arc::new(crate::UnsupportedNativeActions));
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
