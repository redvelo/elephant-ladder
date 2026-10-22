use crate::*;
use serde_json::Value;
use std::collections::HashSet;

pub fn validate_publication_observations(
    descriptor: &RunnerFixtureDescriptor,
    observations: &[PublicationObservation],
) -> Result<(), &'static str> {
    if observations.len() > 8 {
        return Err("too many publication observation phases");
    }
    let expected: &[(u8, &str)] = match descriptor.result_contract.as_str() {
        "active-probe-v1" => &[
            (1, "EPUB/text/probe.xhtml"),
            (2, "EPUB/text/probe.xhtml"),
            (3, "EPUB/text/peer.xhtml"),
        ],
        "container-constrained-v1" => &[(1, "EPUB/content.xhtml")],
        "remote-policy-denied-v1" => &[(1, "EPUB/text/probe.xhtml")],
        "static-fxl-v1" => &[(1, "EPUB/center.xhtml"), (2, "EPUB/first.xhtml")],
        _ => return Err("unknown fixture result contract"),
    };
    if observations.len() != expected.len() {
        return Err("publication observation phase count does not match contract");
    }
    for (observation, (generation, source)) in observations.iter().zip(expected) {
        if observation.generation != *generation || observation.source != *source {
            return Err("publication observation phase does not match contract");
        }
        if descriptor.result_contract == "static-fxl-v1" {
            let Some(snapshot) = &observation.snapshot else {
                return Err("static FXL requires a parent snapshot");
            };
            if !observation.reports.is_empty() || snapshot.kind != "elephant-static-fxl-parent-v1" {
                return Err("static FXL must not carry authored reports");
            }
            if [
                snapshot.visible.primary_occurrence,
                snapshot.visible.first_occurrence,
                snapshot.visible.last_occurrence,
            ]
            .into_iter()
            .any(|ordinal| ordinal > 65535)
                || snapshot
                    .visible
                    .companion
                    .is_some_and(|companion| companion.occurrence > 65535)
            {
                return Err("static FXL visible range exceeds bounds");
            }
            let valid_participant = |resource: &str, adapter: &str, rect: &PublicationRect| {
                matches!(
                    resource,
                    "EPUB/center.xhtml"
                        | "EPUB/first.xhtml"
                        | "EPUB/second.xhtml"
                        | "EPUB/second.svg"
                ) && matches!(adapter, "fixed-xhtml" | "fixed-svg")
                    && validate_rect(rect).is_ok()
            };
            if !valid_participant(
                &snapshot.primary.resource,
                &snapshot.primary.adapter,
                &snapshot.primary.rect,
            ) || snapshot.companion.as_ref().is_some_and(|companion| {
                !valid_participant(&companion.resource, &companion.adapter, &companion.rect)
            }) {
                return Err("invalid static FXL participant");
            }
            validate_rect(&snapshot.host_rect)?;
            continue;
        }
        if observation.snapshot.is_some() {
            return Err("authored report phases carry no parent snapshot");
        }
        match descriptor.result_contract.as_str() {
            "active-probe-v1" => validate_active_probe_reports(
                &observation.reports,
                if *source == "EPUB/text/probe.xhtml" {
                    "probe.xhtml"
                } else {
                    "peer.xhtml"
                },
            )?,
            "container-constrained-v1" => {
                validate_container_constrained_reports(&observation.reports)?
            }
            "remote-policy-denied-v1" => validate_remote_policy_reports(&observation.reports)?,
            _ => return Err("unknown fixture result contract"),
        }
    }
    Ok(())
}

pub fn validate_remote_policy_reports(reports: &[Value]) -> Result<(), &'static str> {
    if reports.len() != 1 {
        return Err("remote policy report count does not match contract");
    }
    let report: RemotePolicyReport = serde_json::from_value(reports[0].clone())
        .map_err(|_| "remote policy report is invalid")?;
    if report.report_type != "elephant-remote-policy-result-v1"
        || report.source != "EPUB/text/probe.xhtml"
    {
        return Err("remote policy report does not match contract");
    }
    let allowed = [
        "blocked:connect-src",
        "blocked:img-src",
        "blocked:style-src",
        "blocked:style-src-elem",
        "blocked:script-src",
        "blocked:script-src-elem",
        "blocked:media-src",
        "blocked:frame-src",
        "loaded",
        "failed",
        "indeterminate",
    ];
    if [
        report.https_fetch,
        report.https_image,
        report.https_style,
        report.https_script,
        report.https_audio,
        report.https_frame,
        report.wss_echo,
        report.http_fetch,
        report.ws_echo,
    ]
    .iter()
    .any(|outcome| !allowed.contains(&outcome.as_str()))
    {
        return Err("remote policy report contains an invalid outcome");
    }
    Ok(())
}

