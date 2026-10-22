export const RUNNER_EVENT = "fixture_run_finished";
export const EVIDENCE_RESULT_CONTRACT = "evidence-v1";
export const EVIDENCE_SCHEMA = "elephant-fixture-evidence-v1";
export const RUNNER_POLICY_PROFILE = "active-enabled-offline-v1";
const MAX_EVIDENCE_BYTES = 8 * 1_048_576;
const MAX_EVIDENCE_DOCUMENTS = 64;

const FIXTURE_CATALOG = Object.freeze({
  "static-fxl-ltr": Object.freeze({
    blake3: "2deec1b53dc04bffa8f6a9b97cc06a7754f0a502258bb6997a29fb3e590263d0",
    sha256: "7992cfb780207da629736cbc83045317e51eec0022772ca4740d239f04e5d86c",
    resultContract: "static-fxl-v1",
    expectedTotal: 8,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 2,
  }),
  "static-fxl-rtl": Object.freeze({
    blake3: "2185a2ce98eedab16e02ba7f56ecf52ee77d0f1be22039efe64af324a3c57211",
    sha256: "03e6a0e4a282ab72c6a8dc9b769a795710e52c086ed8d9de63bb851b4ed67735",
    resultContract: "static-fxl-v1",
    expectedTotal: 8,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 2,
  }),
  "static-fxl-mixed-svg": Object.freeze({
    blake3: "42c9a9d713ab874f28d49c4636380c7616496d821771422a8492bf32cec2833d",
    sha256: "818ea3b80955de4da3d9e48f893205c4dc2fb2883127bff35e63432fe293da4c",
    resultContract: "static-fxl-v1",
    expectedTotal: 8,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 2,
  }),
  "active-probe": Object.freeze({
    blake3: "0302f52dedeb27cf981b51f885805d671bae7160bc0207efb9174739673803dc",
    sha256: "52882e792e632bd51590b1e50421b78d9dc3ae3ce8c367852e18c2606570ab24",
    resultContract: "active-probe-v1",
    expectedTotal: 37,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 3,
  }),
  "active-probe-twin": Object.freeze({
    blake3: "7f2725c43dea19c91ad810aa783fba3587d5c998d58e209a20ed283e26ae72d2",
    sha256: "fda27108e9bc17fcc32c223465541a66abc1219e4c4a0147d48de4422a1b6030",
    resultContract: "active-probe-v1",
    expectedTotal: 37,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 3,
  }),
  "container-constrained": Object.freeze({
    blake3: "a35db5bbab8b84515c60a17c8aeb0a75eb96f15d7ecd10d1173f36bd1ae65a62",
    sha256: "2371f81ec4a599e04305f18b1bb85b089abb322bd9460e53046c6f7aeced5b34",
    resultContract: "container-constrained-v1",
    expectedTotal: 7,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 1,
  }),
  "remote-policy-denied": Object.freeze({
    blake3: "3139a4a9cdffe9204f670e8e27a817ac6e30ad275b0d917058d0bbd7891eee24",
    sha256: "e5120c80e46e8145d53bd182ed4c874986e0a7727aa1437201c40ca39acc6ef0",
    resultContract: "remote-policy-denied-v1",
    expectedTotal: 9,
    policyProfile: "active-enabled-offline-v1",
    observationPhaseCount: 1,
  }),
});

/// Every fixture in the catalog, which is also what an empty selection runs.
export const FIXTURE_IDS = Object.freeze(Object.keys(FIXTURE_CATALOG));

export function parseFixtureIds(args) {
  const fixtureArgs = args[0] === "--" ? args.slice(1) : args;
  const ids = fixtureArgs.length === 0 ? FIXTURE_IDS : fixtureArgs;
  const seen = new Set();
  for (const id of ids) {
    if (typeof id !== "string" || !Object.hasOwn(FIXTURE_CATALOG, id)) {
      throw new Error(
        `Unknown fixture id: ${id}; allowed: ${Object.keys(FIXTURE_CATALOG).join(", ")}`,
      );
    }
    if (seen.has(id)) throw new Error(`Duplicate fixture id: ${id}`);
    seen.add(id);
  }
  return [...ids];
}

