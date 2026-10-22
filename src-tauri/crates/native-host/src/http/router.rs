//! The trusted reader origin's route table.
//!
//! Every domain contributes its own router and they are merged flat, because the
//! paths they own do not share a prefix. Authorization and tracing wrap the whole
//! table, so no route can be added without them.

use axum::{Router, http::HeaderMap, middleware, routing::get};

use crate::http::auth::constant_time_equal;
use crate::http::not_found;
use crate::http::{HttpObservation, HttpServerRole};
use crate::reader::{self, reader_asset, reader_policy};
use crate::{
    LIBRARY_DOCUMENT_HEADER, ReaderState, device, diagnostics, dictionary, library, media_overlay,
    observe_http, pagination, policy, reading, search, session,
};

pub fn reader_router(state: ReaderState, extension: Option<Router<ReaderState>>) -> Router {
    let router = Router::new()
        .merge(reader::routes())
        .merge(dictionary::routes())
        .merge(device::routes())
        .merge(library::routes())
        .merge(session::routes())
        .merge(search::routes())
        .merge(policy::routes())
        .merge(reading::routes())
        .merge(media_overlay::routes())
        .merge(crate::notes::routes())
        .merge(pagination::routes())
        .merge(diagnostics::routes())
        .route("/{*path}", get(reader_asset));
    // A product build contributes nothing here, so it has no route that accepts
    // what a fixture run reports.
    let router = match extension {
        Some(routes) => router.merge(routes),
        None => router,
    };
    router
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), reader_policy))
        .layer(middleware::from_fn_with_state(
            HttpObservation {
                role: HttpServerRole::Reader,
            },
            observe_http,
        ))
        .with_state(state)
}

pub(crate) fn request_library_document(
    headers: &HeaderMap,
    state: &ReaderState,
) -> Result<String, &'static str> {
    let mut values = headers.get_all(LIBRARY_DOCUMENT_HEADER).iter();
    let supplied = values.next().ok_or("missing library document")?;
    if values.next().is_some() {
        return Err("invalid library document");
    }
    let supplied = supplied.to_str().map_err(|_| "invalid library document")?;
    let current = state
        .library_document_id
        .read()
        .expect("library document lock poisoned");
    if current
        .as_ref()
        .is_none_or(|current| !constant_time_equal(supplied.as_bytes(), current.as_bytes()))
    {
        return Err("stale library document");
    }
    Ok(supplied.to_owned())
}
