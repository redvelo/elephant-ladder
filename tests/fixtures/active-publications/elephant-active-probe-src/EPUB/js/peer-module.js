const reportType = "elephant-active-probe-result-v1";
const source = "peer.xhtml";
const localStorageKey = "elephant-active-probe-v1";
const cookieKey = "elephant_active_probe_v1";
const lifecycleGenerationKey = "elephant-dedicated-worker-generation-v1";
const lifecycleChannelName = "elephant-dedicated-worker-lifecycle-v1";
// Four intervals of the worker's 100ms heartbeat.
const lifecycleQuietPeriod = 400;
const lifecycleDeadline = 2000;

const lifecycleSignal = waitForLifecycleSignal();

void run();

async function run() {
  try {
    report(source, "peer-local-storage", "passed", {
      value: localStorage.getItem(localStorageKey),
    });
  } catch (error) {
    report(source, "peer-local-storage", "failed", { error: errorKind(error) });
  }

  try {
    const database = await openDatabase();
    const value = await readDatabase(database);
    database.close();
    report(source, "peer-indexed-db", "passed", { value });
  } catch (error) {
    report(source, "peer-indexed-db", "failed", { error: errorKind(error) });
  }

  try {
    report(source, "peer-cookie", "passed", { value: readCookie(cookieKey) });
  } catch (error) {
    report(source, "peer-cookie", "failed", { error: errorKind(error) });
  }

  await checkWorkerTeardown();

  report(source, "peer-complete", "passed");
}

async function checkWorkerTeardown() {
  if (!("BroadcastChannel" in globalThis)) {
    report(source, "peer-worker-teardown", "unsupported", { broadcastChannel: false });
    return;
  }
  try {
    await lifecycleSignal;
    const previous = localStorage.getItem(lifecycleGenerationKey);
    const teardown =
      previous === null
        ? { staleHeartbeat: false, heardMs: null }
        : await awaitGenerationSilence(previous, lifecycleQuietPeriod, lifecycleDeadline);
    report(source, "peer-worker-teardown", teardown.staleHeartbeat ? "failed" : "passed", {
      previous,
      ...teardown,
    });
  } catch (error) {
    report(source, "peer-worker-teardown", "failed", { error: errorKind(error) });
  }
}

function waitForLifecycleSignal() {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      window.removeEventListener("message", receive);
      reject(new Error("lifecycle-signal-timeout"));
    }, 5000);
    function receive(event) {
      if (event.source !== parent || event.data?.type !== "elephant-worker-lifecycle-observe-v1") return;
      clearTimeout(timer);
      window.removeEventListener("message", receive);
      resolve();
    }
    window.addEventListener("message", receive);
  });
}

// Observes the previous generation falling silent rather than asserting it already has:
// teardown is asynchronous, and a worker being torn down correctly can still emit a beat.
// A generation still beating at the deadline outlived its document, which is the failure.
function awaitGenerationSilence(generation, quietPeriod, deadline) {
  return new Promise((resolve) => {
    const channel = new BroadcastChannel(lifecycleChannelName);
    const started = performance.now();
    let lastHeartbeat = null;
    let quiet;
    let expiry;
    const finish = (staleHeartbeat) => {
      clearTimeout(quiet);
      clearTimeout(expiry);
      channel.close();
      resolve({
        staleHeartbeat,
        heardMs: lastHeartbeat === null ? null : Math.round(lastHeartbeat - started),
      });
    };
    expiry = setTimeout(() => finish(true), deadline);
    quiet = setTimeout(() => finish(false), quietPeriod);
    channel.addEventListener("message", (event) => {
      if (event.data?.generation !== generation) return;
      lastHeartbeat = performance.now();
      clearTimeout(quiet);
      quiet = setTimeout(() => finish(false), quietPeriod);
    });
  });
}

function readCookie(name) {
  const prefix = `${name}=`;
  const value = document.cookie.split("; ").find((item) => item.startsWith(prefix));
  return value === undefined ? null : value.slice(prefix.length);
}

function report(reportSource, scenario, outcome, evidence = {}) {
  parent.postMessage({ type: reportType, source: reportSource, scenario, outcome, evidence }, "*");
}

function errorKind(error) {
  if (error instanceof DOMException && error.name === "SecurityError") return "security-error";
  if (error instanceof DOMException && error.name === "NotSupportedError") return "not-supported-error";
  if (error instanceof TypeError) return "type-error";
  return "other-error";
}

function openDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("elephant-active-probe-v1", 1);
    request.addEventListener("upgradeneeded", () => request.result.createObjectStore("state"));
    request.addEventListener("success", () => resolve(request.result));
    request.addEventListener("error", () => reject(request.error));
  });
}

function readDatabase(database) {
  return new Promise((resolve, reject) => {
    const request = database.transaction("state", "readonly").objectStore("state").get("probe");
    request.addEventListener("success", () => resolve(request.result ?? null));
    request.addEventListener("error", () => reject(request.error));
  });
}