function parseRunnerEvent(line) {
  let value;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (value?.event !== RUNNER_EVENT) return null;
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Malformed terminal fixture event");

  const identityFields = [
    "fixture_id",
    "fixture_blake3",
    "fixture_sha256",
    "result_contract",
    "policy_profile",
    "run_status",
  ];
  if (identityFields.some((field) => typeof value[field] !== "string"))
    throw new Error("Terminal fixture identity is incomplete");
  if (value.run_status !== "complete" && value.run_status !== "incomplete")
    throw new Error("Terminal fixture status is malformed");
  if (
    (value.run_status === "complete" && !validOrigin(value.publication_origin)) ||
    (value.publication_origin !== undefined && !validOrigin(value.publication_origin))
  )
    throw new Error("Terminal publication origin is malformed");
  if (
    !/^[a-f0-9]{64}$/.test(value.fixture_blake3) ||
    !/^[a-f0-9]{64}$/.test(value.fixture_sha256)
  ) {
    throw new Error("Terminal fixture digest is malformed");
  }
  const identity = {
    fixtureId: value.fixture_id,
    runStatus: value.run_status,
    publicationOrigin: value.publication_origin ?? null,
    fixtureBlake3: value.fixture_blake3,
    fixtureSha256: value.fixture_sha256,
    resultContract: value.result_contract,
    policyProfile: value.policy_profile,
  };
  if (value.result_contract === EVIDENCE_RESULT_CONTRACT)
    return parseEvidenceEvent(value, identity);
  if (value.incomplete_reason !== undefined)
    throw new Error("Terminal check results carry no incomplete reason");
  const hasCompleteObservations =
    value.publication_observations_file === "fixture-publication-observations.json" &&
    /^[a-f0-9]{64}$/.test(value.publication_observations_sha256) &&
    Number.isInteger(value.publication_observations_bytes) &&
    value.publication_observations_bytes >= 1 &&
    value.publication_observations_bytes <= 1_048_576 &&
    Number.isInteger(value.publication_observations_phase_count) &&
    value.publication_observations_phase_count >= 1 &&
    value.publication_observations_phase_count <= 8;
  const hasNoObservations = [
    value.publication_observations_file,
    value.publication_observations_sha256,
    value.publication_observations_bytes,
    value.publication_observations_phase_count,
  ].every((field) => field === undefined);
  if (
    (value.run_status === "complete" && !hasCompleteObservations) ||
    (value.run_status === "incomplete" && !hasNoObservations)
  ) {
    throw new Error("Terminal publication observations metadata is malformed");
  }
  if (
    !Number.isInteger(value.passed) ||
    value.passed < 0 ||
    !Number.isInteger(value.total) ||
    value.total < 1 ||
    value.total > 64 ||
    !Number.isInteger(value.failure_count) ||
    value.failure_count < 0 ||
    (value.run_status === "complete"
      ? value.passed + value.failure_count !== value.total
      : value.passed !== 0 ||
        value.failure_count !== 1 ||
        value.failures !== '["runner-execution"]') ||
    typeof value.failures !== "string"
  )
    throw new Error("Terminal fixture counts are malformed");

  let failures;
  try {
    failures = JSON.parse(value.failures);
  } catch {
    throw new Error("Terminal fixture failures are malformed");
  }
  if (
    !Array.isArray(failures) ||
    failures.length !== value.failure_count ||
    failures.some(
      (failure) => typeof failure !== "string" || failure.length < 1 || failure.length > 160,
    )
  ) {
    throw new Error("Terminal fixture failures do not match its counts");
  }

  const captureFields = [
    value.viewport_width,
    value.viewport_height,
    value.capture_x,
    value.capture_y,
    value.capture_width,
    value.capture_height,
  ];
  let capture = null;
  if (!captureFields.every((field) => field === undefined)) {
    capture = {
      viewportWidth: value.viewport_width,
      viewportHeight: value.viewport_height,
      x: value.capture_x,
      y: value.capture_y,
      width: value.capture_width,
      height: value.capture_height,
    };
    if (!validCapture(capture)) throw new Error("Terminal fixture capture geometry is malformed");
  }

  return {
    ...identity,
    passed: value.passed,
    total: value.total,
    failureCount: value.failure_count,
    failures,
    capture,
    publicationObservations:
      value.run_status === "complete"
        ? {
            file: value.publication_observations_file,
            sha256: value.publication_observations_sha256,
            bytes: value.publication_observations_bytes,
            phaseCount: value.publication_observations_phase_count,
          }
        : null,
  };
}

