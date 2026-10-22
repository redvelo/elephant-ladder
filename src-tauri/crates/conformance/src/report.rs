use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceSubmission {
    pub passed: u8,
    pub total: u8,
    pub failures: Vec<String>,
    pub capture: Option<ConformanceCapture>,
    #[serde(rename = "incompleteReason")]
    pub incomplete_reason: Option<String>,
    #[serde(rename = "publicationObservations")]
    pub publication_observations: Option<Vec<PublicationObservation>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceCapture {
    pub viewport_width: u16,
    pub viewport_height: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// One observed phase of a run: the authored reports the publication posted, and
/// for a static fixture the parent-side snapshot the runner took instead.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationObservation {
    pub generation: u8,
    pub source: String,
    pub reports: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<StaticFxlSnapshot>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticFxlSnapshot {
    pub kind: String,
    pub document_generation: u16,
    pub primary: StaticFxlParticipant,
    pub companion: Option<StaticFxlCompanion>,
    pub visible: epub_navigator::kernel::VisibleOccurrenceRange,
    pub host_rect: PublicationRect,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticFxlParticipant {
    pub occurrence: u16,
    pub resource: String,
    pub adapter: String,
    pub frame_generation: u16,
    pub layout_generation: u16,
    pub rect: PublicationRect,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StaticFxlCompanion {
    pub occurrence: u16,
    pub resource: String,
    pub adapter: String,
    pub frame_generation: u16,
    pub layout_generation: u16,
    pub rect: PublicationRect,
    pub owner_document_generation: u16,
    pub placement: epub_navigator::kernel::FixedSpreadPlacement,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationRect {
    pub x: f64,
    pub y: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveProbeReport {
    #[serde(rename = "type")]
    pub report_type: String,
    pub source: String,
    pub scenario: String,
    pub outcome: String,
    pub evidence: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerConstrainedReport {
    #[serde(rename = "type")]
    pub report_type: String,
    pub source: String,
    pub child_script: bool,
    pub epub_reading_system: bool,
    pub parent_dom_blocked: bool,
    pub frame_size_blocked: bool,
    pub reader_dom_blocked: bool,
    pub credentials_blocked: bool,
    pub native_route_blocked: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePolicyReport {
    #[serde(rename = "type")]
    pub report_type: String,
    pub source: String,
    pub https_fetch: String,
    pub https_image: String,
    pub https_style: String,
    pub https_script: String,
    pub https_audio: String,
    pub https_frame: String,
    pub wss_echo: String,
    pub http_fetch: String,
    pub ws_echo: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationObservationsArtifact {
    pub schema: &'static str,
    pub fixture: PublicationObservationsFixture,
    pub phases: Vec<PublicationObservation>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationObservationsFixture {
    pub id: String,
    pub sha256: String,
    pub blake3: String,
    pub result_contract: String,
    pub policy_profile: String,
    /// The origin the host served the fixture from, read from the live session
    /// rather than reported by the publication.
    pub publication_origin: String,
}

/// What an evidence run hands back: the runner's own environment, the publication
/// as Navigator was given it, and one record per reading-order document. The host
/// bounds the shape and keeps the values as the page reported them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceSubmission {
    pub runner: Value,
    pub publication: Value,
    pub documents: Vec<Value>,
    #[serde(default)]
    pub incomplete_reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureEvidenceArtifact {
    pub schema: &'static str,
    pub fixture: PublicationObservationsFixture,
    pub runner: Value,
    pub publication: Value,
    pub documents: Vec<Value>,
}
