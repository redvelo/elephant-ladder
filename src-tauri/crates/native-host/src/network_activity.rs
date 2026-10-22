use crate::ActiveReaderSession;
use crate::ActiveReaderSessionView;
use crate::Arc;
use crate::Ordering;
use crate::ReaderState;
use crate::Weak;
use crate::has_exact_header;
use crate::http::TrustedRead;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Serialize;

#[derive(Default)]
pub(super) struct BrowserActivity {
    available: bool,
    stopped: bool,
}

/// Browser observation capability shared with the native platform controller.
#[derive(Clone)]
pub struct NetworkActivityObserver {
    active: ActiveReaderSessionView,
}

/// A generation-bound owner of observations; does not retain the publication.
#[derive(Clone)]
pub struct SessionNetworkActivitySink {
    observer: NetworkActivityObserver,
    session: Weak<ActiveReaderSession>,
    session_id: Arc<str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    session_id: String,
    available: bool,
    last_observed_at: Option<u64>,
}

impl NetworkActivityObserver {
    pub(super) fn new(active: ActiveReaderSessionView) -> Self {
        Self { active }
    }

    fn current(&self) -> Option<Arc<ActiveReaderSession>> {
        if self.active.network_activity.lock().unwrap().stopped {
            return None;
        }
        self.active
            .load()
            .filter(|session| !session.revoked.load(Ordering::Acquire))
    }

    /// Capture before asynchronous CDP work, then require the bound sink's
    /// session_id() to match before delivering observations (origins can be reused).
    pub fn current_session_id(&self) -> Option<String> {
        self.current()
            .map(|session| session.reader_session_id.to_string())
    }

    /// Accepts exact foreground/nested origins, never measurement or remote origins.
    pub fn bind_frame_origin(&self, origin: &str) -> Option<SessionNetworkActivitySink> {
        let session = self.current()?;
        if origin != session.authorities.foreground_origin.as_ref()
            && origin != session.authorities.nested_origin.as_ref()
        {
            return None;
        }
        Some(SessionNetworkActivitySink {
            observer: self.clone(),
            session: Arc::downgrade(&session),
            session_id: session.reader_session_id.clone(),
        })
    }

    /// Enable only after the platform observer has successfully attached.
    pub fn set_available(&self, available: bool) {
        let mut state = self.active.network_activity.lock().unwrap();
        state.available = available && !state.stopped;
    }

    pub(super) fn shutdown(&self) {
        let mut state = self.active.network_activity.lock().unwrap();
        state.available = false;
        state.stopped = true;
    }

    fn snapshot(&self) -> Option<Snapshot> {
        let session = self.current()?;
        let last_observed_at = *session.network_activity.lock().unwrap();
        let state = self.active.network_activity.lock().unwrap();
        if state.stopped || session.revoked.load(Ordering::Acquire) {
            return None;
        }
        Some(Snapshot {
            session_id: session.reader_session_id.to_string(),
            available: state.available,
            last_observed_at,
        })
    }
}

impl SessionNetworkActivitySink {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn is_active(&self) -> bool {
        self.session.upgrade().is_some_and(|session| {
            self.observer
                .current()
                .is_some_and(|current| Arc::ptr_eq(&session, &current))
        })
    }

    /// Records the latest observed epoch milliseconds, never the time of a poll.
    pub fn observe_at(&self, epoch_ms: u64) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let mut last = session.network_activity.lock().unwrap();
        if self.is_active() {
            *last = Some(last.map_or(epoch_ms, |previous| previous.max(epoch_ms)));
        }
    }
}

pub(super) async fn read(
    State(state): State<ReaderState>,
    headers: HeaderMap,
    _: TrustedRead,
) -> Response {
    let mut response = {
        match NetworkActivityObserver::new(state.lifecycle.active_view()).snapshot() {
            Some(snapshot)
                if has_exact_header(
                    &headers,
                    "x-elephant-reader-session",
                    &snapshot.session_id,
                ) =>
            {
                Json(snapshot).into_response()
            }
            _ => (
                StatusCode::CONFLICT,
                "reader session is unavailable or stale",
            )
                .into_response(),
        }
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DiagnosticsLevel, HostInfo, PreparedHost, UnsupportedNativeActions};

    #[test]
    fn network_activity_host_stop_permanently_disables_observer() {
        let directory = tempfile::tempdir().unwrap();
        let prepared =
            PreparedHost::bind(directory.path(), crate::HostExtensions::default()).unwrap();
        let observer = prepared.network_activity_observer();
        assert!(!observer.active.network_activity.lock().unwrap().available);
        observer.set_available(true);
        assert!(observer.active.network_activity.lock().unwrap().available);
        let running = prepared.start(
            HostInfo {
                application: "test".to_owned(),
                diagnostics_level: DiagnosticsLevel::Summary,
            },
            Arc::new(UnsupportedNativeActions),
        );
        running.stop();
        observer.set_available(true);
        assert!(!observer.active.network_activity.lock().unwrap().available);
        assert!(observer.current_session_id().is_none());
        assert!(observer.snapshot().is_none());
    }
}