/// An evidence run ends the same way a check run does, but it counts nothing: the
/// terminal event names the evidence artifact, or the reason there is none.
function parseEvidenceEvent(value, identity) {
  const countFields = [
    value.passed,
    value.total,
    value.failure_count,
    value.failures,
    value.viewport_width,
    value.publication_observations_file,
  ];
  if (!countFields.every((field) => field === undefined))
    throw new Error("Terminal evidence event carries check fields");
  const evidenceFields = [
    value.evidence_file,
    value.evidence_sha256,
    value.evidence_bytes,
    value.evidence_documents,
  ];
  if (value.run_status === "incomplete") {
    if (
      typeof value.incomplete_reason !== "string" ||
      value.incomplete_reason.length < 1 ||
      value.incomplete_reason.length > 1000 ||
      !evidenceFields.every((field) => field === undefined)
    )
      throw new Error("Terminal incomplete evidence event is malformed");
    return { ...identity, incompleteReason: value.incomplete_reason, evidence: null };
  }
  if (
    value.incomplete_reason !== undefined ||
    value.evidence_file !== "fixture-evidence.json" ||
    !/^[a-f0-9]{64}$/.test(value.evidence_sha256) ||
    !Number.isInteger(value.evidence_bytes) ||
    value.evidence_bytes < 1 ||
    value.evidence_bytes > MAX_EVIDENCE_BYTES ||
    !Number.isInteger(value.evidence_documents) ||
    value.evidence_documents < 0 ||
    value.evidence_documents > MAX_EVIDENCE_DOCUMENTS
  )
    throw new Error("Terminal evidence metadata is malformed");
  return {
    ...identity,
    incompleteReason: null,
    evidence: {
      file: value.evidence_file,
      sha256: value.evidence_sha256,
      bytes: value.evidence_bytes,
      documents: value.evidence_documents,
    },
  };
}

/// Identity for a publication the catalog does not embed: what the collector knows
/// about the file it started the run with.
export function evidenceFixtureIdentity(sha256) {
  if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error("Evidence fixture SHA-256 is malformed");
  return Object.freeze({
    sha256,
    resultContract: EVIDENCE_RESULT_CONTRACT,
    policyProfile: RUNNER_POLICY_PROFILE,
  });
}

function validateRunnerIdentity(event, requestedId, expected) {
  if (expected === undefined) throw new Error(`Unknown fixture id: ${requestedId}`);
  const mismatches = [];
  if (event.fixtureId !== requestedId) mismatches.push(`id ${JSON.stringify(event.fixtureId)}`);
  if (expected.blake3 !== undefined && event.fixtureBlake3 !== expected.blake3)
    mismatches.push(`BLAKE3 ${JSON.stringify(event.fixtureBlake3)}`);
  if (event.fixtureSha256 !== expected.sha256)
    mismatches.push(`SHA-256 ${JSON.stringify(event.fixtureSha256)}`);
  if (event.resultContract !== expected.resultContract)
    mismatches.push(`result contract ${JSON.stringify(event.resultContract)}`);
  if (event.policyProfile !== expected.policyProfile)
    mismatches.push(`policy profile ${JSON.stringify(event.policyProfile)}`);
  if (expected.resultContract !== EVIDENCE_RESULT_CONTRACT) {
    if (event.total !== expected.expectedTotal) mismatches.push(`total ${event.total}`);
    if (
      event.runStatus === "complete" &&
      event.publicationObservations.phaseCount !== expected.observationPhaseCount
    )
      mismatches.push(`observation phase count ${event.publicationObservations.phaseCount}`);
  }
  if (mismatches.length > 0)
    throw new Error(`Terminal fixture identity mismatch: ${mismatches.join(", ")}`);
  return event;
}

