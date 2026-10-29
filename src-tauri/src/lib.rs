use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(all(feature = "fixture-runner", target_os = "android"))]
use std::sync::OnceLock;

#[cfg(feature = "fixture-runner")]
use elephant_ladder_conformance::RunnerFixtureSelection;
use elephant_ladder_native_host::{
    ContentNavigationPolicy, DiagnosticsLevel, HostInfo, NavigationRecovery, PreparedHost,
    RunningHost,
};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tracing_subscriber::filter::LevelFilter;

mod native_actions;

#[cfg(desktop)]
mod instance;
#[cfg(desktop)]
mod network_observer;

#[cfg(all(feature = "fixture-runner", target_os = "android"))]
static ANDROID_RUNNER_FIXTURE_ARGUMENTS: OnceLock<Vec<String>> = OnceLock::new();

const DISABLED_BROWSER_IPC_SCRIPT: &str = r#"
Object.defineProperty(window.__TAURI_INTERNALS__, 'postMessage', {
  value: Object.freeze(function () {
    throw new Error('Tauri IPC is disabled by Elephant Ladder');
  })
});
"#;

/// The activity hands over the same command line a desktop run would be started with.
#[cfg(all(feature = "fixture-runner", target_os = "android"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_redvelo_elephant_1ladder_MainActivity_installFixtureSelection(
    mut environment: jni::JNIEnv<'_>,
    _activity: jni::objects::JObject<'_>,
    arguments: jni::objects::JObjectArray<'_>,
) -> jni::sys::jboolean {
    let Ok(length) = environment.get_array_length(&arguments) else {
        return jni::sys::JNI_FALSE;
    };
    let mut collected = Vec::with_capacity(length as usize);
    for index in 0..length {
        let Ok(element) = environment.get_object_array_element(&arguments, index) else {
            return jni::sys::JNI_FALSE;
        };
        let element = jni::objects::JString::from(element);
        let Ok(value) = environment.get_string(&element) else {
            return jni::sys::JNI_FALSE;
        };
        collected.push(String::from(value));
    }
    if RunnerFixtureSelection::parse_args(&collected).is_err() {
        return jni::sys::JNI_FALSE;
    }
    match ANDROID_RUNNER_FIXTURE_ARGUMENTS.set(collected.clone()) {
        Ok(()) => jni::sys::JNI_TRUE,
        Err(_) if ANDROID_RUNNER_FIXTURE_ARGUMENTS.get() == Some(&collected) => jni::sys::JNI_TRUE,
        Err(_) => jni::sys::JNI_FALSE,
    }
}

#[derive(Default)]
struct HostLifecycle(Mutex<Option<HostRuntime>>);

struct HostRuntime {
    running: RunningHost,
    recovery: NavigationRecovery,
    reader_origin: String,
    content_navigation_policy: ContentNavigationPolicy,
    webview_data_directory: PathBuf,
    #[cfg(desktop)]
    network_observer: network_observer::Observer,
}

impl HostLifecycle {
    fn stop(&self) {
        if let Some(runtime) = self.0.lock().expect("host lifecycle lock poisoned").take() {
            #[cfg(desktop)]
            runtime.network_observer.stop();
            runtime.running.stop();
        }
    }

    fn recovery_window(
        &self,
    ) -> Result<Option<(tauri::Url, String, ContentNavigationPolicy, PathBuf)>, String> {
        let runtime_guard = self.0.lock().expect("host lifecycle lock poisoned");
        let Some(runtime) = runtime_guard.as_ref() else {
            return Ok(None);
        };
        let recovery = runtime.recovery.clone();
        let reader_origin = runtime.reader_origin.clone();
        let content_navigation_policy = runtime.content_navigation_policy.clone();
        let webview_data_directory = runtime.webview_data_directory.clone();
        drop(runtime_guard);
        let url = tauri::async_runtime::block_on(recovery.recreate_url())?
            .parse()
            .map_err(|error| format!("invalid recovery URL: {error}"))?;
        Ok(Some((
            url,
            reader_origin,
            content_navigation_policy,
            webview_data_directory,
        )))
    }
}

fn webview_data_directory(app_data_directory: &Path) -> PathBuf {
    app_data_directory.join("webview")
}

