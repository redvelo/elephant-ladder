(() => {
  "use strict";

  const source = "EPUB/text/probe.xhtml";
  const scenarios = [
    ["httpsFetch", "connect-src", "https://httpbingo.org/get?elephant=remote-fetch", () => fetch("https://httpbingo.org/get?elephant=remote-fetch")],
    ["httpsImage", "img-src", "https://httpbingo.org/image/png?elephant=remote-image", () => append("img", "src", "https://httpbingo.org/image/png?elephant=remote-image")],
    ["httpsStyle", "style-src", "https://httpbingo.org/base64/OnJvb3R7LS1lbGVwaGFudC1yZW1vdGU6MTd9?content-type=text%2Fcss&elephant=remote-style", () => append("link", "href", "https://httpbingo.org/base64/OnJvb3R7LS1lbGVwaGFudC1yZW1vdGU6MTd9?content-type=text%2Fcss&elephant=remote-style", { rel: "stylesheet" })],
    ["httpsScript", "script-src", "https://httpbingo.org/base64/Z2xvYmFsVGhpcy5lbGVwaGFudFJlbW90ZVNjcmlwdD0xNzs=?content-type=application%2Fjavascript&elephant=remote-script", () => append("script", "src", "https://httpbingo.org/base64/Z2xvYmFsVGhpcy5lbGVwaGFudFJlbW90ZVNjcmlwdD0xNzs=?content-type=application%2Fjavascript&elephant=remote-script")],
    ["httpsAudio", "media-src", "https://httpbingo.org/base64/UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=?content-type=audio%2Fwav&elephant=remote-audio", () => append("audio", "src", "https://httpbingo.org/base64/UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=?content-type=audio%2Fwav&elephant=remote-audio")],
    ["httpsFrame", "frame-src", "https://httpbingo.org/base64/PCFkb2N0eXBlIGh0bWw-PHNjcmlwdD5wYXJlbnQucG9zdE1lc3NhZ2UoMTcsJyonKTwvc2NyaXB0Pg==?content-type=text%2Fhtml&elephant=remote-frame", () => append("iframe", "src", "https://httpbingo.org/base64/PCFkb2N0eXBlIGh0bWw-PHNjcmlwdD5wYXJlbnQucG9zdE1lc3NhZ2UoMTcsJyonKTwvc2NyaXB0Pg==?content-type=text%2Fhtml&elephant=remote-frame")],
    ["wssEcho", "connect-src", "wss://httpbingo.org/websocket/echo?elephant=remote-wss", () => new WebSocket("wss://httpbingo.org/websocket/echo?elephant=remote-wss")],
    ["httpFetch", "connect-src", "http://httpbingo.org/get?elephant=remote-http", () => fetch("http://httpbingo.org/get?elephant=remote-http")],
    ["wsEcho", "connect-src", "ws://httpbingo.org/websocket/echo?elephant=remote-ws", () => new WebSocket("ws://httpbingo.org/websocket/echo?elephant=remote-ws")],
  ];

  const results = {};
  let current = null;
  let resolveViolation = null;
  document.addEventListener("securitypolicyviolation", (event) => {
    if (current === null || resolveViolation === null) return;
    const [name, directive, target] = current;
    if (!directiveMatches(event.effectiveDirective, directive)) return;
    const expected = new URL(target.replaceAll("&amp;", "&"));
    if (event.blockedURI !== "" && new URL(event.blockedURI).origin !== expected.origin) return;
    resolveViolation(`blocked:${event.effectiveDirective}`);
  });

  window.addEventListener("load", async () => {
    for (const scenario of scenarios) {
      current = scenario;
      results[scenario[0]] = await observe(scenario[3]);
    }
    current = null;
    document.getElementById("status").textContent = "Network-policy observations complete.";
    parent.postMessage({
      type: "elephant-remote-policy-result-v1",
      source,
      ...results,
    }, "*");
  });

  function observe(start) {
    return new Promise((resolve) => {
      let settled = false;
      let timer = 0;
      const finish = (value) => {
        if (settled) return;
        settled = true;
        resolveViolation = null;
        clearTimeout(timer);
        resolve(value);
      };
      resolveViolation = finish;
      timer = setTimeout(() => finish("indeterminate"), 750);
      try {
        const operation = start();
        if (operation instanceof Promise) operation.then(() => finish("loaded"), () => undefined);
        else if (operation instanceof WebSocket) operation.addEventListener("open", () => finish("loaded"), { once: true });
        else operation.addEventListener("load", () => finish("loaded"), { once: true });
      } catch {
        finish("failed");
      }
    });
  }

  function append(name, attribute, value, attributes = {}) {
    const element = document.createElement(name);
    for (const [key, item] of Object.entries(attributes)) element.setAttribute(key, item);
    element.setAttribute(attribute, value);
    document.body.append(element);
    return element;
  }

  function directiveMatches(actual, expected) {
    return actual === expected ||
      (expected === "script-src" && actual === "script-src-elem") ||
      (expected === "style-src" && actual === "style-src-elem");
  }
})();
