export const REPORT_TYPE = "elephant-active-probe-result-v1";

export const SCENARIOS = Object.freeze([
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
  "peer-local-storage",
  "peer-indexed-db",
  "peer-cookie",
  "peer-worker-teardown",
  "probe-complete",
  "peer-complete",
]);

export const OUTCOMES = Object.freeze(["passed", "failed", "blocked", "unsupported"]);

export function report(source, scenario, outcome, evidence = {}) {
  if (!SCENARIOS.includes(scenario) || !OUTCOMES.includes(outcome)) {
    throw new TypeError("Elephant active probe report is outside its bounded contract");
  }
  parent.postMessage(
    { type: REPORT_TYPE, source, scenario, outcome, evidence },
    "*",
  );
}

export function errorKind(error) {
  if (error instanceof DOMException && error.name === "SecurityError") return "security-error";
  if (error instanceof DOMException && error.name === "NotSupportedError") return "not-supported-error";
  if (error instanceof TypeError) return "type-error";
  return "other-error";
}

export function openDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("elephant-active-probe-v1", 1);
    request.addEventListener("upgradeneeded", () => request.result.createObjectStore("state"));
    request.addEventListener("success", () => resolve(request.result));
    request.addEventListener("error", () => reject(request.error));
  });
}

export function readDatabase(database) {
  return new Promise((resolve, reject) => {
    const request = database.transaction("state", "readonly").objectStore("state").get("probe");
    request.addEventListener("success", () => resolve(request.result ?? null));
    request.addEventListener("error", () => reject(request.error));
  });
}

export function writeDatabase(database, value) {
  return new Promise((resolve, reject) => {
    const transaction = database.transaction("state", "readwrite", { durability: "strict" });
    transaction.objectStore("state").put(value, "probe");
    transaction.addEventListener("complete", resolve);
    transaction.addEventListener("error", () => reject(transaction.error));
  });
}
