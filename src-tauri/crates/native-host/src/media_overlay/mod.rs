//! Narration derived from Media Overlays: what the package declares, what the
//! SMIL and audio actually support, and what the reader may play.
//!
//! Reading availability never depends on narration, so every failure here lands
//! in an explicit unavailable state rather than failing the publication.

pub(crate) mod backing;
pub(crate) mod derive;
mod description;
pub(crate) mod playback;
mod routes;
pub(crate) mod transcode;
pub(crate) mod validate;

pub(crate) use description::{
    AvailablePackageFields, EffectiveLinear, FragmentExistence,
    MAX_MEDIA_OVERLAY_AMBIGUOUS_CANDIDATES, MAX_MEDIA_OVERLAY_ASSOCIATIONS,
    MAX_MEDIA_OVERLAY_COLLECTION_ITEMS, MAX_MEDIA_OVERLAY_COVERAGE_ENTRIES,
    MAX_MEDIA_OVERLAY_COVERAGE_ISSUES, MAX_MEDIA_OVERLAY_DECLARATIONS, MAX_MEDIA_OVERLAY_SEGMENTS,
    MAX_MEDIA_OVERLAY_SEMANTIC_DEPTH, MAX_MEDIA_OVERLAY_STRING_BYTES, MAX_MEDIA_OVERLAY_TIMELINES,
    MAX_MEDIA_OVERLAY_TOKENS_PER_NODE, MediaOverlayAnalysisIssue, MediaOverlayAssociation,
    MediaOverlayAvailabilityIssue, MediaOverlayBinding, MediaOverlayBindingAvailability,
    MediaOverlayBindingIssue, MediaOverlayCapability, MediaOverlayCoverage,
    MediaOverlayCoverageResult, MediaOverlayCoverageState, MediaOverlayDeclaration,
    MediaOverlayDescription, MediaOverlayDurationMetadata, MediaOverlayLocalBytes,
    MediaOverlayMalformedReason, MediaOverlayManifestId, MediaOverlayPackageFacts,
    MediaOverlayPackageIssue, MediaOverlayPackageMetadata, MediaOverlayPathCoverage,
    MediaOverlayPlayback, MediaOverlayReference, MediaOverlaySegment,
    MediaOverlaySegmentPlayability, MediaOverlaySegmentUnavailableReason, MediaOverlaySemanticNode,
    MediaOverlaySemanticNodeKind, MediaOverlayTimeline, MediaOverlayTimelineAvailability,
    MediaOverlayTimelineIssue, MediaOverlayTimelineSummary, MediaOverlayUnsupportedReason,
    OverlayAssociationState, ReadingOrderContentTarget, SmilClock, capability_matches_playback,
    derive_media_overlay_capability, representation_limited_description,
};
#[cfg(test)]
pub(crate) use description::{MediaOverlayTimelineResult, absent_media_overlay_description};
pub(crate) use routes::routes;
pub(crate) use validate::{validate_media_overlay_description, validate_timeline_for_description};

#[cfg(test)]
mod tests;
