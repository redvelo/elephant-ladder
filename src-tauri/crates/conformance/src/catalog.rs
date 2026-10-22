use std::path::{Path, PathBuf};

use include_dir::{Dir, include_dir};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) static RUNNER_READER_ASSETS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/../../../dist-fixture-runner");

pub static ACTIVE_PROBE_BYTES: &[u8] = include_bytes!(
    "../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-active-probe.epub"
);
pub static ACTIVE_PROBE_TWIN_BYTES: &[u8] = include_bytes!(
    "../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-active-probe-twin.epub"
);
pub static CONTAINER_CONSTRAINED_BYTES: &[u8] = include_bytes!(
    "../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-container-constrained.epub"
);
pub static REMOTE_POLICY_BYTES: &[u8] = include_bytes!(
    "../../../../../elephant-ladder/tests/fixtures/active-publications/elephant-remote-policy.epub"
);
pub static RUNNER_FIXTURE_NOTICE: &str =
    include_str!("../../../../../elephant-ladder/tests/fixtures/active-publications/NOTICE.md");

/// Active content enabled, remote resources blocked: what the runner page asks the
/// host's own policy routes for before it opens a fixture.
pub const RUNNER_POLICY_PROFILE: &str = "active-enabled-offline-v1";

pub struct RunnerCatalogEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub result_contract: &'static str,
    pub expected_total: u8,
    pub expected_sha256: &'static str,
    pub bytes: &'static [u8],
}