export function validateFixtureEvidenceArtifact(value, terminal) {
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).length !== 5 ||
    value.schema !== EVIDENCE_SCHEMA ||
    typeof value.fixture !== "object" ||
    value.fixture === null ||
    Array.isArray(value.fixture) ||
    Object.keys(value.fixture).length !== 6 ||
    value.fixture.id !== terminal.fixtureId ||
    value.fixture.sha256 !== terminal.fixtureSha256 ||
    value.fixture.blake3 !== terminal.fixtureBlake3 ||
    value.fixture.resultContract !== terminal.resultContract ||
    value.fixture.policyProfile !== terminal.policyProfile ||
    value.fixture.publicationOrigin !== terminal.publicationOrigin ||
    typeof value.runner !== "object" ||
    value.runner === null ||
    typeof value.publication !== "object" ||
    value.publication === null ||
    !Array.isArray(value.documents) ||
    value.documents.length !== terminal.evidence.documents ||
    value.documents.some((document, index) => document?.occurrence !== index)
  ) {
    throw new Error("Fixture evidence artifact does not match the terminal fixture identity");
  }
  return value;
}

export function validatePublicationObservationsArtifact(value, terminal) {
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).length !== 3 ||
    value.schema !== "elephant-publication-observations-v2" ||
    typeof value.fixture !== "object" ||
    value.fixture === null ||
    Array.isArray(value.fixture) ||
    Object.keys(value.fixture).length !== 6 ||
    value.fixture.id !== terminal.fixtureId ||
    value.fixture.sha256 !== terminal.fixtureSha256 ||
    value.fixture.blake3 !== terminal.fixtureBlake3 ||
    value.fixture.resultContract !== terminal.resultContract ||
    value.fixture.policyProfile !== terminal.policyProfile ||
    value.fixture.publicationOrigin !== terminal.publicationOrigin ||
    !Array.isArray(value.phases) ||
    value.phases.length !== terminal.publicationObservations.phaseCount
  ) {
    throw new Error(
      "Publication observations artifact does not match the terminal fixture identity",
    );
  }
  return value;
}

export function createRunnerEventCollector(requestedId, expected = FIXTURE_CATALOG[requestedId]) {
  let terminal = null;
  return {
    consume(line) {
      const event = parseRunnerEvent(line);
      if (event === null) return null;
      if (terminal !== null) throw new Error("Duplicate terminal fixture event");
      terminal = validateRunnerIdentity(event, requestedId, expected);
      return terminal;
    },
    get terminal() {
      return terminal;
    },
  };
}

export function classifyRunnerResult(terminal) {
  if (terminal.resultContract === EVIDENCE_RESULT_CONTRACT)
    return terminal.runStatus === "complete" ? "evidence-recorded" : "incomplete";
  return terminal.passed === terminal.total && terminal.failureCount === 0
    ? "automated-pass"
    : "automated-fail";
}

