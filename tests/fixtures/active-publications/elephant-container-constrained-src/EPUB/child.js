(async () => {
  const blocked = (operation) => {
    try {
      operation();
      return false;
    } catch {
      return true;
    }
  };
  const parentDomBlocked = blocked(() => parent.document.documentElement.setAttribute("data-hostile-child", "modified"));
  const frameElementBlocked = frameElement === null || blocked(() => frameElement.style.width = "1px");
  const readerDomBlocked = blocked(() => parent.parent.document.documentElement.setAttribute("data-hostile-reader", "modified"));
  const credentialsBlocked = blocked(() => { document.cookie = "hostile=1"; localStorage.setItem("hostile", "1"); });
  const agent = document.querySelector("script[data-epub-navigator-content-agent]") || [...document.scripts].find((script) => /\/public\/content-agent\/[0-9a-f]{64}\.js$/.test(new URL(script.src).pathname));
  let nativeRouteBlocked = true;
  if (agent instanceof HTMLScriptElement) {
    try {
      await fetch(`${new URL(agent.src).origin}/native/info`);
      nativeRouteBlocked = false;
    } catch {
      nativeRouteBlocked = true;
    }
  }
  parent.postMessage({
    type: "elephant-container-child-result-v1",
    childScript: true,
    epubReadingSystem: typeof navigator.epubReadingSystem === "object" && navigator.epubReadingSystem !== null,
    parentDomBlocked,
    frameElementBlocked,
    readerDomBlocked,
    credentialsBlocked,
    nativeRouteBlocked
  }, "*");
})();
