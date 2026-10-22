import { staticDependencyValue } from "./static-dependency.js";
import { errorKind, openDatabase, readDatabase, report, writeDatabase } from "./report.js";

const source = "probe.xhtml";
const localStorageKey = "elephant-active-probe-v1";
const cookieKey = "elephant_active_probe_v1";
const wideCookieKey = "elephant_active_probe_wide_v1";
const lifecycleGenerationKey = "elephant-dedicated-worker-generation-v1";
const lifecycleChannelName = "elephant-dedicated-worker-lifecycle-v1";
// Four intervals of the worker's 100ms heartbeat.
const lifecycleQuietPeriod = 400;
const lifecycleDeadline = 2000;

const lifecycleSignal = waitForLifecycleSignal();
let lifecycleWorker;

void run();

async function run() {
  report(source, "document-module-static-import", "passed", { value: staticDependencyValue });

  try {
    const dependency = await import("./dynamic-dependency.js");
    report(source, "document-module-dynamic-import", "passed", {
      value: dependency.dynamicDependencyValue,
    });
  } catch (error) {
    report(source, "document-module-dynamic-import", "failed", { error: errorKind(error) });
  }

  await checkJson("relative-fetch", "../data/relative.json", "relative-fetch-loaded");
  await checkJson("root-relative-fetch", "/EPUB/data/root-relative.json", "root-relative-fetch-loaded");
  await checkRemoteFetch();
  await checkWorker();
  await checkDedicatedWorkerTeardown();
  await checkSharedWorker();
  await checkServiceWorker();
  await checkStorage();
  checkDocumentDomainRelaxation();
  checkReaderIsolation();
  await checkNativeCapabilityIsolation();
  checkPopup();
  await checkDocumentResources();
  await checkWavRange();
  report(source, "probe-complete", "passed");
}

async function checkDedicatedWorkerTeardown() {
  if (!("BroadcastChannel" in globalThis)) {
    report(source, "dedicated-worker-teardown", "unsupported", { broadcastChannel: false });
    return;
  }
  try {
    await lifecycleSignal;
    const previous = localStorage.getItem(lifecycleGenerationKey);
    const teardown =
      previous === null
        ? { staleHeartbeat: false, heardMs: null }
        : await awaitGenerationSilence(previous, lifecycleQuietPeriod, lifecycleDeadline);
    const generation = randomGeneration();
    const started = await startLifecycleWorker(generation);
    localStorage.setItem(lifecycleGenerationKey, generation);
    report(source, "dedicated-worker-teardown", teardown.staleHeartbeat ? "failed" : "passed", {
      previous,
      generation,
      started,
      ...teardown,
    });
  } catch (error) {
    report(source, "dedicated-worker-teardown", "failed", { error: errorKind(error) });
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

function startLifecycleWorker(generation) {
  return new Promise((resolve, reject) => {
    lifecycleWorker = new Worker(new URL("./dedicated-lifecycle-worker.js", import.meta.url), {
      type: "module",
    });
    const timer = setTimeout(() => reject(new Error("lifecycle-worker-timeout")), 3000);
    lifecycleWorker.addEventListener("message", (event) => {
      if (event.data?.type !== "started" || event.data.generation !== generation) return;
      clearTimeout(timer);
      resolve(true);
    }, { once: true });
    lifecycleWorker.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error("lifecycle-worker-error"));
    }, { once: true });
    lifecycleWorker.postMessage({ type: "start", generation });
  });
}

function randomGeneration() {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
}

async function checkSharedWorker() {
  if (!("SharedWorker" in globalThis)) {
    report(source, "shared-worker", "unsupported", { constructorPresent: false });
    return;
  }
  try {
    const value = await new Promise((resolve, reject) => {
      const worker = new SharedWorker(new URL("./shared-module-worker.js", import.meta.url), {
        type: "module",
        name: "elephant-active-probe-v1",
      });
      const timer = setTimeout(() => {
        worker.port.close();
        reject(new DOMException("shared worker timed out", "SecurityError"));
      }, 3000);
      worker.port.addEventListener("message", (event) => {
        clearTimeout(timer);
        worker.port.close();
        resolve(event.data);
      }, { once: true });
      worker.addEventListener("error", () => {
        clearTimeout(timer);
        worker.port.close();
        reject(new DOMException("shared worker blocked", "SecurityError"));
      }, { once: true });
      worker.port.start();
    });
    const passed = value?.value === "shared-module-worker-ready"
      && value?.workerDependencyValue === "worker-static-dependency-loaded";
    report(source, "shared-worker", passed ? "passed" : "failed", value);
  } catch (error) {
    const kind = errorKind(error);
    report(source, "shared-worker", kind === "security-error" ? "blocked" : "failed", {
      constructorPresent: true,
      error: kind,
    });
  }
}

