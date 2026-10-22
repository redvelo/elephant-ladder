"use strict";

// What the engine behind a reading system says it can do, and what it does when asked.
//
// Every answer is both shown on the page, for a platform no harness can drive, and
// restated in a compact line per group, for one the fixture runner can observe. A
// question this cannot ask without guessing is left out rather than inferred.

(function () {
  var CSS_PREFIXED = [
    ["-epub-hyphens", "none"],
    ["-epub-line-break", "strict"],
    ["-epub-text-align-last", "start"],
    ["-epub-text-combine", "horizontal"],
    ["-epub-text-combine-horizontal", "all"],
    ["-epub-text-emphasis-color", "red"],
    ["-epub-text-emphasis-position", "over left"],
    ["-epub-text-emphasis-style", "filled dot"],
    ["-epub-text-orientation", "upright"],
    ["-epub-text-underline-position", "under left"],
    ["-epub-word-break", "keep-all"],
    ["-epub-writing-mode", "vertical-rl"],
    ["text-transform", "-epub-fullwidth"],
  ];

  var CSS_UNPREFIXED = [
    ["hyphens", "auto"],
    ["line-break", "strict"],
    ["text-align-last", "start"],
    ["text-combine-upright", "all"],
    ["text-emphasis-style", "filled dot"],
    ["text-emphasis-position", "over left"],
    ["text-orientation", "upright"],
    ["text-underline-position", "under left"],
    ["word-break", "keep-all"],
    ["writing-mode", "vertical-rl"],
    ["ruby-position", "over"],
    ["text-transform", "full-width"],
    ["text-transform", "full-size-kana"],
    ["font-variant-east-asian", "full-width"],
  ];

  var IMAGES = [
    ["png", "media/probe.png", "image/png"],
    ["jpeg", "media/probe.jpg", "image/jpeg"],
    ["gif", "media/probe.gif", "image/gif"],
    ["webp", "media/probe.webp", "image/webp"],
    ["avif", "media/probe.avif", "image/avif"],
    ["jxl", "media/probe.jxl", "image/jxl"],
  ];

  var AUDIO = [
    ["mp3", "media/probe.mp3", 'audio/mpeg'],
    ["aac", "media/probe.m4a", 'audio/mp4; codecs="mp4a.40.2"'],
    ["opus", "media/probe.opus", 'audio/ogg; codecs="opus"'],
  ];

  var READING_SYSTEM_FEATURES = [
    "dom-manipulation",
    "layout-changes",
    "touch-events",
    "mouse-events",
    "keyboard-events",
    "spine-scripting",
  ];

  function answer(value) {
    return value === true ? "yes" : value === false ? "no" : String(value);
  }

  function report(group, entries) {
    var list = document.getElementById(group);
    var summary = [];
    for (var index = 0; index < entries.length; index += 1) {
      var name = entries[index][0];
      var value = entries[index][1];
      var term = document.createElement("dt");
      term.textContent = name;
      var description = document.createElement("dd");
      description.textContent = answer(value);
      if (value === true || value === false) description.setAttribute("data-answer", answer(value));
      list.appendChild(term);
      list.appendChild(description);
      // The summary separates entries with a semicolon, so a value cannot carry one.
      summary.push(name + "=" + answer(value).replace(/;/g, ","));
    }
    document.getElementById(group + "-summary").textContent = summary.join(";");
  }

  function supports(declarations) {
    return declarations.map(function (declaration) {
      var name = declaration[0] + ":" + declaration[1];
      if (typeof CSS === "undefined" || typeof CSS.supports !== "function") {
        return [name, "unknown"];
      }
      return [name, CSS.supports(declaration[0], declaration[1])];
    });
  }

  function readingSystem() {
    var entries = [
      ["viewport", window.innerWidth + "x" + window.innerHeight],
      ["devicePixelRatio", String(window.devicePixelRatio)],
      ["userAgent", navigator.userAgent],
    ];
    var system = navigator.epubReadingSystem;
    if (!system) {
      entries.unshift(["epubReadingSystem", false]);
      return entries;
    }
    // EPUB reading systems declare a name and a version alongside hasFeature.
    entries.unshift(["epubReadingSystem", true]);
    entries.splice(1, 0, ["name", system.name === undefined ? "absent" : String(system.name)]);
    entries.splice(2, 0, [
      "version",
      system.version === undefined ? "absent" : String(system.version),
    ]);
    for (var index = 0; index < READING_SYSTEM_FEATURES.length; index += 1) {
      var feature = READING_SYSTEM_FEATURES[index];
      var value;
      try {
        value = system.hasFeature(feature);
      } catch (error) {
        value = "unknown";
      }
      entries.push([feature, value === undefined || value === null ? "unimplemented" : value]);
    }
    return entries;
  }

  // Reports what the engine claims and what actually arrived, separately.
  //
  // A reading system may convert an image the engine cannot decode before serving it, so
  // a successful decode does not prove the engine has that decoder. The claim comes from
  // the engine alone; the delivered result is whatever the host handed over.
  function imageClaim(mime) {
    if (typeof ImageDecoder === "undefined" || !ImageDecoder.isTypeSupported) {
      return Promise.resolve("unknown");
    }
    return ImageDecoder.isTypeSupported(mime).then(
      function (supported) {
        return supported ? "yes" : "no";
      },
      function () {
        return "unknown";
      },
    );
  }

  function decodeImages() {
    return Promise.all(
      IMAGES.map(function (entry) {
        var delivered = new Promise(function (resolve) {
          var image = new Image();
          image.onload = function () {
            resolve(image.naturalWidth > 0 && image.naturalHeight > 0 ? "decoded" : "empty");
          };
          image.onerror = function () {
            resolve("failed");
          };
          image.src = entry[1];
        });
        return Promise.all([imageClaim(entry[2]), delivered]).then(function (both) {
          return [entry[0], both[0] + "/" + both[1]];
        });
      }),
    );
  }

  // Both what the engine claims for the type and whether it reads the file's metadata.
  function loadAudio() {
    return Promise.all(
      AUDIO.map(function (entry) {
        return new Promise(function (resolve) {
          var element = document.createElement("audio");
          var claim = element.canPlayType(entry[2]);
          var settle = function (loaded) {
            resolve([entry[0], (claim === "" ? "no" : claim) + "/" + (loaded ? "loaded" : "failed")]);
          };
          element.preload = "metadata";
          element.onloadedmetadata = function () {
            settle(true);
          };
          element.onerror = function () {
            settle(false);
          };
          element.src = entry[1];
          window.setTimeout(function () {
            settle(element.readyState > 0);
          }, 8000);
        });
      }),
    );
  }

  function run() {
    report("system", readingSystem());
    report("css-prefixed", supports(CSS_PREFIXED));
    report("css-unprefixed", supports(CSS_UNPREFIXED));
    var status = document.getElementById("status");
    Promise.all([decodeImages(), loadAudio()]).then(
      function (results) {
        report("images", results[0]);
        report("audio", results[1]);
        status.textContent = "complete";
        status.setAttribute("data-state", "complete");
      },
      function () {
        status.textContent = "partial: the resource probes did not settle";
        status.setAttribute("data-state", "partial");
      },
    );
  }

  if (document.readyState === "loading") {
    window.addEventListener("DOMContentLoaded", run);
  } else {
    run();
  }
})();
