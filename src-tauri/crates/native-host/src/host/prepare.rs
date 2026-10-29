//! Everything a host needs before it serves: storage opened, origins allocated,
//! planes bound, and the reader's own credential minted.
//!
//! Binding happens before any listener accepts, so a request can never observe a
//! half-prepared host.

use std::{
    collections::{HashMap, HashSet},
    net::TcpListener,
    path::Path as FsPath,
    sync::{Arc, Mutex, RwLock, atomic::AtomicU64},
    time::{Duration, Instant},
};

use tokio::sync::Semaphore;

use crate::diagnostics::{DiagnosticReceipt, NativeDiagnostics};
use crate::http::auth::{generate_session_token, random_id};
use crate::mount::{
    ImportedPublicationState, MeasurementPublicationState, imported_publication_router,
    measurement_publication_router, nested_publication_router,
};
use crate::reader::{NavigationBootstrap, NavigationTicket};
use crate::reader_lifecycle::ReaderLifecycle;

use crate::host::extension::HostExtensions;
use crate::host::limits::{PREPARATION_WAIT, READER_HOST, RESOURCE_STREAM_CONCURRENCY};
use crate::host::state::{ReaderState, ReaderStateSeed};
use crate::host::{RunningHost, bind_installation_origin_listeners, spawn_server};
use crate::http::router::reader_router;
use crate::network_activity::NetworkActivityObserver;
use crate::reader::{ContentNavigationPolicy, NavigationRecovery, READER_ASSETS};
use crate::reader_lifecycle::ActiveReaderSessionView;
use crate::snapshot::SnapshotRegistry;
use crate::{HostInfo, NativeActions, bind_listener, dictionary, search_index, storage};

pub struct PreparedHost {
    pub(crate) reader: TcpListener,
    pub extensions: HostExtensions,
    pub(crate) imported_publication: TcpListener,
    pub(crate) nested_publication: TcpListener,
    pub(crate) measurement: TcpListener,
    pub(crate) reader_state: ReaderStateSeed,
    pub(crate) imported_publication_state: ImportedPublicationState,
    pub storage: storage::Storage,
    pub(crate) search_index: search_index::SearchIndexStore,
    pub(crate) dictionary: dictionary::DictionarySet,
    pub(crate) launch_url: String,
}

impl PreparedHost {
    /// Prepares a host. A product build contributes `HostExtensions::default()`,
    /// which is what makes every extension path unreachable.
    pub fn bind(app_data: impl AsRef<FsPath>, extensions: HostExtensions) -> Result<Self, String> {
        let app_data = app_data.as_ref();
        Self::bind_common(app_data, app_data, extensions)
    }

    /// Prepares a host whose bulk, re-downloadable data lives under its own root.
    ///
    /// On Windows the application data directory is the roaming profile; dictionaries
    /// are multi-gigabyte and machine-local, so the product passes the local data
    /// directory here. On Linux and Android both directories are the same path.
    pub fn bind_with_bulk_data(
        app_data: impl AsRef<FsPath>,
        bulk_data: impl AsRef<FsPath>,
        extensions: HostExtensions,
    ) -> Result<Self, String> {
        Self::bind_common(app_data.as_ref(), bulk_data.as_ref(), extensions)
    }

