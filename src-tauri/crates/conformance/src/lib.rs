//! The conformance and probe test bed: never part of a product build.
//!
//! Everything here exists to qualify the host against fixtures, and it reaches a
//! binary only when that binary depends on this crate. A fixture is handed to the
//! host the way the platform hands over a book, so it travels the product's own
//! import, policy, and open path; what this crate adds is the runner page, the
//! route it reports through, and the evidence it persists.

mod artifact;
mod catalog;
mod report;
mod runner;
mod validate;

pub use artifact::{
    PEER_PROBE_SCENARIOS, PRIMARY_PROBE_SCENARIOS, PersistedEvidence,
    PersistedPublicationObservations, fixture_evidence_terminal_event, fixture_terminal_event,
    persist_fixture_evidence, persist_fixture_terminal_event, persist_publication_observations,
};
pub use catalog::{
    ACTIVE_PROBE_BYTES, ACTIVE_PROBE_TWIN_BYTES, CONTAINER_CONSTRAINED_BYTES,
    EVIDENCE_RESULT_CONTRACT, MAX_OBSERVATION_PLAN_BYTES, REMOTE_POLICY_BYTES, RUNNER_CATALOG,
    RUNNER_FIXTURE_NOTICE, RUNNER_POLICY_PROFILE, RunnerCatalogEntry, RunnerFixtureDescriptor,
    RunnerFixtureSelection, RunnerFixtureSource, valid_fixture_id,
};
pub use report::{
    ActiveProbeReport, ConformanceCapture, ConformanceSubmission, ContainerConstrainedReport,
    EvidenceSubmission, FixtureEvidenceArtifact, PublicationObservation,
    PublicationObservationsArtifact, PublicationObservationsFixture, PublicationRect,
    RemotePolicyReport, StaticFxlCompanion, StaticFxlParticipant, StaticFxlSnapshot,
};
pub use runner::{
    MAX_EVIDENCE_REQUEST_BYTES, RunnerState, contribute, reader_routes, write_fixture,
};
pub use validate::{
    MAX_EVIDENCE_DOCUMENTS, validate_active_probe_reports, validate_container_constrained_reports,
    validate_evidence_submission, validate_probe_evidence, validate_probe_evidence_value,
    validate_publication_observations, validate_rect, validate_remote_policy_reports,
};
