//! What a fixture run contributes, tested where the host and the runner meet.
//!
//! These cannot live in either crate's own test tree: they pass host types across
//! the contribution seam, and a dev-dependency cycle would give each crate its own
//! instance of the host. This crate already depends on both, so here the types are
//! the same ones.
#![cfg(feature = "fixture-runner")]

use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN};
use axum::http::{Request, StatusCode};
use elephant_ladder_conformance::{RunnerFixtureSelection, RunnerFixtureSource, contribute};
use elephant_ladder_native_host::{
    HostExtensions, LIBRARY_DOCUMENT_HEADER, OPEN_OPERATION_HEADER, PreparedHost, reader_router,
    test_reader_state,
};
use http_body_util::BodyExt as _;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;

const TEST_READER_ORIGIN: &str = "http://reader.test";
const TRUSTED_READER_HEADER: &str = "x-elephant-request";

fn trusted(request: axum::http::request::Builder) -> axum::http::request::Builder {
    request
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(TRUSTED_READER_HEADER, "trusted-reader")
        .header(LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
}

fn trusted_get(path: &str) -> Request<Body> {
    trusted(Request::builder().method("GET").uri(path))
        .body(Body::empty())
        .unwrap()
}

fn trusted_json(method: &str, path: &str, body: Value) -> Request<Body> {
    trusted(Request::builder().method(method).uri(path))
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn conformance_request(body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/conformance")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn import_request_with(bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/publications/import")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/epub+zip")
        .header(CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .unwrap()
}

fn open_request(publication_id: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/native/publications/{publication_id}/open"))
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(OPEN_OPERATION_HEADER, 1)
        .header(LIBRARY_DOCUMENT_HEADER, "b".repeat(64))
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

fn valid_active_probe_observations() -> Vec<elephant_ladder_conformance::PublicationObservation> {
    [
        (
            1,
            "EPUB/text/probe.xhtml",
            "probe.xhtml",
            elephant_ladder_conformance::PRIMARY_PROBE_SCENARIOS,
        ),
        (
            2,
            "EPUB/text/probe.xhtml",
            "probe.xhtml",
            elephant_ladder_conformance::PRIMARY_PROBE_SCENARIOS,
        ),
        (
            3,
            "EPUB/text/peer.xhtml",
            "peer.xhtml",
            elephant_ladder_conformance::PEER_PROBE_SCENARIOS,
        ),
    ]
    .into_iter()
    .map(|(generation, source, report_source, scenarios)| {
        serde_json::from_value(serde_json::json!({
            "generation": generation,
            "source": source,
            "reports": scenarios.iter().map(|scenario| serde_json::json!({
                "type": "elephant-active-probe-result-v1",
                "source": report_source,
                "scenario": scenario,
                "outcome": "passed",
                "evidence": {}
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    })
    .collect()
}

#[test]
fn runner_catalog_parses_only_one_allowlisted_fixture() {
    for (id, contract, total, sha256) in [
        (
            "static-fxl-ltr",
            "static-fxl-v1",
            8,
            "7992cfb780207da629736cbc83045317e51eec0022772ca4740d239f04e5d86c",
        ),
        (
            "static-fxl-rtl",
            "static-fxl-v1",
            8,
            "03e6a0e4a282ab72c6a8dc9b769a795710e52c086ed8d9de63bb851b4ed67735",
        ),
        (
            "static-fxl-mixed-svg",
            "static-fxl-v1",
            8,
            "818ea3b80955de4da3d9e48f893205c4dc2fb2883127bff35e63432fe293da4c",
        ),
        (
            "active-probe",
            "active-probe-v1",
            37,
            "52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24",
        ),
        (
            "active-probe-twin",
            "active-probe-v1",
            37,
            "fda27108e9bc17fcc32c223465541a66abc1219e4c4a0147d48de4422a1b6030",
        ),
        (
            "container-constrained",
            "container-constrained-v1",
            7,
            "2371f81ec4a599e04305f18b1bb85b089abb322bd9460e53046c6f7aeced5b34",
        ),
        (
            "remote-policy-denied",
            "remote-policy-denied-v1",
            9,
            "e5120c80e46e8145d53bd182ed4c874986e0a7727aa1437201c40ca39acc6ef0",
        ),
    ] {
        let selection = RunnerFixtureSelection::parse_args(["--fixture", id]).unwrap();
        assert_eq!(selection.descriptor.id, id);
        assert_eq!(selection.descriptor.result_contract, contract);
        assert_eq!(selection.descriptor.expected_total, total);
        assert_eq!(selection.descriptor.expected_sha256, sha256);
        let RunnerFixtureSource::Embedded(bytes) = selection.source else {
            panic!("{id} is an embedded fixture");
        };
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            sha256,
            "{id} bytes do not match the catalog digest"
        );
        assert!(selection.observation.is_none());
        assert_eq!(selection.descriptor.blake3.len(), 64);
        assert_eq!(
            selection.descriptor.policy_profile,
            elephant_ladder_conformance::RUNNER_POLICY_PROFILE
        );
        assert!(!selection.exit_after_run());
    }
    assert_eq!(elephant_ladder_conformance::RUNNER_CATALOG.len(), 7);

    assert!(RunnerFixtureSelection::parse_args([] as [&str; 0]).is_err());
    assert!(RunnerFixtureSelection::parse_args(["--fixture"]).is_err());
    assert!(RunnerFixtureSelection::parse_args(["--fixture", "unknown"]).is_err());
    assert!(RunnerFixtureSelection::parse_args(["--fixture", "w3c-scr-support"]).is_err());
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture",
            "active-probe",
            "--fixture",
            "active-probe-twin"
        ])
        .is_err()
    );
    assert!(RunnerFixtureSelection::parse_args(["--fixture=active-probe"]).is_err());
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture",
            "active-probe",
            "--publication-port",
            "1"
        ])
        .is_err()
    );
    let exiting =
        RunnerFixtureSelection::parse_args(["--fixture", "active-probe", "--exit-after-run"])
            .unwrap();
    assert!(exiting.exit_after_run());
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture",
            "active-probe",
            "--exit-after-run",
            "--exit-after-run"
        ])
        .is_err()
    );
}