    fn bind_common(
        app_data: &FsPath,
        bulk_data: &FsPath,
        extensions: HostExtensions,
    ) -> Result<Self, String> {
        let mut storage = storage::Storage::open_with_bulk_data(app_data, bulk_data)?;
        let search_index = search_index::SearchIndexStore::open(app_data)?;
        search_index.retain(&storage.live_snapshot_ids()?)?;
        let dictionary = open_dictionaries(&mut storage);
        let (reader, imported_publication_listener) =
            bind_installation_origin_listeners(&mut storage)?;
        let nested_publication_listener = bind_listener("127.0.0.1:0")?;
        let measurement = bind_listener("127.0.0.1:0")?;
        let reader_port = reader
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let measurement_port = measurement
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let imported_publication_port = imported_publication_listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let nested_publication_port = nested_publication_listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let reader_authority = format!("{READER_HOST}:{reader_port}");
        let reader_origin = format!("http://{reader_authority}");
        let reader_session_id = generate_session_token().map_err(|error| error.to_string())?;
        let launch_id = random_id(16)?;
        let token = generate_session_token().map_err(|error| error.to_string())?;
        let launch_nonce = generate_session_token().map_err(|error| error.to_string())?;
        let library_document_id = generate_session_token().map_err(|error| error.to_string())?;
        let imported_publication_state = ImportedPublicationState {
            asset_origin: Arc::from(reader_origin.clone()),
            active_reader_session: ActiveReaderSessionView::default(),
            snapshots: Arc::new(Mutex::new(SnapshotRegistry::default())),
            stream_admission: Arc::new(Semaphore::new(RESOURCE_STREAM_CONCURRENCY)),
            transform_admission: Arc::new(Semaphore::new(1)),
        };
        Ok(Self {
            reader,
            extensions,
            imported_publication: imported_publication_listener,
            nested_publication: nested_publication_listener,
            measurement,
            reader_state: ReaderStateSeed {
                reader_authority,
                reader_origin: reader_origin.clone(),
                reader_session_id,
                launch_id,
                token,
                imported_publication_port,
                nested_publication_port,
                measurement_port,
                launch_nonce: launch_nonce.clone(),
                library_document_id,
            },
            imported_publication_state,
            storage,
            search_index,
            dictionary,
            launch_url: format!("{reader_origin}/window/{launch_nonce}/start"),
        })
    }

    pub fn launch_url(&self) -> &str {
        &self.launch_url
    }

    pub fn content_navigation_policy(&self) -> ContentNavigationPolicy {
        ContentNavigationPolicy {
            active_reader_session: self
                .imported_publication_state
                .active_reader_session
                .clone(),
        }
    }

    pub fn network_activity_observer(&self) -> NetworkActivityObserver {
        NetworkActivityObserver::new(
            self.imported_publication_state
                .active_reader_session
                .clone(),
        )
    }

