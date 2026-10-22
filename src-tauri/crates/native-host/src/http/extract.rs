//! Typed request-boundary extractors.
//!
//! Authorization is a type, not a convention: a handler that does not name one
//! of these cannot be reached without its check having run. This replaces the
//! hand-written authorization prologue that was repeated at every privileged
//! route.
//!
//! The lifecycle lock is deliberately *not* extracted here. Extractors run
//! before the request body is read from the socket, so acquiring the serialized
//! lifecycle operation during extraction would let a slow client stall every
//! lifecycle mutation. Handlers take that lock themselves, after extraction.

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Request, rejection::JsonRejection},
    http::request::Parts,
};
use serde::de::DeserializeOwned;

use super::auth::{authorize, authorize_safe_read};
use super::error::HostError;
use crate::ReaderState;

/// Proof that the request carries the exact trusted-reader origin and exactly
/// one well-formed bearer credential. Carries no data: it is the check itself.
pub(crate) struct Trusted;

impl FromRequestParts<ReaderState> for Trusted {
    type Rejection = HostError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ReaderState,
    ) -> Result<Self, Self::Rejection> {
        authorize(&parts.headers, &state.reader_origin, &state.token, false)
            .map(|()| Self)
            .map_err(HostError::Unauthorized)
    }
}

/// Proof for a safe read: an exact `Origin` when present, else same-origin Fetch
/// metadata, else the trusted-reader request header; plus the bearer credential.
pub(crate) struct TrustedRead;

impl FromRequestParts<ReaderState> for TrustedRead {
    type Rejection = HostError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ReaderState,
    ) -> Result<Self, Self::Rejection> {
        authorize_safe_read(&parts.headers, &state.reader_origin, &state.token)
            .map(|()| Self)
            .map_err(HostError::Unauthorized)
    }
}

/// Proof for a state-changing request: the trusted-reader boundary plus an exact
/// `application/json` content type. Does not read the body, so handlers that
/// decode their own body shape keep doing so.
pub(crate) struct TrustedWrite;

impl FromRequestParts<ReaderState> for TrustedWrite {
    type Rejection = HostError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ReaderState,
    ) -> Result<Self, Self::Rejection> {
        authorize(&parts.headers, &state.reader_origin, &state.token, true)
            .map(|()| Self)
            .map_err(HostError::Unauthorized)
    }
}

/// A state-changing request whose JSON body is decoded strictly. Authorization
/// runs before the body is read, so an unauthorized caller cannot probe body
/// decoding.
pub(crate) struct TrustedJson<T>(pub(crate) T);

impl<T> FromRequest<ReaderState> for TrustedJson<T>
where
    T: DeserializeOwned,
{
    type Rejection = HostError;

    async fn from_request(request: Request, state: &ReaderState) -> Result<Self, Self::Rejection> {
        let (parts, body) = request.into_parts();
        authorize(&parts.headers, &state.reader_origin, &state.token, true)
            .map_err(HostError::Unauthorized)?;
        let request = Request::from_parts(parts, body);
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| match rejection {
                JsonRejection::JsonDataError(_) => HostError::Unprocessable("invalid request body"),
                _ => HostError::BadRequest("invalid request body"),
            })
    }
}
