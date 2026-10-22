//! What a non-product build contributes to a running host.
//!
//! A contribution is a trusted-reader page and the routes it reports through.
//! Publications are not part of it: a contributed publication is handed over the
//! way the platform hands one over and travels the product's own import and open
//! path, so the host has no plane, policy, or bootstrap that a product build
//! lacks. This is what keeps conformance code out of the product: a product
//! build contributes nothing, and no branch names a fixture.

use crate::host::state::ReaderState;

/// What a non-product build contributes to a host it prepares.
///
/// A product build contributes nothing, so every field is empty and the host
/// takes the same paths it would have taken without this type existing.
#[derive(Default)]
pub struct HostExtensions {
    /// Routes merged into the trusted reader origin.
    pub reader_routes: Option<axum::Router<ReaderState>>,
    /// The reader bundle to serve instead of the product one.
    pub reader_assets: Option<&'static include_dir::Dir<'static>>,
}