pub static RUNNER_CATALOG: &[RunnerCatalogEntry] = &[
    RunnerCatalogEntry {
        id: "static-fxl-ltr",
        label: "Static XHTML LTR Spreads",
        result_contract: "static-fxl-v1",
        expected_total: 8,
        expected_sha256: "7992cfb780207da629736cbc83045317e51eec0022772ca4740d239f04e5d86c",
        bytes: include_bytes!(
            "../../../../../elephant-ladder/tests/fixtures/active-publications/static-fxl-ltr.epub"
        ),
    },
    RunnerCatalogEntry {
        id: "static-fxl-rtl",
        label: "Static XHTML RTL Spreads",
        result_contract: "static-fxl-v1",
        expected_total: 8,
        expected_sha256: "03e6a0e4a282ab72c6a8dc9b769a795710e52c086ed8d9de63bb851b4ed67735",
        bytes: include_bytes!(
            "../../../../../elephant-ladder/tests/fixtures/active-publications/static-fxl-rtl.epub"
        ),
    },
    RunnerCatalogEntry {
        id: "static-fxl-mixed-svg",
        label: "Static XHTML and SVG Spreads",
        result_contract: "static-fxl-v1",
        expected_total: 8,
        expected_sha256: "818ea3b80955de4da3d9e48f893205c4dc2fb2883127bff35e63432fe293da4c",
        bytes: include_bytes!(
            "../../../../../elephant-ladder/tests/fixtures/active-publications/static-fxl-mixed-svg.epub"
        ),
    },
    RunnerCatalogEntry {
        id: "active-probe",
        label: "Elephant Active Probe",
        result_contract: "active-probe-v1",
        expected_total: 37,
        expected_sha256: "52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24",
        bytes: ACTIVE_PROBE_BYTES,
    },
    RunnerCatalogEntry {
        id: "active-probe-twin",
        label: "Elephant Active Probe Twin",
        result_contract: "active-probe-v1",
        expected_total: 37,
        expected_sha256: "fda27108e9bc17fcc32c223465541a66abc1219e4c4a0147d48de4422a1b6030",
        bytes: ACTIVE_PROBE_TWIN_BYTES,
    },
    RunnerCatalogEntry {
        id: "container-constrained",
        label: "Elephant Container-Constrained Probe",
        result_contract: "container-constrained-v1",
        expected_total: 7,
        expected_sha256: "2371f81ec4a599e04305f18b1bb85b089abb322bd9460e53046c6f7aeced5b34",
        bytes: CONTAINER_CONSTRAINED_BYTES,
    },
    RunnerCatalogEntry {
        id: "remote-policy-denied",
        label: "Elephant Remote Policy Denial Probe",
        result_contract: "remote-policy-denied-v1",
        expected_total: 9,
        expected_sha256: "e5120c80e46e8145d53bd182ed4c874986e0a7727aa1437201c40ca39acc6ef0",
        bytes: REMOTE_POLICY_BYTES,
    },
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerFixtureDescriptor {
    pub id: String,
    pub label: String,
    pub result_contract: String,
    pub expected_total: u8,
    pub expected_sha256: String,
    pub blake3: String,
    pub policy_profile: String,
}

/// Evidence runs record what the host and Navigator did with a publication and leave
/// the verdict to the collector: there is no check count to meet.
pub const EVIDENCE_RESULT_CONTRACT: &str = "evidence-v1";

/// The largest observation plan a run accepts. Navigator bounds the request itself;
/// this keeps a plan file from being anything but one.
pub const MAX_OBSERVATION_PLAN_BYTES: u64 = 64 * 1024;

pub enum RunnerFixtureSource {
    Embedded(&'static [u8]),
    File(PathBuf),
}

pub struct RunnerFixtureSelection {
    pub descriptor: RunnerFixtureDescriptor,
    pub source: RunnerFixtureSource,
    /// The document observation the runner page asks Navigator for in every document
    /// of an evidence run, as the plan file supplied it.
    pub observation: Option<serde_json::Value>,
    /// The reader window's logical inner size, when the run states one; the viewport
    /// a layout was observed in is an input to the evidence, not an accident of the
    /// display the window landed on.
    pub window: Option<(u32, u32)>,
    pub exit_after_run: bool,
}

impl RunnerFixtureSelection {
    pub fn parse_args<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut args = args.into_iter();
        let mut selected = None;
        let mut observation = None;
        let mut window = None;
        let mut exit_after_run = false;
        while let Some(argument) = args.next() {
            let argument = argument
                .as_ref()
                .to_str()
                .ok_or_else(|| "fixture-runner argument is not valid UTF-8".to_owned())?;
            match argument {
                "--fixture" => {
                    if selected.is_some() {
                        return Err("duplicate fixture selection".to_owned());
                    }
                    let id = args
                        .next()
                        .ok_or_else(|| "--fixture requires an id".to_owned())?;
                    let id = id
                        .as_ref()
                        .to_str()
                        .ok_or_else(|| "fixture id is not valid UTF-8".to_owned())?;
                    let entry = RUNNER_CATALOG
                        .iter()
                        .find(|entry| entry.id == id)
                        .ok_or_else(|| format!("unknown fixture id: {id}"))?;
                    selected = Some(Self::from_entry(entry));
                }
                "--fixture-epub" => {
                    if selected.is_some() {
                        return Err("duplicate fixture selection".to_owned());
                    }
                    let path = args
                        .next()
                        .ok_or_else(|| "--fixture-epub requires a path".to_owned())?;
                    selected = Some(Self::from_file(PathBuf::from(path.as_ref()))?);
                }
                "--observation" => {
                    if observation.is_some() {
                        return Err("duplicate --observation selection".to_owned());
                    }
                    let path = args
                        .next()
                        .ok_or_else(|| "--observation requires a path".to_owned())?;
                    observation = Some(read_observation_plan(Path::new(path.as_ref()))?);
                }
                "--window" => {
                    if window.is_some() {
                        return Err("duplicate --window selection".to_owned());
                    }
                    let size = args
                        .next()
                        .ok_or_else(|| "--window requires <width>x<height>".to_owned())?;
                    window = Some(parse_window(size.as_ref().to_str().unwrap_or_default())?);
                }
                "--exit-after-run" => {
                    if exit_after_run {
                        return Err("duplicate --exit-after-run selection".to_owned());
                    }
                    exit_after_run = true;
                }
                _ => return Err(format!("unexpected fixture-runner argument: {argument}")),
            }
        }
        let mut selected = selected.ok_or_else(|| {
            "missing required --fixture <id> or --fixture-epub <path> selection".to_owned()
        })?;
        if observation.is_some() && selected.descriptor.result_contract != EVIDENCE_RESULT_CONTRACT
        {
            return Err("--observation applies only to --fixture-epub runs".to_owned());
        }
        selected.observation = observation;
        selected.window = window;
        selected.exit_after_run = exit_after_run;
        Ok(selected)
    }

    pub fn exit_after_run(&self) -> bool {
        self.exit_after_run
    }

    pub fn from_entry(entry: &'static RunnerCatalogEntry) -> Self {
        Self {
            descriptor: RunnerFixtureDescriptor {
                id: entry.id.to_owned(),
                label: entry.label.to_owned(),
                result_contract: entry.result_contract.to_owned(),
                expected_total: entry.expected_total,
                expected_sha256: entry.expected_sha256.to_owned(),
                blake3: blake3::hash(entry.bytes).to_hex().to_string(),
                policy_profile: RUNNER_POLICY_PROFILE.to_owned(),
            },
            source: RunnerFixtureSource::Embedded(entry.bytes),
            observation: None,
            window: None,
            exit_after_run: false,
        }
    }

    /// A publication supplied as a file is identified by its file stem and by the
    /// digests of the bytes it holds when the run starts; the file itself is handed to
    /// the host where it is.
    pub fn from_file(path: PathBuf) -> Result<Self, String> {
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| valid_fixture_id(stem))
            .ok_or_else(|| {
                format!(
                    "fixture file name must be a lowercase identifier: {}",
                    path.display()
                )
            })?
            .to_owned();
        if path.extension().and_then(|extension| extension.to_str()) != Some("epub") {
            return Err(format!(
                "fixture file must end in .epub: {}",
                path.display()
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read fixture {}: {error}", path.display()))?;
        if bytes.is_empty() {
            return Err(format!("fixture file is empty: {}", path.display()));
        }
        Ok(Self {
            descriptor: RunnerFixtureDescriptor {
                label: id.clone(),
                id,
                result_contract: EVIDENCE_RESULT_CONTRACT.to_owned(),
                expected_total: 0,
                expected_sha256: format!("{:x}", Sha256::digest(&bytes)),
                blake3: blake3::hash(&bytes).to_hex().to_string(),
                policy_profile: RUNNER_POLICY_PROFILE.to_owned(),
            },
            source: RunnerFixtureSource::File(path),
            observation: None,
            window: None,
            exit_after_run: false,
        })
    }
}

pub fn valid_fixture_id(id: &str) -> bool {
    let mut characters = id.chars();
    (1..=80).contains(&id.len())
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
}

fn parse_window(value: &str) -> Result<(u32, u32), String> {
    let invalid = || format!("--window must be <width>x<height> in logical pixels: {value}");
    let (width, height) = value.split_once('x').ok_or_else(invalid)?;
    let width: u32 = width.parse().map_err(|_| invalid())?;
    let height: u32 = height.parse().map_err(|_| invalid())?;
    if !(320..=8192).contains(&width) || !(240..=8192).contains(&height) {
        return Err(invalid());
    }
    Ok((width, height))
}

fn read_observation_plan(path: &Path) -> Result<serde_json::Value, String> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        format!(
            "could not read observation plan {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || metadata.len() > MAX_OBSERVATION_PLAN_BYTES {
        return Err(format!(
            "observation plan must be a file of at most {MAX_OBSERVATION_PLAN_BYTES} bytes: {}",
            path.display()
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "could not read observation plan {}: {error}",
            path.display()
        )
    })?;
    let plan: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("observation plan {} is not JSON: {error}", path.display()))?;
    if !plan.is_object() {
        return Err(format!(
            "observation plan {} must be a JSON object",
            path.display()
        ));
    }
    Ok(plan)
}
