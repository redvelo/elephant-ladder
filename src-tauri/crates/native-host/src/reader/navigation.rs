//! Which origins the WebView may navigate to, and how the reader is reached again
//! after the platform discards the page.
//!
//! Recovery mints a fresh bootstrap nonce rather than reusing the previous URL,
//! so a recovered navigation cannot replay an earlier session's credential.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use crate::http::auth::generate_session_token;
use crate::policy::{ResolvedActiveContentPolicy, ResolvedRemoteResourcesPolicy};
use crate::reader::{NavigationBootstrap, NavigationTicket, ReadingBootstrapSeed};
use crate::reader_lifecycle::{ActiveReaderSessionView, ReaderLifecycle};
use crate::session::{
    ActiveReaderSession, allocate_session_authorities, live_pagination_descriptor,
};

#[derive(Clone)]
pub struct ContentNavigationPolicy {
    pub(crate) active_reader_session: ActiveReaderSessionView,
}

#[derive(Clone)]
pub struct NavigationRecovery {
    pub(crate) reader_origin: Arc<str>,
    pub(crate) lifecycle: Arc<ReaderLifecycle>,
    pub(crate) imported_publication_port: u16,
    pub(crate) nested_publication_port: u16,
    pub(crate) measurement_port: u16,
}

impl NavigationRecovery {
    pub async fn recreate_url(&self) -> Result<String, String> {
        let mut operation = self.lifecycle.lock().await;
        let nonce = generate_session_token().map_err(|error| error.to_string())?;
        let bootstrap = operation
            .navigation_ticket
            .as_ref()
            .map(|ticket| ticket.bootstrap.clone())
            .ok_or_else(|| "navigation recovery is unavailable".to_owned())?;
        match bootstrap {
            NavigationBootstrap::Library { .. } => {
                operation
                    .navigation_ticket
                    .as_mut()
                    .expect("validated navigation ticket must remain available")
                    .nonce = nonce.clone();
            }
            NavigationBootstrap::Reading(reading) => {
                let current = self
                    .lifecycle
                    .load_active()
                    .ok_or_else(|| "reader session is unavailable".to_owned())?;
                if current.revoked.load(Ordering::Acquire)
                    || current.reader_session_id.as_ref() != reading.reader_session_id
                {
                    return Err("reader session was replaced".to_owned());
                }
                let reader_session_id =
                    generate_session_token().map_err(|error| error.to_string())?;
                let content_script_nonce = matches!(
                    current.policy.active_content,
                    ResolvedActiveContentPolicy::Disabled
                )
                .then(|| Arc::from(reader_session_id.as_str()));
                let pagination_handle =
                    generate_session_token().map_err(|error| error.to_string())?;
                let authorities = allocate_session_authorities(
                    &current.browser_origin_id,
                    self.imported_publication_port,
                    self.nested_publication_port,
                    self.measurement_port,
                )?;
                let publication = serde_json::json!({
                    "facts": current.entry.facts.as_ref(),
                    "mount": {
                        "baseUrl": format!("{}/", authorities.foreground_origin),
                        "remoteResources": match current.policy.remote_resources {
                            ResolvedRemoteResourcesPolicy::Allowed => "allowed",
                            ResolvedRemoteResourcesPolicy::Blocked => "blocked",
                        },
                        "contentOrigin": "isolated-publication-origin"
                    }
                });
                let pagination = live_pagination_descriptor(
                    &current.entry.responses,
                    &authorities.measurement_origin,
                    &pagination_handle,
                );
                let replacement = Arc::new(ActiveReaderSession {
                    reader_session_id: Arc::from(reader_session_id.as_str()),
                    pagination_handle: Arc::from(pagination_handle.as_str()),
                    publication_id: current.publication_id.clone(),
                    browser_origin_id: current.browser_origin_id.clone(),
                    entry: current.entry.clone(),
                    trace_id: current.trace_id.clone(),
                    authorities: authorities.clone(),
                    policy: current.policy,
                    content_script_nonce,
                    revoked: AtomicBool::new(false),
                    last_accepted_position_sequence: AtomicU32::new(0),
                    network_activity: Mutex::new(None),
                    dictionary: current.dictionary.clone(),
                });
                operation.replace(replacement);
                operation.navigation_ticket = Some(NavigationTicket {
                    nonce: nonce.clone(),
                    bootstrap: NavigationBootstrap::Reading(Box::new(ReadingBootstrapSeed {
                        reader_session_id,
                        publication_id: reading.publication_id.clone(),
                        publication_origin: authorities.foreground_origin.to_string(),
                        measurement_origin: authorities.measurement_origin.to_string(),
                        publication,
                        initial_reading_position: reading.initial_reading_position.clone(),
                        position_revision: reading.position_revision,
                        annotations: reading.annotations.clone(),
                        annotation_identifiers: reading.annotation_identifiers.clone(),
                        preferences: reading.preferences.clone(),
                        preference_revisions: reading.preference_revisions.clone(),
                        policy: current.policy,
                        pagination,
                        media_overlay: current.entry.media_overlay_capability.clone(),
                        dictionary: current.dictionary.capability(),
                    })),
                });
            }
        }
        Ok(format!("{}/window/{nonce}/start", self.reader_origin))
    }
}

impl ContentNavigationPolicy {
    pub fn allows_origin(&self, origin: &str) -> bool {
        let active = self.active_reader_session.load();
        active.as_ref().is_some_and(|session| {
            !session.revoked.load(Ordering::Acquire)
                && (origin == session.authorities.foreground_origin.as_ref()
                    || origin == session.authorities.nested_origin.as_ref()
                    || origin == session.authorities.measurement_origin.as_ref())
        })
    }
}
