//! The Chrome DevTools Protocol surface this application observes.
//!
//! Parsing is bounded and session-local: no complete URL, header, or body
//! survives it. The crate holds no application state and no platform binding, so
//! the observer that drives it stays in the host.

pub mod protocol;
pub mod targets;

/// A message this crate will not act on.
///
/// Parsing deliberately retains nothing about what was rejected, so this carries
/// no detail: a rejected message is discarded, never reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rejected;
