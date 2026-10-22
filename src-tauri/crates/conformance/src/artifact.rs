use crate::*;
use elephant_ladder_native_host::auth::random_id;
use elephant_ladder_native_host::fsync_directory;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path as FsPath;

pub struct PersistedPublicationObservations {
    pub sha256: String,
    pub bytes: usize,
    pub phase_count: usize,
}

pub fn fixture_terminal_event(
    descriptor: &RunnerFixtureDescriptor,
    submission: &ConformanceSubmission,
    publication_origin: Option<&str>,
    publication_artifact: Option<&PersistedPublicationObservations>,
) -> Value {
    let mut event = serde_json::json!({
        "event": "fixture_run_finished",
        "run_status": if submission.incomplete_reason.is_some() { "incomplete" } else { "complete" },
        "fixture_id": descriptor.id,
        "fixture_blake3": descriptor.blake3,
        "fixture_sha256": descriptor.expected_sha256,
        "result_contract": descriptor.result_contract,
        "policy_profile": descriptor.policy_profile,
        "passed": submission.passed,
        "total": submission.total,
        "failure_count": submission.failures.len(),
        "failures": serde_json::to_string(&submission.failures).expect("string lists serialize"),
    });
    let fields = event.as_object_mut().expect("terminal event is an object");
    if let Some(origin) = publication_origin {
        fields.insert("publication_origin".into(), origin.into());
    }
    if let Some(capture) = submission.capture.as_ref() {
        fields.insert("viewport_width".into(), capture.viewport_width.into());
        fields.insert("viewport_height".into(), capture.viewport_height.into());
        fields.insert("capture_x".into(), capture.x.into());
        fields.insert("capture_y".into(), capture.y.into());
        fields.insert("capture_width".into(), capture.width.into());
        fields.insert("capture_height".into(), capture.height.into());
    }
    if let Some(artifact) = publication_artifact {
        fields.insert(
            "publication_observations_file".into(),
            "fixture-publication-observations.json".into(),
        );
        fields.insert(
            "publication_observations_sha256".into(),
            artifact.sha256.clone().into(),
        );
        fields.insert(
            "publication_observations_bytes".into(),
            artifact.bytes.into(),
        );
        fields.insert(
            "publication_observations_phase_count".into(),
            artifact.phase_count.into(),
        );
    }
    event
}

/// Writes the artifact so a reader never sees a partial one.
///
/// The bytes land in a uniquely named temporary beside the destination, are synced,
/// and are then renamed over it; the directory is synced so the rename survives a
/// crash (where a directory can be synced at all). A failed write takes its temporary with it.
fn write_atomically(
    app_data_dir: &FsPath,
    destination: &FsPath,
    bytes: &[u8],
    what: &str,
) -> Result<(), String> {
    let temporary = app_data_dir.join(format!(
        ".{}-{}.tmp",
        destination
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("artifact"),
        random_id(16).map_err(|error| error.to_string())?
    ));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, destination)?;
        fsync_directory(app_data_dir)
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not atomically write {what}: {error}"));
    }
    Ok(())
}

pub struct PersistedEvidence {
    pub sha256: String,
    pub bytes: usize,
    pub document_count: usize,
}

pub fn fixture_evidence_terminal_event(
    descriptor: &RunnerFixtureDescriptor,
    publication_origin: Option<&str>,
    artifact: Option<&PersistedEvidence>,
    incomplete_reason: Option<&str>,
) -> Value {
    let mut event = serde_json::json!({
        "event": "fixture_run_finished",
        "run_status": if incomplete_reason.is_some() { "incomplete" } else { "complete" },
        "fixture_id": descriptor.id,
        "fixture_blake3": descriptor.blake3,
        "fixture_sha256": descriptor.expected_sha256,
        "result_contract": descriptor.result_contract,
        "policy_profile": descriptor.policy_profile,
    });
    let fields = event.as_object_mut().expect("terminal event is an object");
    if let Some(origin) = publication_origin {
        fields.insert("publication_origin".into(), origin.into());
    }
    if let Some(reason) = incomplete_reason {
        fields.insert("incomplete_reason".into(), reason.into());
    }
    if let Some(artifact) = artifact {
        fields.insert("evidence_file".into(), "fixture-evidence.json".into());
        fields.insert("evidence_sha256".into(), artifact.sha256.clone().into());
        fields.insert("evidence_bytes".into(), artifact.bytes.into());
        fields.insert("evidence_documents".into(), artifact.document_count.into());
    }
    event
}