async function checkRemoteFetch() {
  try {
    await fetch("https://example.invalid/elephant-ladder-remote-probe", {
      signal: AbortSignal.timeout(3000),
    });
    report(source, "remote-fetch", "failed", { blocked: false });
  } catch (error) {
    report(source, "remote-fetch", "blocked", { blocked: true, error: errorKind(error) });
  }
}

async function checkJson(scenario, url, expected) {
  try {
    const response = await fetch(url);
    const value = response.ok ? (await response.json()).value : null;
    report(source, scenario, response.ok && value === expected ? "passed" : "failed", {
      status: response.status,
      value,
    });
  } catch (error) {
    report(source, scenario, "failed", { error: errorKind(error) });
  }
}

async function checkWorker() {
  try {
    const value = await new Promise((resolve, reject) => {
      const worker = new Worker(new URL("./dedicated-module-worker.js", import.meta.url), { type: "module" });
      const timer = setTimeout(() => {
        worker.terminate();
        reject(new Error("worker-timeout"));
      }, 3000);
      worker.addEventListener("message", (event) => {
        clearTimeout(timer);
        worker.terminate();
        resolve(event.data);
      }, { once: true });
      worker.addEventListener("error", () => {
        clearTimeout(timer);
        worker.terminate();
        reject(new Error("worker-error"));
      }, { once: true });
    });
    const passed = value?.value === "dedicated-module-worker-ready"
      && value?.workerDependencyValue === "worker-static-dependency-loaded";
    report(source, "dedicated-module-worker", passed ? "passed" : "failed", value);
  } catch {
    report(source, "dedicated-module-worker", "failed", { error: "worker-error" });
  }
}

async function checkServiceWorker() {
  if (!("serviceWorker" in navigator)) {
    report(source, "service-worker-registration", "unsupported", { registrationPresent: false });
    return;
  }
  try {
    const registration = await navigator.serviceWorker.register(
      new URL("./service-worker.js", import.meta.url),
      { scope: new URL("../", import.meta.url).pathname },
    );
    await registration.unregister();
    report(source, "service-worker-registration", "failed", { registrationPresent: true });
  } catch (error) {
    let registrationPresent = false;
    try {
      registrationPresent = (await navigator.serviceWorker.getRegistration(
        new URL("../", import.meta.url).pathname,
      )) !== undefined;
    } catch {
      registrationPresent = false;
    }
    report(source, "service-worker-registration", registrationPresent ? "failed" : "blocked", {
      error: errorKind(error),
      registrationPresent,
    });
  }
}

async function checkStorage() {
  try {
    const previous = localStorage.getItem(localStorageKey);
    const current = String(Number(previous ?? "0") + 1);
    localStorage.setItem(localStorageKey, current);
    report(source, "local-storage", "passed", { previous, current });
  } catch (error) {
    report(source, "local-storage", "failed", { error: errorKind(error) });
  }

  try {
    const database = await openDatabase();
    const previous = await readDatabase(database);
    const current = Number(previous ?? 0) + 1;
    await writeDatabase(database, current);
    database.close();
    report(source, "indexed-db", "passed", { previous, current });
  } catch (error) {
    report(source, "indexed-db", "failed", { error: errorKind(error) });
  }

  try {
    const previous = readCookie(cookieKey);
    const current = String(Number(previous ?? "0") + 1);
    document.cookie = `${cookieKey}=${current}; Path=/; Max-Age=86400; SameSite=Strict`;
    const stored = readCookie(cookieKey);
    document.cookie = `${wideCookieKey}=visible; Domain=localhost; Path=/; Max-Age=86400; SameSite=Strict`;
    const wideDomainCookieVisible = readCookie(wideCookieKey) !== null;
    if (wideDomainCookieVisible) {
      document.cookie = `${wideCookieKey}=; Domain=localhost; Path=/; Max-Age=0; SameSite=Strict`;
    }
    const outcome = wideDomainCookieVisible
      ? "failed"
      : stored === current
        ? "passed"
        : stored === null
          ? "blocked"
          : "failed";
    report(source, "cookie", outcome, {
      previous,
      current,
      stored,
      wideDomainCookieVisible,
    });
  } catch (error) {
    report(source, "cookie", "failed", { error: errorKind(error) });
  }
}