    pub fn start(self, host_info: HostInfo, native_actions: Arc<dyn NativeActions>) -> RunningHost {
        let storage = storage::StorageHandle::new(self.storage);
        let HostExtensions {
            reader_routes: extension_reader_routes,
            reader_assets: extension_reader_assets,
        } = self.extensions;
        if let Some(assets) = extension_reader_assets {
            let _ = READER_ASSETS.set(assets);
        }
        let contributed = extension_reader_routes.is_some();
        let lifecycle = Arc::new(ReaderLifecycle::new(
            self.imported_publication_state
                .active_reader_session
                .clone(),
            Some(NavigationTicket {
                nonce: self.reader_state.launch_nonce.clone(),
                bootstrap: NavigationBootstrap::Library {
                    document_id: self.reader_state.library_document_id.clone(),
                },
            }),
        ));
        let reader_state = ReaderState {
            host_info,
            native_actions,
            reader_authority: Arc::from(self.reader_state.reader_authority),
            reader_origin: Arc::from(self.reader_state.reader_origin),
            reader_session_id: Arc::from(self.reader_state.reader_session_id),
            launch_id: Arc::from(self.reader_state.launch_id),
            token: Arc::from(self.reader_state.token),
            imported_publication_port: self.reader_state.imported_publication_port,
            nested_publication_port: self.reader_state.nested_publication_port,
            measurement_port: self.reader_state.measurement_port,
            contributed,
            library_document_id: Arc::new(RwLock::new(None)),
            acquisition_slot: Arc::new(Semaphore::new(1)),
            storage: storage.clone(),
            search_index: Arc::new(self.search_index),
            search_indexing: Arc::new(Mutex::new(HashSet::new())),
            dictionary: Arc::new(dictionary::DictionaryHost::new(self.dictionary)),
            lifecycle: lifecycle.clone(),
            preparations: Arc::new(Mutex::new(HashMap::new())),
            dictionary_operations: Arc::new(dictionary::DictionaryOperations::default()),
            dictionary_admission: Arc::new(Semaphore::new(1)),
            preparation_admission: Arc::new(Semaphore::new(1)),
            preparation_wait: PREPARATION_WAIT,
            snapshots: self.imported_publication_state.snapshots.clone(),
            latest_open_operation: Arc::new(AtomicU64::new(0)),
            open_sequence_gate: Arc::new(Mutex::new(())),
            pending_annotation_link: Arc::new(Mutex::new(None)),
            pending_opened_publications: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            diagnostics: Arc::new(NativeDiagnostics {
                started: Instant::now(),
                process_sequence: AtomicU64::new(0),
                receipt: Mutex::new(DiagnosticReceipt::default()),
            }),
        };
        let navigation_recovery = NavigationRecovery {
            reader_origin: reader_state.reader_origin.clone(),
            lifecycle,
            imported_publication_port: reader_state.imported_publication_port,
            nested_publication_port: reader_state.nested_publication_port,
            measurement_port: reader_state.measurement_port,
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("elephant-native-host")
            .build()
            .expect("failed to create native host runtime");
        let runtime_context = runtime.enter();
        let maintenance_storage = storage.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let storage = maintenance_storage.clone();
                match tokio::task::spawn_blocking(move || storage.locked()?.maintain_deletions())
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => tracing::error!(
                        event = "storage_maintenance_failed",
                        error_kind = "storage"
                    ),
                    Err(_) => {
                        tracing::error!(event = "storage_maintenance_failed", error_kind = "worker")
                    }
                }
            }
        });
        // An install interrupted by an earlier run continues from its partial files. This
        // never blocks startup and never discards them when the device is offline.
        dictionary::resume_interrupted_installs(&reader_state);
        let servers = vec![
            spawn_server(
                self.reader,
                reader_router(reader_state.clone(), extension_reader_routes),
                "reader",
            ),
            spawn_server(
                self.imported_publication,
                imported_publication_router(self.imported_publication_state.clone()),
                "imported-publication",
            ),
            spawn_server(
                self.nested_publication,
                nested_publication_router(self.imported_publication_state),
                "nested-publication",
            ),
            spawn_server(
                self.measurement,
                measurement_publication_router(MeasurementPublicationState {
                    reader_origin: reader_state.reader_origin.clone(),
                    asset_origin: reader_state.reader_origin.clone(),
                    active_reader_session: reader_state.lifecycle.active_view(),
                    stream_admission: Arc::new(Semaphore::new(RESOURCE_STREAM_CONCURRENCY)),
                    transform_admission: Arc::new(Semaphore::new(1)),
                }),
                "measurement",
            ),
        ];
        drop(runtime_context);
        RunningHost {
            servers,
            runtime: Some(runtime),
            storage: Some(storage),
            navigation_recovery: Some(navigation_recovery),
        }
    }
}

/// Opens installed dictionaries, first installing development packs in debug builds.
pub(crate) fn open_dictionaries(storage: &mut storage::Storage) -> dictionary::DictionarySet {
    #[cfg(debug_assertions)]
    if let Some(directory) = std::env::var_os("ELEPHANT_DICTIONARY_DEV_PACKS") {
        let targets = std::env::var("ELEPHANT_DICTIONARY_DEV_TARGETS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Err(error) =
            dictionary::install_development_packs(storage, FsPath::new(&directory), &targets)
        {
            tracing::warn!(event = "dictionary.development_packs_failed", error = %error);
        }
    }
    let registry = storage
        .dictionary_installations()
        .and_then(|installations| Ok((installations, storage.dictionary_views()?)));
    let set = match registry {
        Ok((installations, views)) => dictionary::DictionarySet::open(&installations, &views),
        Err(error) => {
            tracing::warn!(event = "dictionary.registry_unavailable", error = %error);
            dictionary::DictionarySet::empty()
        }
    };
    for reason in set.unavailable() {
        tracing::warn!(event = "dictionary.pack_unavailable", reason = %reason);
    }
    set
}
