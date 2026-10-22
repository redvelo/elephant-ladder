(function () {
  document.documentElement.dataset.elephantClassic = "executed";
  window.addEventListener("message", function relayScriptedSvg(event) {
    const object = document.getElementById("svg-probe");
    const value = event.data;
    if (event.source !== object?.contentWindow
      || value?.type !== "elephant-active-probe-result-v1"
      || value?.source !== "scripted-probe.svg" || value?.scenario !== "scripted-svg"
      || value?.outcome !== "passed" || value?.evidence?.executed !== true) return;
    window.removeEventListener("message", relayScriptedSvg);
    parent.postMessage({
      type: "elephant-active-probe-result-v1",
      source: "probe.xhtml",
      scenario: "scripted-svg",
      outcome: "passed",
      evidence: { executed: true, origin: event.origin },
    }, "*");
  });
  parent.postMessage(
    {
      type: "elephant-active-probe-result-v1",
      source: "probe.xhtml",
      scenario: "classic-script",
      outcome: "passed",
      evidence: { value: "executed" },
    },
    "*",
  );
  void import("./probe-module.js");
})();