fn build_main_window(
    app: &AppHandle,
    reader_url: tauri::Url,
    reader_origin: String,
    content_navigation_policy: ContentNavigationPolicy,
    webview_data_directory: PathBuf,
    window_size: Option<(u32, u32)>,
) -> tauri::Result<()> {
    #[cfg(feature = "fixture-runner")]
    let title = "Elephant Ladder Fixture Runner";
    #[cfg(not(feature = "fixture-runner"))]
    let title = "Elephant Ladder";
    #[cfg(desktop)]
    let observer = app
        .state::<HostLifecycle>()
        .0
        .lock()
        .expect("host lifecycle lock poisoned")
        .as_ref()
        .map(|runtime| runtime.network_observer.clone())
        .ok_or_else(|| std::io::Error::other("host must be installed before the reader window"))?;
    #[cfg(desktop)]
    let bootstrap = std::sync::Arc::new(network_observer::Bootstrap::default());
    #[cfg(desktop)]
    let navigation_bootstrap = bootstrap.clone();
    #[cfg(desktop)]
    let initial_url = tauri::Url::parse("about:blank").expect("valid bootstrap URL");
    #[cfg(mobile)]
    let initial_url = reader_url;
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(initial_url))
        .title(title)
        .inner_size(
            f64::from(window_size.map_or(1100, |(width, _)| width)),
            f64::from(window_size.map_or(760, |(_, height)| height)),
        )
        .data_directory(webview_data_directory);
    let window = window
        .on_navigation(move |url| {
            let allowed = url.origin().ascii_serialization() == reader_origin
                || content_navigation_policy.allows_origin(&url.origin().ascii_serialization());
            #[cfg(desktop)]
            let allowed = navigation_bootstrap.allows(url, allowed);
            allowed
        })
        .build()?;
    // The page never sees an OS drag under this runtime: the window layer owns it and reports
    // the dropped paths. A drop is the same request as opening a book from a file manager, so
    // it travels the same way, and the reader imports what the window was handed.
    #[cfg(desktop)]
    {
        let dropped_inbox = app.state::<instance::LinkInbox>().inner().clone();
        window.on_window_event(move |event| {
            let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
            else {
                return;
            };
            for path in instance::dropped_publications(paths) {
                dropped_inbox.push_dropped(path);
            }
        });
    }
    #[cfg(desktop)]
    observer.attach(window, reader_url, bootstrap);
    #[cfg(mobile)]
    let _ = window;
    Ok(())
}

