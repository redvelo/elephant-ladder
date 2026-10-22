let heartbeat;

self.addEventListener("message", (event) => {
  if (event.data?.type !== "start" || typeof event.data.generation !== "string") return;
  const channel = new BroadcastChannel("elephant-dedicated-worker-lifecycle-v1");
  const beat = () => channel.postMessage({ generation: event.data.generation });
  beat();
  heartbeat = setInterval(beat, 100);
  self.postMessage({ type: "started", generation: event.data.generation });
});
