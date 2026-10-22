import { workerDependencyValue } from "./worker-dependency.js";

self.postMessage({ value: "dedicated-module-worker-ready", workerDependencyValue });
