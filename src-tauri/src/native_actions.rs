use std::sync::Arc;

use elephant_ladder_native_host::NativeActions;
use tauri::{AppHandle, Runtime, plugin::TauriPlugin};

#[cfg(not(any(desktop, target_os = "android")))]
use elephant_ladder_native_host::UnsupportedNativeActions;
#[cfg(target_os = "android")]
use elephant_ladder_native_host::{
    NarrationCommand, NarrationPlayback, NarrationPlaybackState, NativeActionError,
    OpenedPublication, OpenedPublicationIntent,
};
#[cfg(any(desktop, target_os = "android"))]
use tauri::Manager;
#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct AvailabilityResponse {
    available: bool,
}

const PLUGIN_NAME: &str = "native-actions";

#[cfg(target_os = "android")]
const ANDROID_PLUGIN_IDENTIFIER: &str = "com.redvelo.elephant_ladder";

#[cfg(target_os = "android")]
struct AndroidNativeActions<R: Runtime> {
    display: elephant_ladder_native_host::DisplayCapabilities,
    handle: PluginHandle<R>,
    front_light_dialog_available: bool,
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct OpenUrlArgs<'a> {
    url: &'a str,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct SaveFileResponse {
    location: Option<String>,
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveFileArgs<'a> {
    file_name: &'a str,
    media_type: &'a str,
    contents: &'a str,
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct PurgeOriginArgs<'a> {
    origin: &'a str,
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NarrationPlaybackArgs<'a> {
    state: Option<NarrationPlaybackState>,
    title: &'a str,
    section: Option<&'a str>,
    position_ms: Option<u64>,
    duration_ms: Option<u64>,
    rate: f32,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct AnnotationLinksResponse {
    links: Vec<String>,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct NarrationCommandsResponse {
    commands: Vec<NarrationCommand>,
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct SideButtonsArgs {
    enabled: bool,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenedPublicationsResponse {
    publications: Vec<OpenedPublicationResponse>,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenedPublicationResponse {
    file_name: String,
    path: String,
}

#[cfg(target_os = "android")]
impl<R: Runtime> NativeActions for AndroidNativeActions<R> {
    fn display_capabilities(&self) -> elephant_ladder_native_host::DisplayCapabilities {
        self.display.clone()
    }
    fn front_light_dialog_available(&self) -> bool {
        self.front_light_dialog_available
    }

    fn side_buttons_available(&self) -> bool {
        true
    }

    fn show_front_light_dialog(&self) -> Result<(), NativeActionError> {
        if !self.front_light_dialog_available {
            return Err(NativeActionError::Unsupported);
        }
        self.handle
            .run_mobile_plugin::<()>("showFrontLightDialog", ())
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn open_external_url(&self, url: &str) -> Result<(), NativeActionError> {
        self.handle
            .run_mobile_plugin::<()>("openUrl", OpenUrlArgs { url })
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn purge_origin_website_data(&self, origin: &str) -> Result<(), NativeActionError> {
        self.handle
            .run_mobile_plugin::<()>("purgeOriginWebsiteData", PurgeOriginArgs { origin })
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn save_file(
        &self,
        file_name: &str,
        media_type: &str,
        contents: &[u8],
    ) -> Result<Option<String>, NativeActionError> {
        let contents = std::str::from_utf8(contents)
            .map_err(|_| NativeActionError::Failed("saved file is not text".to_owned()))?;
        self.handle
            .run_mobile_plugin::<SaveFileResponse>(
                "saveFile",
                SaveFileArgs {
                    file_name,
                    media_type,
                    contents,
                },
            )
            .map(|response| response.location)
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn set_narration_playback(
        &self,
        playback: Option<NarrationPlayback<'_>>,
    ) -> Result<(), NativeActionError> {
        let args = NarrationPlaybackArgs {
            state: playback.map(|playback| playback.state),
            title: playback.map_or("", |playback| playback.title),
            section: playback.and_then(|playback| playback.section),
            position_ms: playback.and_then(|playback| playback.position_ms),
            duration_ms: playback.and_then(|playback| playback.duration_ms),
            rate: playback.map_or(1.0, |playback| playback.rate),
        };
        self.handle
            .run_mobile_plugin::<()>("setNarrationPlayback", args)
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn take_annotation_links(&self) -> Result<Vec<String>, NativeActionError> {
        self.handle
            .run_mobile_plugin::<AnnotationLinksResponse>("takeAnnotationLinks", ())
            .map(|response| response.links)
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn take_narration_commands(&self) -> Result<Vec<NarrationCommand>, NativeActionError> {
        self.handle
            .run_mobile_plugin::<NarrationCommandsResponse>("takeNarrationCommands", ())
            .map(|response| response.commands)
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn set_side_buttons_turn_pages(&self, enabled: bool) -> Result<(), NativeActionError> {
        self.handle
            .run_mobile_plugin::<()>("setSideButtonsTurnPages", SideButtonsArgs { enabled })
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }

    fn take_opened_publications(&self) -> Result<Vec<OpenedPublication>, NativeActionError> {
        self.handle
            .run_mobile_plugin::<OpenedPublicationsResponse>("takeOpenedPublications", ())
            .map(|response| {
                response
                    .publications
                    .into_iter()
                    .map(|publication| OpenedPublication {
                        file_name: publication.file_name,
                        path: std::path::PathBuf::from(publication.path),
                        // The platform staged a copy for this process, which owns it from here.
                        owned: true,
                        intent: OpenedPublicationIntent::Read,
                    })
                    .collect()
            })
            .map_err(|error| NativeActionError::Failed(error.to_string()))
    }
}

#[cfg(desktop)]
struct DesktopNativeActions<R: Runtime> {
    links: crate::instance::LinkInbox,
    app: AppHandle<R>,
}

#[cfg(desktop)]
impl<R: Runtime> DesktopNativeActions<R> {
    fn main_window(
        &self,
    ) -> Result<tauri::WebviewWindow<R>, elephant_ladder_native_host::NativeActionError> {
        self.app.get_webview_window("main").ok_or_else(|| {
            elephant_ladder_native_host::NativeActionError::Failed("main window closed".to_owned())
        })
    }
}

#[cfg(desktop)]
impl<R: Runtime> NativeActions for DesktopNativeActions<R> {
    fn window_fullscreen_available(&self) -> bool {
        true
    }

    fn set_window_fullscreen(
        &self,
        fullscreen: bool,
    ) -> Result<(), elephant_ladder_native_host::NativeActionError> {
        self.main_window()?
            .set_fullscreen(fullscreen)
            .map_err(|error| {
                elephant_ladder_native_host::NativeActionError::Failed(error.to_string())
            })
    }

    fn window_fullscreen(&self) -> Result<bool, elephant_ladder_native_host::NativeActionError> {
        self.main_window()?.is_fullscreen().map_err(|error| {
            elephant_ladder_native_host::NativeActionError::Failed(error.to_string())
        })
    }

    fn open_external_url(
        &self,
        url: &str,
    ) -> Result<(), elephant_ladder_native_host::NativeActionError> {
        open::that(url).map_err(|error| {
            elephant_ladder_native_host::NativeActionError::Failed(error.to_string())
        })
    }

    fn take_annotation_links(
        &self,
    ) -> Result<Vec<String>, elephant_ladder_native_host::NativeActionError> {
        Ok(self.links.take())
    }

    fn take_opened_publications(
        &self,
    ) -> Result<
        Vec<elephant_ladder_native_host::OpenedPublication>,
        elephant_ladder_native_host::NativeActionError,
    > {
        Ok(self
            .links
            .take_publications()
            .into_iter()
            .map(|handed| {
                let path = std::path::PathBuf::from(handed.path);
                elephant_ladder_native_host::OpenedPublication {
                    file_name: path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                    path,
                    owned: handed.owned,
                    intent: if handed.read {
                        elephant_ladder_native_host::OpenedPublicationIntent::Read
                    } else {
                        elephant_ladder_native_host::OpenedPublicationIntent::Keep
                    },
                }
            })
            .collect())
    }

    fn save_file(
        &self,
        file_name: &str,
        _media_type: &str,
        contents: &[u8],
    ) -> Result<Option<String>, elephant_ladder_native_host::NativeActionError> {
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(file_name)
            .add_filter("JSON", &["json"]);
        if let Some(directory) = dirs::download_dir() {
            dialog = dialog.set_directory(directory);
        }
        let Some(path) = dialog.save_file() else {
            return Ok(None);
        };
        std::fs::write(&path, contents).map_err(|error| {
            elephant_ladder_native_host::NativeActionError::Failed(error.to_string())
        })?;
        Ok(Some(path.to_string_lossy().into_owned()))
    }
}

#[cfg(target_os = "android")]
struct MobilePlugin<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            _app.manage(MobilePlugin(_api.register_android_plugin(
                ANDROID_PLUGIN_IDENTIFIER,
                "NativeActionsPlugin",
            )?));
            Ok(())
        })
        .build()
}

pub fn port<R: Runtime>(app: &AppHandle<R>) -> Arc<dyn NativeActions> {
    #[cfg(target_os = "android")]
    {
        let handle = app.state::<MobilePlugin<R>>().0.clone();
        let front_light_dialog_available = handle
            .run_mobile_plugin::<AvailabilityResponse>("frontLightDialogAvailable", ())
            .map(|response| response.available)
            .unwrap_or(false);
        Arc::new(AndroidNativeActions {
            display: handle
                .run_mobile_plugin("displayTechnology", ())
                .unwrap_or_default(),
            handle,
            front_light_dialog_available,
        })
    }

    #[cfg(desktop)]
    {
        Arc::new(DesktopNativeActions {
            links: app.state::<crate::instance::LinkInbox>().inner().clone(),
            app: app.clone(),
        })
    }

    #[cfg(not(any(desktop, target_os = "android")))]
    {
        let _ = app;
        Arc::new(UnsupportedNativeActions)
    }
}