#[cfg_attr(
    any(target_os = "linux", target_os = "windows"),
    tauri_runtime_cef::cef_entry_point
)]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    initialize_tracing();
    #[cfg(all(feature = "fixture-runner", not(target_os = "android")))]
    let runner_fixture = RunnerFixtureSelection::parse_args(std::env::args_os().skip(1))
        .unwrap_or_else(|error| panic!("invalid fixture-runner command line: {error}"));
    #[cfg(all(feature = "fixture-runner", target_os = "android"))]
    let runner_fixture = RunnerFixtureSelection::parse_args(
        ANDROID_RUNNER_FIXTURE_ARGUMENTS
            .get()
            .expect("missing Android fixture-runner launch selection"),
    )
    .expect("the Android fixture-runner launch selection is valid");
    #[cfg(feature = "fixture-runner")]
    let exit_after_run = runner_fixture.exit_after_run();
    #[cfg(feature = "fixture-runner")]
    let window_size = runner_fixture.window;
    #[cfg(not(feature = "fixture-runner"))]
    let window_size: Option<(u32, u32)> = None;
    let context = tauri::generate_context!();
    #[cfg(desktop)]
    let link_inbox = instance::LinkInbox::default();
    #[cfg(all(target_os = "linux", not(feature = "fixture-runner")))]
    let instance_socket = {
        let app_data = dirs::data_dir()
            .expect("could not resolve the application data directory")
            .join(&context.config().identifier);
        let socket = instance::socket::socket_path(&app_data);
        let links = instance::socket::link_arguments(std::env::args().skip(1));
        if instance::socket::hand_off(&socket, &app_data, &links) {
            return;
        }
        // Nothing else is running, so any copy left in shared storage was never read.
        instance::socket::clear_handed_publications(&app_data);
        for link in links {
            link_inbox.push(link);
        }
        socket
    };
    // Browser code uses the authenticated loopback host exclusively. This custom
    // invoke system deliberately contains no __INVOKE_KEY__ placeholder.
    let builder = tauri::Builder::default().invoke_system(DISABLED_BROWSER_IPC_SCRIPT);
    #[cfg(desktop)]
    let builder = {
        let cef_root = dirs::data_dir()
            .expect("could not resolve the application data directory")
            .join(&context.config().identifier)
            .join("webview");
        let cef = tauri_runtime_cef::Cef::default()
            .root_cache_path(cef_root)
            // Windows has no CEF sandbox under this runtime: the broker needs the application
            // hosted as a DLL by CEF's bootstrap executable, and a Tauri application is an
            // executable. `Required` would refuse to start there, so Windows keeps the sandbox
            // when it can and logs when it cannot.
            .sandbox(if cfg!(target_os = "windows") {
                tauri_runtime_cef::SandboxPolicy::Auto
            } else {
                tauri_runtime_cef::SandboxPolicy::Required
            });
        // The reader, each publication and the measurement plane answer on their own
        // `.localhost` authority. A Flatpak with no network share cannot reach a resolver, so
        // the page never loads; they are loopback by definition, so Chromium is told directly.
        // The switch turns off a Chromium security boundary and Chromium says so, which is why
        // it is confined to the sandbox that needs it: elsewhere Chromium already treats
        // `*.localhost` as loopback under RFC 6761.
        #[cfg(target_os = "linux")]
        let cef = cef.command_line_arg("host-resolver-rules", Some("MAP *.localhost 127.0.0.1"));
        // Chromium keeps its cookie-encryption key in the macOS Keychain. A conformance run
        // redirects HOME to a throwaway directory and the binary is unsigned, so the system
        // cannot match an ACL and prompts on every launch; cancelling fails OSCrypt. The mock
        // keychain is confined to the fixture runner, which stores nothing worth encrypting.
        #[cfg(all(target_os = "macos", feature = "fixture-runner"))]
        let cef = cef.command_line_arg("use-mock-keychain", None::<&str>);
        // The same key lives in the Secret Service on Linux, and a login keyring starts locked:
        // the first run of a session blocks on an unlock dialog the collector cannot see, which
        // it reports as a timeout. The basic store keeps the fixture runner off the keyring.
        #[cfg(all(target_os = "linux", feature = "fixture-runner"))]
        let cef = cef.command_line_arg("password-store", Some("basic"));
        // Development builds expose the DevTools protocol on loopback so the running
        // window can be measured; packaged builds never open the port. The port is
        // selectable so several development instances can be measured at once.
        let cef = if tauri::is_dev() {
            let port = std::env::var("ELEPHANT_DEVTOOLS_PORT")
                .ok()
                .filter(|value| value.parse::<u16>().is_ok_and(|port| port != 0))
                .unwrap_or_else(|| "9223".to_owned());
            cef.command_line_arg("remote-debugging-port", Some(&port))
                .command_line_arg(
                    "remote-allow-origins",
                    Some(&format!("http://127.0.0.1:{port}")),
                )
        } else {
            cef
        };
        builder.runtime(cef)
    };
    #[cfg(target_os = "android")]
    let builder = builder.runtime(tauri_runtime_wry::Wry::default());
    #[cfg(desktop)]
    let builder = builder.manage(link_inbox);
    let app = builder
        .plugin(native_actions::init())
        .manage(HostLifecycle::default())
        .setup(move |app| {
            let app_data = app.path().app_data_dir().map_err(|error| {
                format!("could not resolve application data directory: {error}")
            })?;
            // Bulk, re-downloadable data stays out of the Windows roaming profile; on
            // Linux and Android this is the same directory as the application data.
            let app_local_data = app.path().app_local_data_dir().map_err(|error| {
                format!("could not resolve local application data directory: {error}")
            })?;
            let webview_data_directory = webview_data_directory(&app_data);
            #[cfg(feature = "fixture-runner")]
            let terminal_artifact = app_data.join("fixture-run-finished.json");
            #[cfg(feature = "fixture-runner")]
            if exit_after_run && terminal_artifact.exists() {
                std::fs::remove_file(&terminal_artifact).map_err(|error| {
                    format!("could not remove stale fixture terminal artifact: {error}")
                })?;
            }
            #[cfg(not(feature = "fixture-runner"))]
            let extensions = Ok(elephant_ladder_native_host::HostExtensions::default());
            #[cfg(feature = "fixture-runner")]
            let extensions = elephant_ladder_conformance::contribute(runner_fixture, &app_data);
            let host = extensions.and_then(|extensions| {
                PreparedHost::bind_with_bulk_data(&app_data, &app_local_data, extensions)
            });
            let host = host.map_err(|error| {
                // Preparation fails before any window exists, so nothing can be shown
                // to the reader and the process aborts. This line is the whole
                // diagnostic: on Android the database is app-private and `logcat` is
                // the only place the reason can be recovered from.
                tracing::error!(event = "native_host_prepare_failed", reason = %error);
                format!("failed to prepare the native EPUB host: {error}")
            })?;
            #[cfg(all(target_os = "linux", not(feature = "fixture-runner")))]
            {
                let handle = app.handle().clone();
                instance::socket::listen(
                    &instance_socket,
                    app.state::<instance::LinkInbox>().inner().clone(),
                    move || {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    },
                )
                .map_err(|error| format!("could not listen for application links: {error}"))?;
            }
            let reader_url: tauri::Url = host
                .launch_url()
                .parse()
                .map_err(|error| format!("invalid reader URL: {error}"))?;
            let reader_origin = reader_url.origin().ascii_serialization();
            let content_navigation_policy = host.content_navigation_policy();
            #[cfg(desktop)]
            let network_observer =
                network_observer::Observer::new(host.network_activity_observer());
            let package = app.package_info();
            let running = host.start(
                HostInfo {
                    application: package.name.clone(),
                    diagnostics_level: if cfg!(any(debug_assertions, feature = "diagnostic-trace"))
                    {
                        DiagnosticsLevel::Trace
                    } else {
                        DiagnosticsLevel::Summary
                    },
                },
                native_actions::port(app.handle()),
            );
            let recovery = running.navigation_recovery();
            app.state::<HostLifecycle>()
                .0
                .lock()
                .expect("host lifecycle lock poisoned")
                .replace(HostRuntime {
                    running,
                    recovery,
                    reader_origin: reader_origin.clone(),
                    content_navigation_policy: content_navigation_policy.clone(),
                    webview_data_directory: webview_data_directory.clone(),
                    #[cfg(desktop)]
                    network_observer,
                });

            build_main_window(
                app.handle(),
                reader_url,
                reader_origin,
                content_navigation_policy,
                webview_data_directory,
                window_size,
            )?;
            #[cfg(feature = "fixture-runner")]
            if exit_after_run {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    // Long enough for an evidence run to walk a ten-document reading order
                    // when every document waits out its settle deadline; the collector's own
                    // timeout is the one that fires first.
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
                    while std::time::Instant::now() < deadline {
                        if terminal_artifact.is_file() {
                            match handle.get_webview_window("main") {
                                Some(window) => {
                                    if let Err(error) = window.close() {
                                        tracing::error!(
                                            event = "fixture_runner_window_close_failed",
                                            error = %error
                                        );
                                        handle.exit(1);
                                    }
                                }
                                None => handle.exit(1),
                            }
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    tracing::error!(event = "fixture_runner_auto_exit_timeout");
                    handle.exit(1);
                });
            }
            Ok(())
        })
        .build(context)
        .expect("error while building Tauri application");

    app.run(move |handle, event| match event {
        RunEvent::ExitRequested { .. } | RunEvent::Exit => {
            handle.state::<HostLifecycle>().stop();
        }
        RunEvent::Resumed => {
            if handle.get_webview_window("main").is_none() {
                let recovery = handle.state::<HostLifecycle>().recovery_window();
                match recovery.and_then(|recovery| {
                    recovery.map_or(Ok(false), |(url, origin, policy, data_directory)| {
                        build_main_window(handle, url, origin, policy, data_directory, window_size)
                            .map(|_| true)
                            .map_err(|error| error.to_string())
                    })
                }) {
                    Ok(true) => tracing::info!(event = "reader_window_recovered"),
                    Ok(false) => {}
                    Err(_) => tracing::error!(
                        event = "reader_recovery_failed",
                        error_kind = "window-recovery"
                    ),
                }
            } else {
                tracing::info!(event = "host_resumed");
            }
        }
        _ => {}
    });
}

fn initialize_tracing() {
    let maximum_level = if cfg!(any(debug_assertions, feature = "diagnostic-trace")) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    tracing_subscriber::fmt()
        .json()
        .flatten_event(true)
        .with_ansi(false)
        .with_max_level(maximum_level)
        .try_init()
        .expect("failed to initialize application tracing");
}

#[cfg(test)]
mod tests {
    use super::{DISABLED_BROWSER_IPC_SCRIPT, webview_data_directory};
    use std::path::Path;

    #[test]
    fn webview_data_is_scoped_to_the_application_data_directory() {
        assert_eq!(
            webview_data_directory(Path::new("/app/data")),
            Path::new("/app/data/webview")
        );
    }

    #[test]
    fn browser_ipc_system_is_fail_closed_and_contains_no_invoke_key() {
        assert!(DISABLED_BROWSER_IPC_SCRIPT.contains("Tauri IPC is disabled by Elephant Ladder"));
        assert!(!DISABLED_BROWSER_IPC_SCRIPT.contains("__INVOKE_KEY__"));
        assert!(!DISABLED_BROWSER_IPC_SCRIPT.contains("window.ipc"));
    }
}
