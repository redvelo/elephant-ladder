//! Reusable request and response mechanics for the native host's listeners.
//!
//! This module owns the transport boundary only. It holds no publication,
//! library, or reader-state knowledge: those belong to the domain modules that
//! mount their own routes.

pub mod auth;
mod error;
mod extract;
mod respond;
pub mod router;
mod trace;

pub(crate) use error::{Conflict, HostError, HostResult};
pub(crate) use extract::{Trusted, TrustedJson, TrustedRead, TrustedWrite};
pub use respond::{not_found, not_found_response};
pub(crate) use respond::{response_with_bytes, session_headers};
pub use trace::{HttpObservation, HttpServerRole, observe_http};
