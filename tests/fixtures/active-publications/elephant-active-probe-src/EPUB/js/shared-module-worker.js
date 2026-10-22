import { workerDependencyValue } from "./worker-dependency.js";

self.addEventListener("connect", (event) => {
  const [port] = event.ports;
  port.postMessage({ value: "shared-module-worker-ready", workerDependencyValue });
});
