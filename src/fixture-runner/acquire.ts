import { isPublicationSummary } from "../host/bootstrap/publication";
import type { LibraryBootstrap, PublicationSummary } from "../host/bootstrap/types";
import { isNavigationPath } from "../host/navigation";
import { createPerformanceTraceId, nativeRequest } from "../host/requests";
import type { RunnerFixtureDescriptor } from "./fixture";
import { isRecord } from "./guards";

export type AcquisitionStep = "policy" | "importing" | "opening";

/**
 * Takes the fixture through the product's own path: the policy the profile names is saved
 * through the policy route, the handed-over file is read and imported through the import
 * route, and the open route answers with the reading page to navigate to. No step here is
 * particular to a fixture; a book handed over by the platform travels exactly this way.
 */
export async function acquireFixture(
  bootstrap: LibraryBootstrap,
  fixture: RunnerFixtureDescriptor,
  signal: AbortSignal,
  onStep: (step: AcquisitionStep) => void,
): Promise<string> {
  onStep("policy");
  await applyPolicyProfile(bootstrap, fixture.policyProfile, signal);
  onStep("importing");
  const file = await handedFixtureFile(bootstrap, fixture, signal);
  const publication = await importFixture(bootstrap, file, signal);
  onStep("opening");
  return openFixture(bootstrap, publication, signal);
}

function policyForProfile(profile: RunnerFixtureDescriptor["policyProfile"]) {
  switch (profile) {
    case "active-enabled-offline-v1":
      return { activeContent: "automatic", remoteResources: "blocked" } as const;
  }
}

async function applyPolicyProfile(
  bootstrap: LibraryBootstrap,
  profile: RunnerFixtureDescriptor["policyProfile"],
  signal: AbortSignal,
): Promise<void> {
  const authority = {
    sessionToken: bootstrap.sessionToken,
    libraryDocumentId: bootstrap.libraryDocumentId,
    trusted: true,
  };
  const current = await nativeRequest("/native/publication-policy", authority, { signal });
  if (!current.ok) throw new Error(`Policy read answered HTTP ${current.status}`);
  const stored: unknown = await current.json();
  if (
    !isRecord(stored) ||
    !isRecord(stored.revisions) ||
    !Number.isInteger(stored.revisions.global)
  )
    throw new Error("Stored policy is invalid");
  const saved = await nativeRequest("/native/publication-policy", authority, {
    method: "PUT",
    json: { baseRevision: stored.revisions.global, policy: policyForProfile(profile) },
    signal,
  });
  if (!saved.ok) throw new Error(`Policy save answered HTTP ${saved.status}`);
}

async function handedFixtureFile(
  bootstrap: LibraryBootstrap,
  fixture: RunnerFixtureDescriptor,
  signal: AbortSignal,
): Promise<File> {
  const authority = { sessionToken: bootstrap.sessionToken, trusted: true };
  const pending = await nativeRequest("/native/opened-publications/pending", authority, {
    signal,
  });
  if (pending.status !== 200)
    throw new Error(`No fixture was handed over (HTTP ${pending.status})`);
  const arrival: unknown = await pending.json();
  const expectedName = `${fixture.id}.epub`;
  if (!isRecord(arrival) || typeof arrival.openId !== "string" || arrival.fileName !== expectedName)
    throw new Error("The handed publication is not the selected fixture");
  const bytes = await nativeRequest(
    `/native/opened-publications/${encodeURIComponent(arrival.openId)}/bytes`,
    authority,
    { signal },
  );
  if (!bytes.ok) throw new Error(`Fixture bytes answered HTTP ${bytes.status}`);
  const buffer = await bytes.arrayBuffer();
  const digest = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", buffer)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  if (digest !== fixture.expectedSha256)
    throw new Error(`Handed fixture digest ${digest} does not match the catalog`);
  return new File([buffer], expectedName, { type: "application/epub+zip" });
}

const importConflictRetryMs = 400;
const importConflictAttempts = 25;

async function importFixture(
  bootstrap: LibraryBootstrap,
  file: File,
  signal: AbortSignal,
): Promise<PublicationSummary> {
  for (let attempt = 0; ; attempt += 1) {
    const response = await nativeRequest(
      "/native/publications/import",
      { sessionToken: bootstrap.sessionToken },
      { method: "POST", contentType: "application/epub+zip", body: file, signal },
    );
    if (response.ok) {
      const imported: unknown = await response.json();
      if (!isRecord(imported) || !isPublicationSummary(imported.publication))
        throw new Error("Import returned invalid publication data");
      return imported.publication;
    }
    if (response.status === 409 && attempt < importConflictAttempts) {
      await new Promise((resolve) => setTimeout(resolve, importConflictRetryMs));
      if (signal.aborted) throw new DOMException("Import cancelled.", "AbortError");
      continue;
    }
    throw new Error(`Import failed with HTTP ${response.status}: ${await response.text()}`);
  }
}

async function openFixture(
  bootstrap: LibraryBootstrap,
  publication: PublicationSummary,
  signal: AbortSignal,
): Promise<string> {
  const traceId = createPerformanceTraceId();
  for (let operation = 1; ; operation += 1) {
    const response = await nativeRequest(
      `/native/publications/${encodeURIComponent(publication.publicationId)}/open`,
      { sessionToken: bootstrap.sessionToken, libraryDocumentId: bootstrap.libraryDocumentId },
      {
        method: "POST",
        headers: { "X-Elephant-Trace-Id": traceId, "X-Elephant-Open-Operation": String(operation) },
        signal,
      },
    );
    const body = await response.text();
    let opened: unknown;
    try {
      opened = JSON.parse(body) as unknown;
    } catch {
      throw new Error(`Open answered HTTP ${response.status}: ${body}`);
    }
    if (response.status === 202) continue;
    if (!response.ok) {
      const status = isRecord(opened) && typeof opened.status === "string" ? opened.status : null;
      throw new Error(`Open failed with ${status ?? `HTTP ${response.status}`}`);
    }
    if (
      !isRecord(opened) ||
      opened.status !== "navigate" ||
      !isNavigationPath(opened.navigationPath)
    )
      throw new Error("Open returned invalid navigation data");
    return opened.navigationPath;
  }
}
