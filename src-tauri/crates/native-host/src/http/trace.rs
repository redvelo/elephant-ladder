//! Request tracing for every listener.
//!
//! Stream completion and cancellation are recorded separately from handler
//! completion, and the records carry no credentials, authorities, private
//! identities, authored paths, or reading targets.

use crate::{HTTP_REQUEST_SEQUENCE, duration_milliseconds};
use axum::{
    body::Body, extract::MatchedPath, extract::State, http::Request, http::StatusCode,
    middleware::Next, response::Response,
};
use std::{sync::atomic::Ordering, time::Instant};
use tracing::Instrument;

#[derive(Clone, Copy)]
pub enum HttpServerRole {
    Reader,
    ContributedPublication,
    ImportedPublication,
    NestedPublication,
    Measurement,
}

#[derive(Clone, Copy)]
pub struct HttpObservation {
    pub role: HttpServerRole,
}

pub(crate) struct HttpRequestTrace {
    pub(crate) dispatch: tracing::Dispatch,
    pub(crate) request_id: u64,
    pub(crate) role: &'static str,
    pub(crate) method: String,
    pub(crate) route: String,
    pub(crate) started: Instant,
    pub(crate) finished: bool,
}

pub async fn observe_http(
    State(observation): State<HttpObservation>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let request_id = HTTP_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let role = observation.role.name();
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| observation.role.fallback_route().to_owned());
    let span = tracing::debug_span!(
        "http_request",
        request_id,
        server_role = role,
        method,
        route
    );
    let mut request_trace = HttpRequestTrace {
        dispatch: tracing::dispatcher::get_default(Clone::clone),
        request_id,
        role,
        method,
        route,
        started: Instant::now(),
        finished: false,
    };
    let response = next.run(request).instrument(span).await;
    request_trace.finish(response.status());
    response
}

impl HttpServerRole {
    fn name(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::ContributedPublication => "contributed-publication",
            Self::ImportedPublication => "imported-publication",
            Self::NestedPublication => "nested-publication",
            Self::Measurement => "measurement",
        }
    }

    fn fallback_route(self) -> &'static str {
        match self {
            Self::Reader => "reader-fallback",
            Self::ContributedPublication => "contributed-publication-resource",
            Self::ImportedPublication => "imported-publication-resource",
            Self::NestedPublication => "nested-publication-resource",
            Self::Measurement => "measurement-publication-resource",
        }
    }
}

impl HttpRequestTrace {
    fn finish(&mut self, status: StatusCode) {
        self.finished = true;
        let elapsed_ms = duration_milliseconds(self.started.elapsed());
        if status.is_server_error() {
            tracing::error!(
                event = "http_request_finished",
                request_id = self.request_id,
                server_role = self.role,
                method = self.method,
                route = self.route,
                status = status.as_u16(),
                elapsed_ms,
                outcome = "server-error"
            );
        } else {
            tracing::debug!(
                event = "http_request_finished",
                request_id = self.request_id,
                server_role = self.role,
                method = self.method,
                route = self.route,
                status = status.as_u16(),
                elapsed_ms,
                outcome = "completed"
            );
        }
    }
}

impl Drop for HttpRequestTrace {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        tracing::dispatcher::with_default(&self.dispatch, || {
            tracing::warn!(
                event = "http_request_cancelled",
                request_id = self.request_id,
                server_role = self.role,
                method = self.method,
                route = self.route,
                elapsed_ms = duration_milliseconds(self.started.elapsed()),
                outcome = "cancelled"
            );
        });
    }
}
