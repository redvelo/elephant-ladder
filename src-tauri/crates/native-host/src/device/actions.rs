//! The native-operation boundary the platform adapter implements.
//!
//! Tauri is a thin adapter: it supplies application paths, creates trusted
//! windows, and performs platform effects through this trait. Tauri types never
//! appear in host state, storage, HTTP handlers, or publication policy, which is
//! what keeps this crate free of a `tauri` dependency.

use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct HostInfo {
    pub application: String,
    pub diagnostics_level: DiagnosticsLevel,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticsLevel {
    Summary,
    Trace,
}

#[derive(Debug)]
pub enum NativeActionError {
    Unsupported,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebsiteDataPurgeCategoryStatus {
    Attempted,
    Unsupported,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebsiteDataPurgeReport {
    pub android_web_storage: WebsiteDataPurgeCategoryStatus,
    pub cookies: WebsiteDataPurgeCategoryStatus,
    pub cache_storage: WebsiteDataPurgeCategoryStatus,
    pub worker_storage: WebsiteDataPurgeCategoryStatus,
    pub http_authentication: WebsiteDataPurgeCategoryStatus,
    pub form_data: WebsiteDataPurgeCategoryStatus,
    pub opfs: WebsiteDataPurgeCategoryStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NarrationPlaybackState {
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug)]
pub struct NarrationPlayback<'a> {
    pub state: NarrationPlaybackState,
    pub title: &'a str,
    pub section: Option<&'a str>,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub rate: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NarrationCommand {
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayTechnology {
    Eink,
    #[default]
    Unknown,
}

/// What the device itself decided about its display. On BOOX hardware the refresh mode and the
/// per-application drag behaviour are read from the firmware's profile, never written: the
/// profile pins both, and the application can only report them.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct DisplayCapabilities {
    pub technology: DisplayTechnology,
    /// The refresh mode as the device names it, such as `regal_plus`; absent off e-ink.
    #[serde(
        default,
        rename = "refreshMode",
        skip_serializing_if = "Option::is_none"
    )]
    pub refresh_mode: Option<String>,
    /// Whether the device switches to its fast waveform for the length of a drag, which costs a
    /// swipe page turn a flashing refresh a tap does not pay; absent where it cannot be read.
    #[serde(
        default,
        rename = "speedModeWhileDragging",
        skip_serializing_if = "Option::is_none"
    )]
    pub speed_mode_while_dragging: Option<bool>,
}

pub trait NativeActions: Send + Sync {
    fn display_capabilities(&self) -> DisplayCapabilities {
        DisplayCapabilities::default()
    }
    fn front_light_dialog_available(&self) -> bool {
        false
    }

    /// Whether this platform has buttons of its own that can turn pages.
    fn side_buttons_available(&self) -> bool {
        false
    }

    fn window_fullscreen_available(&self) -> bool {
        false
    }

    fn set_window_fullscreen(&self, _fullscreen: bool) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    fn window_fullscreen(&self) -> Result<bool, NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Asks the platform to present its own front-light dialog.
    ///
    /// The device owns the control surface and the resulting values, so this
    /// reports dispatch only. There is no readback.
    fn show_front_light_dialog(&self) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    fn open_external_url(&self, _url: &str) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    fn purge_origin_website_data(&self, _origin: &str) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Asks the user where to save a file; `None` means the user cancelled.
    fn save_file(
        &self,
        _file_name: &str,
        _media_type: &str,
        _contents: &[u8],
    ) -> Result<Option<String>, NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Keeps native media playback active for narration; `None` releases it.
    fn set_narration_playback(
        &self,
        _playback: Option<NarrationPlayback<'_>>,
    ) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Drains play and pause commands from native media controls.
    fn take_narration_commands(&self) -> Result<Vec<NarrationCommand>, NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Drains `elephant-ladder://` links the platform delivered to the application.
    fn take_annotation_links(&self) -> Result<Vec<String>, NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Tells the platform whether this application wants the device's own page buttons, which
    /// it can only take from the keys those buttons send.
    fn set_side_buttons_turn_pages(&self, _enabled: bool) -> Result<(), NativeActionError> {
        Err(NativeActionError::Unsupported)
    }

    /// Drains publications the platform asked this application to open, as readable local
    /// files. The platform owns where they came from; the host only reads the bytes.
    fn take_opened_publications(&self) -> Result<Vec<OpenedPublication>, NativeActionError> {
        Err(NativeActionError::Unsupported)
    }
}

/// A publication handed to the application from outside it, waiting to be imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedPublication {
    /// What the platform called the file, used only to name the import.
    pub file_name: String,
    /// A local path this process may read. A platform copy is removed once it is read.
    pub path: std::path::PathBuf,
    /// Whether reading the file should also delete it, which a staged platform copy needs.
    pub owned: bool,
    /// What the person asked for by handing it over.
    pub intent: OpenedPublicationIntent,
}

/// Asking to open a book and adding books to a library are different requests: one names a
/// book to read now, the other hands over as many as the person chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenedPublicationIntent {
    /// Import it and open it, which is what "open with" asks for.
    Read,
    /// Import it into the library and leave the reader where it is.
    Keep,
}

#[derive(Default)]
pub struct UnsupportedNativeActions;

impl NativeActions for UnsupportedNativeActions {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeActionsAvailability {
    pub front_light_dialog: bool,
    pub side_buttons: bool,
    pub window_fullscreen: bool,
}

impl WebsiteDataPurgeReport {
    pub(crate) fn for_native_result(result: &Result<(), NativeActionError>) -> Self {
        let android_web_storage = match result {
            Ok(()) => WebsiteDataPurgeCategoryStatus::Attempted,
            Err(NativeActionError::Unsupported) => WebsiteDataPurgeCategoryStatus::Unsupported,
            Err(NativeActionError::Failed(_)) => WebsiteDataPurgeCategoryStatus::Failed,
        };
        Self {
            android_web_storage,
            cookies: WebsiteDataPurgeCategoryStatus::Unsupported,
            cache_storage: WebsiteDataPurgeCategoryStatus::Unsupported,
            worker_storage: WebsiteDataPurgeCategoryStatus::Unsupported,
            http_authentication: WebsiteDataPurgeCategoryStatus::Unsupported,
            form_data: WebsiteDataPurgeCategoryStatus::Unsupported,
            opfs: WebsiteDataPurgeCategoryStatus::Unsupported,
        }
    }

    pub(crate) fn physical_status(&self) -> &'static str {
        if self.android_web_storage == WebsiteDataPurgeCategoryStatus::Unsupported {
            "unsupported"
        } else {
            "partial"
        }
    }
}