pub fn persist_fixture_evidence(
    app_data_dir: &FsPath,
    descriptor: &RunnerFixtureDescriptor,
    publication_origin: &str,
    submission: EvidenceSubmission,
) -> Result<PersistedEvidence, String> {
    let document_count = submission.documents.len();
    let artifact = FixtureEvidenceArtifact {
        schema: "elephant-fixture-evidence-v1",
        fixture: PublicationObservationsFixture {
            id: descriptor.id.clone(),
            sha256: descriptor.expected_sha256.clone(),
            blake3: descriptor.blake3.clone(),
            result_contract: descriptor.result_contract.clone(),
            policy_profile: descriptor.policy_profile.clone(),
            publication_origin: publication_origin.to_owned(),
        },
        runner: submission.runner,
        publication: submission.publication,
        documents: submission.documents,
    };
    let bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("could not serialize fixture evidence: {error}"))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let destination = app_data_dir.join("fixture-evidence.json");
    write_atomically(app_data_dir, &destination, &bytes, "fixture evidence")?;
    Ok(PersistedEvidence {
        sha256,
        bytes: bytes.len(),
        document_count,
    })
}

pub fn persist_fixture_terminal_event(app_data_dir: &FsPath, event: Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(&event)
        .map_err(|error| format!("could not serialize terminal fixture event: {error}"))?;
    let destination = app_data_dir.join("fixture-run-finished.json");
    write_atomically(app_data_dir, &destination, &bytes, "terminal fixture event")?;
    Ok(())
}

pub const PRIMARY_PROBE_SCENARIOS: &[&str] = &[
    "classic-script",
    "document-module-static-import",
    "document-module-dynamic-import",
    "relative-fetch",
    "root-relative-fetch",
    "remote-fetch",
    "dedicated-module-worker",
    "dedicated-worker-teardown",
    "shared-worker",
    "service-worker-registration",
    "stylesheet",
    "image",
    "scripted-svg",
    "wav-range",
    "local-storage",
    "indexed-db",
    "cookie",
    "document-domain-relaxation",
    "reader-isolation",
    "native-capability-isolation",
    "popup",
    "probe-complete",
];

pub const PEER_PROBE_SCENARIOS: &[&str] = &[
    "peer-local-storage",
    "peer-indexed-db",
    "peer-cookie",
    "peer-worker-teardown",
    "peer-complete",
];

pub fn persist_publication_observations(
    app_data_dir: &FsPath,
    descriptor: &RunnerFixtureDescriptor,
    publication_origin: &str,
    observations: Vec<PublicationObservation>,
) -> Result<PersistedPublicationObservations, String> {
    let phase_count = observations.len();
    let artifact = PublicationObservationsArtifact {
        schema: "elephant-publication-observations-v2",
        fixture: PublicationObservationsFixture {
            id: descriptor.id.clone(),
            sha256: descriptor.expected_sha256.clone(),
            blake3: descriptor.blake3.clone(),
            result_contract: descriptor.result_contract.clone(),
            policy_profile: descriptor.policy_profile.clone(),
            publication_origin: publication_origin.to_owned(),
        },
        phases: observations,
    };
    let bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("could not serialize publication observations: {error}"))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let destination = app_data_dir.join("fixture-publication-observations.json");
    write_atomically(
        app_data_dir,
        &destination,
        &bytes,
        "publication observations",
    )?;
    Ok(PersistedPublicationObservations {
        sha256,
        bytes: bytes.len(),
        phase_count,
    })
}
