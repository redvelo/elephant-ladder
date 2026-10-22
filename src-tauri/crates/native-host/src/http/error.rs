//! The one error shape every reader-origin handler returns.
//!
//! Handlers return [`HostResult`], so a failure is a value with a status, a
//! stable machine-readable code, and — for a compare-and-set conflict — the
//! server's current projection of the contested state. Before this, each domain
//! spelled its own rejection: bare status/text tuples, four differently shaped
//! conflict bodies, and a private constructor per failure mode.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub(crate) type HostResult<T = Response> = Result<T, HostError>;

pub(crate) enum HostError {
    Unauthorized(&'static str),
    BadRequest(&'static str),
    /// Syntactically valid but semantically rejected: the body parsed and the
    /// shape did not match.
    Unprocessable(&'static str),
    NotFound,
    Conflict(Conflict),
    /// The operation is well-formed but its subject cannot serve it right now.
    Unavailable(&'static str),
    /// The platform does not implement this operation at all.
    Unsupported(&'static str),
    /// A remote the host depends on could not be reached or did not answer
    /// acceptably.
    Upstream(&'static str),
    /// Never carries detail to the client: the operation name is logged here and
    /// the client is told only that the operation failed.
    Internal(&'static str),
}

/// A compare-and-set rejection.
///
/// `authoritative` is what the server currently holds for the contested state,
/// so a client can reconcile from the rejection alone rather than issuing a
/// follow-up read that may race with another writer.
pub(crate) struct Conflict {
    reason: &'static str,
    authoritative: serde_json::Value,
    request_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictBody<'a> {
    status: &'static str,
    reason: &'static str,
    authoritative: &'a serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: &'static str,
    message: &'static str,
}

impl Conflict {
    pub(crate) fn new(reason: &'static str, authoritative: serde_json::Value) -> Self {
        Self {
            reason,
            authoritative,
            request_id: None,
        }
    }

    /// Correlates the rejection with the mutation that lost, for a client that
    /// pipelines more than one in flight.
    pub(crate) fn for_request(mut self, request_id: String) -> Self {
        self.request_id = Some(request_id);
        self
    }
}

impl HostError {
    pub(crate) fn conflict(reason: &'static str, authoritative: serde_json::Value) -> Self {
        Self::Conflict(Conflict::new(reason, authoritative))
    }

    /// A conflict the client cannot reconcile from a projection, such as a stale
    /// library document or a publication that is open.
    pub(crate) fn conflict_reason(reason: &'static str) -> Self {
        Self::Conflict(Conflict::new(reason, serde_json::Value::Null))
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized(code)
            | Self::BadRequest(code)
            | Self::Unprocessable(code)
            | Self::Unavailable(code)
            | Self::Unsupported(code)
            | Self::Upstream(code)
            | Self::Internal(code) => code,
            Self::NotFound => "not-found",
            Self::Conflict(conflict) => conflict.reason,
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for HostError {
    fn into_response(self) -> Response {
        let status = self.status();
        if let Self::Conflict(conflict) = &self {
            return (
                status,
                Json(ConflictBody {
                    status: "conflict",
                    reason: conflict.reason,
                    authoritative: &conflict.authoritative,
                    request_id: conflict.request_id.as_deref(),
                }),
            )
                .into_response();
        }
        if let Self::Internal(operation) = &self {
            tracing::error!(event = "host_operation_failed", operation);
        }
        (
            status,
            Json(ErrorBody {
                error: self.code(),
                message: status.canonical_reason().unwrap_or("request failed"),
            }),
        )
            .into_response()
    }
}