#[test]
fn contribution_writes_the_fixture_file_and_adds_no_publication_plane() {
    for entry in elephant_ladder_conformance::RUNNER_CATALOG {
        let selection = RunnerFixtureSelection::parse_args(["--fixture", entry.id]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let extensions = contribute(selection, directory.path()).unwrap();
        assert!(extensions.reader_routes.is_some());
        assert!(extensions.reader_assets.is_some());
        let written = std::fs::read(
            directory
                .path()
                .join("fixture")
                .join(format!("{}.epub", entry.id)),
        )
        .unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&written)),
            entry.expected_sha256
        );

        let host = PreparedHost::bind(directory.path(), extensions).unwrap();
        assert!(
            host.storage.list_publications().unwrap().is_empty(),
            "fixture {} created a library row",
            entry.id
        );
    }
}

#[test]
fn product_bind_has_an_empty_library_and_no_contribution() {
    let directory = tempfile::tempdir().unwrap();
    let host = PreparedHost::bind(directory.path(), HostExtensions::default()).unwrap();
    assert!(host.extensions.reader_routes.is_none());
    assert!(host.extensions.reader_assets.is_none());
    assert!(host.storage.list_publications().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_route_describes_the_fixture_and_hands_it_over_once() {
    let directory = tempfile::tempdir().unwrap();
    let selection = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"]).unwrap();
    let expected_sha256 = selection.descriptor.expected_sha256.clone();
    let extensions = contribute(selection, directory.path()).unwrap();
    let state = test_reader_state(directory.path());
    let router = reader_router(state, extensions.reader_routes);

    let unauthorized = Request::builder()
        .method("GET")
        .uri("/native/conformance/fixture")
        .header("host", "reader.test")
        .header(ORIGIN, "http://attacker.test")
        .header(AUTHORIZATION, "Bearer secret")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(unauthorized).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let described = router
        .clone()
        .oneshot(trusted_get("/native/conformance/fixture"))
        .await
        .unwrap();
    assert_eq!(described.status(), StatusCode::OK);
    assert_eq!(described.headers()["cache-control"], "no-store");
    let described = json_body(described).await;
    assert_eq!(described["descriptor"]["id"], "active-probe");
    assert_eq!(described["descriptor"]["resultContract"], "active-probe-v1");
    assert_eq!(described["descriptor"]["expectedTotal"], 37);
    assert_eq!(described["descriptor"]["expectedSha256"], expected_sha256);
    assert_eq!(
        described["descriptor"]["policyProfile"],
        elephant_ladder_conformance::RUNNER_POLICY_PROFILE
    );
    assert!(
        described["notice"]
            .as_str()
            .unwrap()
            .contains("Active Publication Fixtures")
    );

    let pending = router
        .clone()
        .oneshot(trusted_get("/native/opened-publications/pending"))
        .await
        .unwrap();
    assert_eq!(pending.status(), StatusCode::OK);
    let pending = json_body(pending).await;
    assert_eq!(pending["fileName"], "active-probe.epub");
    assert_eq!(pending["intent"], "read");
    let open_id = pending["openId"].as_str().unwrap().to_owned();

    // Asking again describes the same fixture without offering a second copy.
    let again = router
        .clone()
        .oneshot(trusted_get("/native/conformance/fixture"))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    let bytes = router
        .clone()
        .oneshot(trusted_get(&format!(
            "/native/opened-publications/{open_id}/bytes"
        )))
        .await
        .unwrap();
    assert_eq!(bytes.status(), StatusCode::OK);
    let bytes = bytes.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected_sha256);
    // The file stays: the runner does not own it, so a second process can hand it over again.
    assert!(directory.path().join("fixture/active-probe.epub").is_file());
    let drained = router
        .oneshot(trusted_get(&format!(
            "/native/opened-publications/pending?after={open_id}"
        )))
        .await
        .unwrap();
    assert_eq!(drained.status(), StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "current_thread")]
async fn conformance_route_persists_evidence_for_the_open_publication() {
    let directory = tempfile::tempdir().unwrap();
    let selection = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"]).unwrap();
    let RunnerFixtureSource::Embedded(fixture_bytes) = selection.source else {
        panic!("active-probe is an embedded fixture");
    };
    let fixture_bytes = fixture_bytes.to_vec();
    let extensions = contribute(selection, directory.path()).unwrap();
    let state = test_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let router = reader_router(state, extensions.reader_routes);

    // Nothing is open yet, so there is no publication to bind evidence to.
    let unbound = router
        .clone()
        .oneshot(conformance_request(serde_json::json!({
            "passed": 37,
            "total": 37,
            "failures": [],
            "capture": null,
            "publicationObservations": valid_active_probe_observations()
        })))
        .await
        .unwrap();
    assert_eq!(unbound.status(), StatusCode::CONFLICT);

    // The runner page's library phase: policy, import, open.
    let stored = json_body(
        router
            .clone()
            .oneshot(trusted_get("/native/publication-policy"))
            .await
            .unwrap(),
    )
    .await;
    let saved = router
        .clone()
        .oneshot(trusted_json(
            "PUT",
            "/native/publication-policy",
            serde_json::json!({
                "baseRevision": stored["revisions"]["global"],
                "policy": { "activeContent": "automatic", "remoteResources": "blocked" }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let imported = router
        .clone()
        .oneshot(import_request_with(fixture_bytes))
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported = json_body(imported).await;
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let opened = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let session = lifecycle.active_view().load().unwrap();
    let publication_origin = session.authorities.foreground_origin.to_string();
    assert!(publication_origin.starts_with("http://p-"));

    let wrong_total = router
        .clone()
        .oneshot(conformance_request(serde_json::json!({
            "passed": 38,
            "total": 38,
            "failures": [],
            "capture": null,
            "publicationObservations": valid_active_probe_observations()
        })))
        .await
        .unwrap();
    assert_eq!(wrong_total.status(), StatusCode::BAD_REQUEST);

    let accepted = router
        .clone()
        .oneshot(conformance_request(serde_json::json!({
            "passed": 36,
            "total": 37,
            "failures": ["popup"],
            "capture": {
                "viewportWidth": 1100, "viewportHeight": 760,
                "x": 24, "y": 112, "width": 573, "height": 560
            },
            "publicationObservations": valid_active_probe_observations()
        })))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);

    let terminal: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-run-finished.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["event"], "fixture_run_finished");
    assert_eq!(terminal["run_status"], "complete");
    assert_eq!(terminal["fixture_id"], "active-probe");
    assert_eq!(terminal["publication_origin"], publication_origin);
    assert_eq!(terminal["passed"], 36);
    assert_eq!(terminal["failure_count"], 1);
    assert_eq!(terminal["failures"], "[\"popup\"]");
    assert_eq!(terminal["capture_width"], 573);
    assert_eq!(terminal["publication_observations_phase_count"], 3);
    let observations = std::fs::read(
        directory
            .path()
            .join("fixture-publication-observations.json"),
    )
    .unwrap();
    assert_eq!(
        terminal["publication_observations_sha256"],
        format!("{:x}", Sha256::digest(&observations))
    );
    let observations: Value = serde_json::from_slice(&observations).unwrap();
    assert_eq!(
        observations["schema"],
        "elephant-publication-observations-v2"
    );
    assert_eq!(
        observations["fixture"]["publicationOrigin"],
        publication_origin
    );
    assert_eq!(observations["phases"].as_array().unwrap().len(), 3);
    assert!(observations["phases"][0].get("snapshot").is_none());

    let incomplete = router
        .oneshot(conformance_request(serde_json::json!({
            "passed": 0,
            "total": 37,
            "failures": ["runner-execution"],
            "capture": null,
            "publicationObservations": [],
            "incompleteReason": "Navigator did not commit"
        })))
        .await
        .unwrap();
    assert_eq!(incomplete.status(), StatusCode::NO_CONTENT);
    let terminal: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-run-finished.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["run_status"], "incomplete");
    assert_eq!(terminal["publication_origin"], publication_origin);
    assert!(terminal.get("publication_observations_file").is_none());
}

#[test]
fn validates_and_persists_bound_publication_observations() {
    let descriptor = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"])
        .unwrap()
        .descriptor;
    let observations = valid_active_probe_observations();
    elephant_ladder_conformance::validate_publication_observations(&descriptor, &observations)
        .unwrap();

    let directory = tempfile::tempdir().unwrap();
    let persisted = elephant_ladder_conformance::persist_publication_observations(
        directory.path(),
        &descriptor,
        "http://p-fixture.localhost:43123",
        observations.clone(),
    )
    .unwrap();
    let bytes = std::fs::read(
        directory
            .path()
            .join("fixture-publication-observations.json"),
    )
    .unwrap();
    assert_eq!(persisted.bytes, bytes.len());
    assert_eq!(persisted.phase_count, 3);
    assert_eq!(persisted.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let artifact: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(artifact["schema"], "elephant-publication-observations-v2");
    assert_eq!(artifact["fixture"]["id"], "active-probe");
    assert_eq!(artifact["fixture"]["sha256"], descriptor.expected_sha256);
    assert_eq!(artifact["fixture"]["resultContract"], "active-probe-v1");
    assert_eq!(
        artifact["fixture"]["publicationOrigin"],
        "http://p-fixture.localhost:43123"
    );
    assert!(artifact["fixture"].get("label").is_none());
    assert_eq!(artifact["phases"].as_array().unwrap().len(), 3);
}

#[test]
fn persists_terminal_fixture_event_for_external_collection() {
    let descriptor = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"])
        .unwrap()
        .descriptor;
    let submission = elephant_ladder_conformance::ConformanceSubmission {
        passed: 37,
        total: 37,
        failures: Vec::new(),
        capture: Some(elephant_ladder_conformance::ConformanceCapture {
            viewport_width: 1100,
            viewport_height: 760,
            x: 24,
            y: 112,
            width: 573,
            height: 560,
        }),
        incomplete_reason: None,
        publication_observations: Some(valid_active_probe_observations()),
    };
    let artifact = elephant_ladder_conformance::PersistedPublicationObservations {
        sha256: "a".repeat(64),
        bytes: 4096,
        phase_count: 3,
    };
    let event = elephant_ladder_conformance::fixture_terminal_event(
        &descriptor,
        &submission,
        Some("http://p-fixture.localhost:43123"),
        Some(&artifact),
    );
    let directory = tempfile::tempdir().unwrap();
    elephant_ladder_conformance::persist_fixture_terminal_event(directory.path(), event.clone())
        .unwrap();
    let persisted: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-run-finished.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted, event);
    assert_eq!(persisted["event"], "fixture_run_finished");
    assert_eq!(
        persisted["publication_origin"],
        "http://p-fixture.localhost:43123"
    );
    assert_eq!(persisted["publication_observations_phase_count"], 3);
    assert_eq!(persisted["capture_width"], 573);
}

#[test]
fn rejects_missing_malformed_and_misbound_publication_observations() {
    let descriptor = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"])
        .unwrap()
        .descriptor;
    let validate = elephant_ladder_conformance::validate_publication_observations;

    let mut observations = valid_active_probe_observations();
    observations.pop();
    assert!(validate(&descriptor, &observations).is_err());

    let mut observations = valid_active_probe_observations();
    observations.swap(0, 1);
    assert!(validate(&descriptor, &observations).is_err());

    let mut observations = valid_active_probe_observations();
    observations[0].reports[0]["source"] = serde_json::json!("peer.xhtml");
    assert!(validate(&descriptor, &observations).is_err());

    let mut value = serde_json::to_value(&valid_active_probe_observations()[0]).unwrap();
    value["snapshot"] = serde_json::json!({
        "kind": "elephant-static-fxl-parent-v1",
        "documentGeneration": 1,
        "primary": {
            "occurrence": 0, "resource": "EPUB/center.xhtml", "adapter": "fixed-xhtml",
            "frameGeneration": 1, "layoutGeneration": 1,
            "rect": { "x": 0, "y": 0, "top": 0, "right": 1, "bottom": 1, "left": 0, "width": 1, "height": 1 }
        },
        "companion": null,
        "visible": { "primaryOccurrence": 0, "firstOccurrence": 0, "lastOccurrence": 0, "companion": null },
        "hostRect": { "x": 0, "y": 0, "top": 0, "right": 1, "bottom": 1, "left": 0, "width": 1, "height": 1 }
    });
    let with_snapshot: elephant_ladder_conformance::PublicationObservation =
        serde_json::from_value(value).unwrap();
    let mut observations = valid_active_probe_observations();
    observations[0] = with_snapshot;
    assert!(validate(&descriptor, &observations).is_err());

    let static_descriptor = RunnerFixtureSelection::parse_args(["--fixture", "static-fxl-ltr"])
        .unwrap()
        .descriptor;
    assert!(validate(&static_descriptor, &valid_active_probe_observations()[..2]).is_err());
}

fn evidence_request(body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/native/conformance/evidence")
        .header("host", "reader.test")
        .header(ORIGIN, TEST_READER_ORIGIN)
        .header(AUTHORIZATION, "Bearer secret")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn write_file_fixture(directory: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, elephant_ladder_conformance::ACTIVE_PROBE_BYTES).unwrap();
    path
}

#[test]
fn file_fixtures_are_identified_by_their_stem_and_digests() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_file_fixture(directory.path(), "scr-support_origin.epub");
    let plan = directory.path().join("plan.json");
    std::fs::write(&plan, r#"{"queries":[],"resources":true,"fonts":false}"#).unwrap();

    let selection = RunnerFixtureSelection::parse_args([
        "--fixture-epub",
        path.to_str().unwrap(),
        "--observation",
        plan.to_str().unwrap(),
        "--exit-after-run",
    ])
    .unwrap();
    assert_eq!(selection.descriptor.id, "scr-support_origin");
    assert_eq!(selection.descriptor.label, "scr-support_origin");
    assert_eq!(
        selection.descriptor.result_contract,
        elephant_ladder_conformance::EVIDENCE_RESULT_CONTRACT
    );
    assert_eq!(selection.descriptor.expected_total, 0);
    assert_eq!(
        selection.descriptor.expected_sha256,
        "52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24"
    );
    assert_eq!(selection.descriptor.blake3.len(), 64);
    assert!(matches!(selection.source, RunnerFixtureSource::File(ref file) if *file == path));
    assert_eq!(
        selection.observation,
        Some(serde_json::json!({"queries":[],"resources":true,"fonts":false}))
    );
    assert_eq!(selection.window, None);
    assert!(selection.exit_after_run());
    let sized = RunnerFixtureSelection::parse_args([
        "--fixture-epub",
        path.to_str().unwrap(),
        "--window",
        "1400x900",
    ])
    .unwrap();
    assert_eq!(sized.window, Some((1400, 900)));
    for size in ["1400", "0x900", "1400x100", "9000x900", "1400x900x1"] {
        assert!(
            RunnerFixtureSelection::parse_args(["--fixture", "active-probe", "--window", size])
                .is_err(),
            "{size} was accepted"
        );
    }

    for (name, contents) in [
        ("Scr-Support.epub", &b"x"[..]),
        ("scr support.epub", &b"x"[..]),
        ("scr-support.zip", &b"x"[..]),
        ("empty.epub", &b""[..]),
    ] {
        let path = directory.path().join(name);
        std::fs::write(&path, contents).unwrap();
        assert!(
            RunnerFixtureSelection::parse_args(["--fixture-epub", path.to_str().unwrap()]).is_err(),
            "{name} was accepted"
        );
    }
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture-epub",
            directory.path().join("missing.epub").to_str().unwrap()
        ])
        .is_err()
    );
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture",
            "active-probe",
            "--fixture-epub",
            path.to_str().unwrap()
        ])
        .is_err()
    );
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture",
            "active-probe",
            "--observation",
            plan.to_str().unwrap()
        ])
        .is_err(),
        "embedded fixtures take no observation plan"
    );
    std::fs::write(&plan, "[]").unwrap();
    assert!(
        RunnerFixtureSelection::parse_args([
            "--fixture-epub",
            path.to_str().unwrap(),
            "--observation",
            plan.to_str().unwrap()
        ])
        .is_err(),
        "a plan must be an object"
    );
    assert!(elephant_ladder_conformance::valid_fixture_id(
        "lay-pp-svg-icb_multi"
    ));
    assert!(!elephant_ladder_conformance::valid_fixture_id("-x"));
    assert!(!elephant_ladder_conformance::valid_fixture_id(
        &"a".repeat(81)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn evidence_route_binds_and_persists_what_a_file_fixture_run_observed() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = write_file_fixture(directory.path(), "scr-support.epub");
    let plan = directory.path().join("plan.json");
    std::fs::write(
        &plan,
        r#"{"queries":[{"id":"body","selector":"body","text":true}]}"#,
    )
    .unwrap();
    let selection = RunnerFixtureSelection::parse_args([
        "--fixture-epub",
        fixture.to_str().unwrap(),
        "--observation",
        plan.to_str().unwrap(),
    ])
    .unwrap();
    let expected_sha256 = selection.descriptor.expected_sha256.clone();
    let extensions = contribute(selection, directory.path()).unwrap();
    let state = test_reader_state(directory.path());
    let lifecycle = state.lifecycle.clone();
    let router = reader_router(state, extensions.reader_routes);

    // Nothing is copied: the file is handed over where it is.
    assert!(!directory.path().join("fixture").exists());
    let described = json_body(
        router
            .clone()
            .oneshot(trusted_get("/native/conformance/fixture"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(described["descriptor"]["id"], "scr-support");
    assert_eq!(described["descriptor"]["resultContract"], "evidence-v1");
    assert_eq!(described["descriptor"]["expectedTotal"], 0);
    assert_eq!(
        described["observation"],
        serde_json::json!({"queries":[{"id":"body","selector":"body","text":true}]})
    );
    assert!(
        described["notice"]
            .as_str()
            .unwrap()
            .contains("scr-support.epub")
    );
    let pending = json_body(
        router
            .clone()
            .oneshot(trusted_get("/native/opened-publications/pending"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(pending["fileName"], "scr-support.epub");
    let open_id = pending["openId"].as_str().unwrap();
    let bytes = router
        .clone()
        .oneshot(trusted_get(&format!(
            "/native/opened-publications/{open_id}/bytes"
        )))
        .await
        .unwrap()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), expected_sha256);

    // The check route is not this fixture's route.
    let checks = router
        .clone()
        .oneshot(conformance_request(serde_json::json!({
            "passed": 0, "total": 0, "failures": [], "capture": null,
            "publicationObservations": []
        })))
        .await
        .unwrap();
    assert_eq!(checks.status(), StatusCode::BAD_REQUEST);

    let evidence = serde_json::json!({
        "runner": { "viewport": { "width": 1100, "height": 760 } },
        "publication": { "readingOrderLength": 1 },
        "documents": [{ "occurrence": 0, "resource": "EPUB/content_001.xhtml", "settled": true }]
    });
    let unbound = router
        .clone()
        .oneshot(evidence_request(evidence.clone()))
        .await
        .unwrap();
    assert_eq!(unbound.status(), StatusCode::CONFLICT);

    let imported = json_body(
        router
            .clone()
            .oneshot(import_request_with(bytes.to_vec()))
            .await
            .unwrap(),
    )
    .await;
    let publication_id = imported["publication"]["publicationId"].as_str().unwrap();
    let opened = router
        .clone()
        .oneshot(open_request(publication_id))
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::OK);
    let publication_origin = lifecycle
        .active_view()
        .load()
        .unwrap()
        .authorities
        .foreground_origin
        .to_string();

    for malformed in [
        serde_json::json!({ "runner": {}, "publication": {}, "documents": [{ "occurrence": 1 }] }),
        serde_json::json!({ "runner": [], "publication": {}, "documents": [] }),
        serde_json::json!({ "runner": {}, "publication": {}, "documents": [], "extra": 1 }),
        serde_json::json!({
            "runner": {}, "publication": {},
            "documents": (0..65).map(|index| serde_json::json!({ "occurrence": index })).collect::<Vec<_>>()
        }),
        serde_json::json!({
            "runner": {}, "publication": {}, "documents": [{ "occurrence": 0 }],
            "incompleteReason": "stopped"
        }),
    ] {
        let rejected = router
            .clone()
            .oneshot(evidence_request(malformed))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    }

    let accepted = router
        .clone()
        .oneshot(evidence_request(evidence))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::NO_CONTENT);
    let artifact: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-evidence.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact["schema"], "elephant-fixture-evidence-v1");
    assert_eq!(artifact["fixture"]["id"], "scr-support");
    assert_eq!(artifact["fixture"]["sha256"], expected_sha256);
    assert_eq!(artifact["fixture"]["resultContract"], "evidence-v1");
    assert_eq!(artifact["fixture"]["publicationOrigin"], publication_origin);
    assert_eq!(artifact["runner"]["viewport"]["width"], 1100);
    assert_eq!(
        artifact["documents"][0]["resource"],
        "EPUB/content_001.xhtml"
    );
    let terminal: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-run-finished.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["event"], "fixture_run_finished");
    assert_eq!(terminal["run_status"], "complete");
    assert_eq!(terminal["result_contract"], "evidence-v1");
    assert_eq!(terminal["publication_origin"], publication_origin);
    assert_eq!(terminal["evidence_file"], "fixture-evidence.json");
    assert_eq!(terminal["evidence_documents"], 1);
    assert!(terminal.get("passed").is_none());
    let evidence_bytes = std::fs::read(directory.path().join("fixture-evidence.json")).unwrap();
    assert_eq!(
        terminal["evidence_sha256"],
        format!("{:x}", Sha256::digest(&evidence_bytes))
    );
    assert_eq!(terminal["evidence_bytes"], evidence_bytes.len());

    let incomplete = router
        .oneshot(evidence_request(serde_json::json!({
            "runner": {}, "publication": {}, "documents": [],
            "incompleteReason": "Navigator did not commit"
        })))
        .await
        .unwrap();
    assert_eq!(incomplete.status(), StatusCode::NO_CONTENT);
    let terminal: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("fixture-run-finished.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["run_status"], "incomplete");
    assert_eq!(terminal["incomplete_reason"], "Navigator did not commit");
    assert!(terminal.get("evidence_file").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn evidence_route_is_not_an_embedded_fixture_route() {
    let directory = tempfile::tempdir().unwrap();
    let selection = RunnerFixtureSelection::parse_args(["--fixture", "active-probe"]).unwrap();
    let extensions = contribute(selection, directory.path()).unwrap();
    let router = reader_router(
        test_reader_state(directory.path()),
        extensions.reader_routes,
    );
    let refused = router
        .oneshot(evidence_request(serde_json::json!({
            "runner": {}, "publication": {}, "documents": []
        })))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
}
