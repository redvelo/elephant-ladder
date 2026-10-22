//! What every trusted-reader handler is given.
//!
//! The state carries the credential, the ports of each plane, and handles to the
//! registries. Handlers read the active session through an immutable view, so
//! authorization never waits on a lifecycle writer.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, RwLock, atomic::AtomicU64},
    time::Duration,
};

use tokio::sync::Semaphore;

use crate::diagnostics::NativeDiagnostics;
use crate::reader_lifecycle::ReaderLifecycle;
use crate::session::{PreparationJob, PreparationParticipation};

use crate::annotation_links;
use crate::opened_publications;
use crate::snapshot::SnapshotRegistry;
use crate::{HostInfo, NativeActions, dictionary, search_index, storage};

pub(crate) struct ReaderStateSeed {
    pub(crate) reader_authority: String,
    pub(crate) reader_origin: String,
    pub(crate) reader_session_id: String,
    pub(crate) launch_id: String,
    pub(crate) token: String,
    pub(crate) imported_publication_port: u16,
    pub(crate) nested_publication_port: u16,
    pub(crate) measurement_port: u16,
    pub(crate) launch_nonce: String,
    pub(crate) library_document_id: String,
}

/// The trusted reader origin's state, which a contributed route also reads.
#[derive(Clone)]
pub struct ReaderState {
    pub host_info: HostInfo,
    pub native_actions: Arc<dyn NativeActions>,
    pub reader_authority: Arc<str>,
    pub reader_origin: Arc<str>,
    pub reader_session_id: Arc<str>,
    pub launch_id: Arc<str>,
    pub token: Arc<str>,
    pub imported_publication_port: u16,
    pub nested_publication_port: u16,
    pub measurement_port: u16,
    /// Whether a non-product build contributed reader routes. The host never
    /// learns what they are; it only reports more when one did.
    pub contributed: bool,
    pub library_document_id: Arc<RwLock<Option<Arc<str>>>>,
    pub acquisition_slot: Arc<Semaphore>,
    pub storage: storage::StorageHandle,
    pub(crate) search_index: Arc<search_index::SearchIndexStore>,
    pub search_indexing: Arc<Mutex<HashSet<String>>>,
    pub(crate) dictionary: Arc<dictionary::DictionaryHost>,
    pub(crate) dictionary_operations: Arc<dictionary::DictionaryOperations>,
    pub dictionary_admission: Arc<Semaphore>,
    pub lifecycle: Arc<ReaderLifecycle>,
    pub(crate) preparations: Arc<Mutex<HashMap<String, Arc<PreparationJob>>>>,
    pub preparation_admission: Arc<Semaphore>,
    pub preparation_wait: Duration,
    pub snapshots: SnapshotRegistrySlot,
    pub latest_open_operation: Arc<AtomicU64>,
    pub open_sequence_gate: Arc<Mutex<()>>,
    pub(crate) diagnostics: Arc<NativeDiagnostics>,
    pub(crate) pending_annotation_link: Arc<Mutex<Option<annotation_links::PendingAnnotationLink>>>,
    pub(crate) pending_opened_publications:
        Arc<Mutex<VecDeque<opened_publications::PendingOpenedPublication>>>,
}

pub(crate) type SnapshotRegistrySlot = Arc<Mutex<SnapshotRegistry>>;

impl PreparationParticipation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Joined => "joined",
        }
    }
}