function checkDocumentDomainRelaxation() {
  const domainBefore = document.domain;
  const originBefore = location.origin;
  const target = "localhost";
  let assignment = "accepted";
  try {
    document.domain = target;
  } catch (error) {
    assignment = errorKind(error);
  }
  const domainAfter = document.domain;
  const originAfter = location.origin;
  const relaxed = domainBefore !== target && domainAfter === target;
  report(source, "document-domain-relaxation", relaxed ? "failed" : assignment === "accepted" ? "passed" : "blocked", {
    assignment,
    domainBefore,
    domainAfter,
    originBefore,
    originAfter,
    target,
    crossOriginIsolated,
  });
}

function readCookie(name) {
  const prefix = `${name}=`;
  const value = document.cookie.split("; ").find((item) => item.startsWith(prefix));
  return value === undefined ? null : value.slice(prefix.length);
}

function checkReaderIsolation() {
  if (parent === window) {
    report(source, "reader-isolation", "unsupported", { reason: "not-framed" });
    return;
  }
  try {
    void parent.document.documentElement;
    report(source, "reader-isolation", "failed", { readerDocumentReadable: true });
  } catch (error) {
    report(source, "reader-isolation", "blocked", {
      error: errorKind(error),
      readerDocumentReadable: false,
    });
  }
}

async function checkNativeCapabilityIsolation() {
  const exposed = ["__TAURI__", "__TAURI_IPC__"]
    .filter((name) => name in window);
  if (exposed.length > 0) {
    report(source, "native-capability-isolation", "failed", { exposed });
    return;
  }
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  const rawBridgePresent = "ipc" in window;
  if (typeof invoke !== "function") {
    report(source, "native-capability-isolation", "passed", {
      ipcTransport: "absent",
      rawBridgePresent,
    });
    return;
  }
  let timer;
  try {
    const result = await Promise.race([
      invoke("plugin:app|version"),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("invoke-timeout")), 3000);
      }),
    ]);
    report(source, "native-capability-isolation", "failed", {
      invoke: "resolved",
      resultType: typeof result,
    });
  } catch (error) {
    const message = String(error).slice(0, 256);
    const disabled = message === "Error: Tauri IPC is disabled by Elephant Ladder";
    report(source, "native-capability-isolation", disabled ? "passed" : "failed", {
      ipcTransport: disabled ? "disabled" : "present",
      invoke: disabled ? "disabled" : "indeterminate",
      rawBridgePresent,
      error: errorKind(error),
      message,
    });
  } finally {
    clearTimeout(timer);
  }
}

function checkPopup() {
  const popup = window.open("about:blank", "elephant-active-probe-popup", "popup,width=120,height=120");
  if (popup === null) {
    report(source, "popup", "blocked", { opened: false });
    return;
  }
  popup.close();
  report(source, "popup", "failed", { opened: true });
}

async function checkDocumentResources() {
  if (document.readyState !== "complete") {
    await new Promise((resolve) => window.addEventListener("load", resolve, { once: true }));
  }
  const styled = document.getElementById("styled-probe");
  const color = styled === null ? "missing" : getComputedStyle(styled).color;
  report(source, "stylesheet", color === "rgb(17, 68, 102)" ? "passed" : "failed", { color });

  const image = document.getElementById("image-probe");
  if (image instanceof HTMLImageElement) {
    try {
      await image.decode();
    } catch {
      // A positive intrinsic width remains the cross-engine load signal.
    }
  }
  const complete = image instanceof HTMLImageElement && image.complete;
  const naturalWidth = image instanceof HTMLImageElement ? image.naturalWidth : 0;
  const loaded = complete && naturalWidth > 0;
  report(source, "image", loaded ? "passed" : "failed", { complete, naturalWidth });
}

async function checkWavRange() {
  try {
    const response = await fetch("../media/exact-44-byte.wav", {
      headers: { Range: "bytes=0-43" },
    });
    const bytes = new Uint8Array(await response.arrayBuffer());
    const passed = response.status === 206
      && response.headers.get("Content-Range") === "bytes 0-43/44"
      && bytes.byteLength === 44
      && String.fromCharCode(...bytes.slice(0, 4)) === "RIFF";
    report(source, "wav-range", passed ? "passed" : "failed", {
      byteLength: bytes.byteLength,
      contentRange: response.headers.get("Content-Range"),
      status: response.status,
    });
  } catch (error) {
    report(source, "wav-range", "failed", { error: errorKind(error) });
  }
}