pub fn validate_container_constrained_reports(reports: &[Value]) -> Result<(), &'static str> {
    if reports.len() != 1 {
        return Err("container-constrained report count does not match contract");
    }
    let report: ContainerConstrainedReport = serde_json::from_value(reports[0].clone())
        .map_err(|_| "container-constrained report is invalid")?;
    if report.report_type != "elephant-container-constrained-result-v1"
        || report.source != "EPUB/content.xhtml"
        || !report.child_script
        || !report.epub_reading_system
        || !report.parent_dom_blocked
        || !report.frame_size_blocked
        || !report.reader_dom_blocked
        || !report.credentials_blocked
        || !report.native_route_blocked
    {
        return Err("container-constrained report did not prove its boundary");
    }
    Ok(())
}

pub fn validate_rect(rect: &PublicationRect) -> Result<(), &'static str> {
    for value in [
        rect.x,
        rect.y,
        rect.top,
        rect.right,
        rect.bottom,
        rect.left,
        rect.width,
        rect.height,
    ] {
        if !value.is_finite() || value.abs() > 32_768.0 {
            return Err("snapshot rectangle is invalid");
        }
    }
    Ok(())
}

pub fn validate_active_probe_reports(reports: &[Value], source: &str) -> Result<(), &'static str> {
    let expected = if source == "probe.xhtml" {
        PRIMARY_PROBE_SCENARIOS
    } else {
        PEER_PROBE_SCENARIOS
    };
    if reports.len() != expected.len() {
        return Err("active probe report count does not match contract");
    }
    let mut scenarios = HashSet::with_capacity(reports.len());
    for value in reports {
        let report: ActiveProbeReport =
            serde_json::from_value(value.clone()).map_err(|_| "active probe report is invalid")?;
        if report.report_type != "elephant-active-probe-result-v1"
            || report.source != source
            || !expected.contains(&report.scenario.as_str())
            || !matches!(
                report.outcome.as_str(),
                "passed" | "failed" | "blocked" | "unsupported"
            )
            || !scenarios.insert(report.scenario)
        {
            return Err("active probe report does not match phase");
        }
        validate_probe_evidence(&report.evidence)?;
    }
    if scenarios.len() != expected.len() {
        return Err("active probe scenarios are incomplete");
    }
    Ok(())
}

pub fn validate_probe_evidence(
    evidence: &serde_json::Map<String, Value>,
) -> Result<(), &'static str> {
    if evidence.len() > 16 {
        return Err("active probe evidence has too many fields");
    }
    for (key, value) in evidence {
        let mut bytes = key.bytes();
        if key.len() > 64
            || !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            || !bytes.all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err("active probe evidence key is invalid");
        }
        if let Value::Array(values) = value {
            if values.len() > 16 {
                return Err("active probe evidence array is too long");
            }
            for value in values {
                validate_probe_evidence_value(value)?;
            }
        } else {
            validate_probe_evidence_value(value)?;
        }
    }
    Ok(())
}

pub fn validate_probe_evidence_value(value: &Value) -> Result<(), &'static str> {
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Number(number)
            if number
                .as_f64()
                .is_some_and(|value| value.is_finite() && value.abs() <= 1_000_000_000.0) =>
        {
            Ok(())
        }
        Value::String(value) if value.len() <= 256 => Ok(()),
        _ => Err("active probe evidence value is invalid"),
    }
}

pub const MAX_EVIDENCE_DOCUMENTS: usize = 64;

pub fn validate_evidence_submission(submission: &EvidenceSubmission) -> Result<(), &'static str> {
    if let Some(reason) = submission.incomplete_reason.as_deref() {
        if reason.is_empty() || reason.len() > 1000 {
            return Err("incomplete reason is out of bounds");
        }
        if !submission.documents.is_empty() {
            return Err("an incomplete run carries no documents");
        }
        return Ok(());
    }
    if !submission.runner.is_object() || !submission.publication.is_object() {
        return Err("runner and publication evidence must be objects");
    }
    if submission.documents.len() > MAX_EVIDENCE_DOCUMENTS {
        return Err("too many evidence documents");
    }
    for (index, document) in submission.documents.iter().enumerate() {
        let Some(fields) = document.as_object() else {
            return Err("evidence document is not an object");
        };
        if fields.get("occurrence").and_then(Value::as_u64) != Some(index as u64) {
            return Err("evidence documents must be in reading order");
        }
    }
    Ok(())
}