export function evaluateStorageIsolationRelation(primaryFirst, twin, primarySecond) {
  const checks = [];
  for (const [label, artifact] of [
    ["primary-first", primaryFirst],
    ["twin", twin],
    ["primary-second", primarySecond],
  ]) {
    validateObservationFixture(
      artifact,
      label === "twin" ? "active-probe-twin" : "active-probe",
      3,
    );
  }
  const primaryFirstOrigin = observationOrigin(primaryFirst);
  const twinOrigin = observationOrigin(twin);
  const primarySecondOrigin = observationOrigin(primarySecond);
  const primaryUrl = new URL(primaryFirstOrigin);
  const twinUrl = new URL(twinOrigin);
  checks.push(
    check("storage-primary-origin-stable", primaryFirstOrigin === primarySecondOrigin, [
      primaryFirstOrigin,
      primarySecondOrigin,
    ]),
  );
  checks.push(
    check("storage-sibling-origin-unique", primaryFirstOrigin !== twinOrigin, [
      primaryFirstOrigin,
      twinOrigin,
    ]),
  );
  checks.push(
    check("storage-origin-port", primaryUrl.port !== "" && primaryUrl.port === twinUrl.port, [
      primaryUrl.port,
      twinUrl.port,
    ]),
  );
  for (const scenario of ["local-storage", "indexed-db", "cookie"]) {
    const peerScenario =
      scenario === "local-storage"
        ? "peer-local-storage"
        : scenario === "indexed-db"
          ? "peer-indexed-db"
          : "peer-cookie";
    const first = storageValues(primaryFirst, scenario, peerScenario);
    const sibling = storageValues(twin, scenario, peerScenario);
    const resumed = storageValues(primarySecond, scenario, peerScenario);
    const initial = scenario === "indexed-db" ? [null, 1, 1, 2, 2] : [null, "1", "1", "2", "2"];
    const continued = scenario === "indexed-db" ? [2, 3, 3, 4, 4] : ["2", "3", "3", "4", "4"];
    const unavailable = [null, "1", null, "1", null];
    const allUnavailable =
      scenario === "cookie" &&
      [first, sibling, resumed].every((values) => valuesEqual(values, unavailable));
    checks.push(
      check(`${scenario}-primary-first`, allUnavailable || valuesEqual(first, initial), first),
    );
    checks.push(
      check(`${scenario}-twin-isolated`, allUnavailable || valuesEqual(sibling, initial), sibling),
    );
    checks.push(
      check(
        `${scenario}-primary-resumed`,
        allUnavailable || valuesEqual(resumed, continued),
        resumed,
      ),
    );
  }
  return relation("storage-isolation", checks);
}

function validCapture(value) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = ["viewportWidth", "viewportHeight", "x", "y", "width", "height"];
  if (
    Object.keys(value).length !== keys.length ||
    !keys.every((key) => Number.isInteger(value[key]))
  )
    return false;
  return (
    value.viewportWidth > 0 &&
    value.viewportWidth <= 8192 &&
    value.viewportHeight > 0 &&
    value.viewportHeight <= 8192 &&
    value.x >= 0 &&
    value.y >= 0 &&
    value.width > 0 &&
    value.height > 0 &&
    value.x + value.width <= value.viewportWidth &&
    value.y + value.height <= value.viewportHeight
  );
}

function observationOrigin(artifact) {
  const origin = artifact.fixture?.publicationOrigin;
  if (!validOrigin(origin)) throw new Error("Storage relation publication origin is invalid");
  return origin;
}

function validOrigin(value) {
  if (typeof value !== "string" || value.length > 256) return false;
  try {
    const url = new URL(value);
    return url.origin === value && url.protocol === "http:";
  } catch {
    return false;
  }
}

function storageValues(artifact, scenario, peerScenario) {
  const first = scenarioReport(artifact.phases[0], scenario).evidence;
  const second = scenarioReport(artifact.phases[1], scenario).evidence;
  const peer = scenarioReport(artifact.phases[2], peerScenario).evidence;
  return [first.previous, first.current, second.previous, second.current, peer.value];
}

function scenarioReport(phase, scenario) {
  const reports = Array.isArray(phase.reports) ? phase.reports : [];
  const matches = reports.filter((report) => report?.scenario === scenario);
  if (
    matches.length !== 1 ||
    typeof matches[0].evidence !== "object" ||
    matches[0].evidence === null
  ) {
    throw new Error(`Storage relation report ${scenario} is invalid`);
  }
  return matches[0];
}

function validateObservationFixture(artifact, fixtureId, phaseCount) {
  if (
    typeof artifact !== "object" ||
    artifact === null ||
    Array.isArray(artifact) ||
    artifact.schema !== "elephant-publication-observations-v2" ||
    artifact.fixture?.id !== fixtureId ||
    !Array.isArray(artifact.phases) ||
    artifact.phases.length !== phaseCount
  ) {
    throw new Error(`Relational observation for ${fixtureId} is invalid`);
  }
}

function valuesEqual(actual, expected) {
  return (
    actual.length === expected.length && actual.every((value, index) => value === expected[index])
  );
}

function check(label, passed, values) {
  return { label, passed, detail: JSON.stringify(values) };
}

function relation(id, checks) {
  return {
    id,
    status: checks.every(({ passed }) => passed) ? "automated-pass" : "automated-fail",
    checks,
  };
}
