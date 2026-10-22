// The result is painted onto a canvas that is always laid out, so the page
// geometry never changes and the reading system never re-measures the document.
// Progress is reported by flipping hidden checkboxes that the stylesheet reads.
(function () {
  function ready(run) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run);
    else run();
  }

  ready(function () {
    var video = document.getElementById("launch-video");
    var playButton = document.getElementById("video-play");
    var started = document.getElementById("video-started");
    if (video && playButton && started) {
      playButton.addEventListener("click", function () {
        started.checked = true;
        var playing = video.play();
        if (playing && playing.catch) playing.catch(function () { started.checked = false; });
      });
      video.addEventListener("play", function () { started.checked = true; });
    }

    var button = document.getElementById("count-button");
    var run = document.getElementById("count-run");
    var done = document.getElementById("count-done");
    var failed = document.getElementById("count-failed");
    var canvas = document.getElementById("count-canvas");

    function draw(data) {
      var context = canvas.getContext("2d");
      if (!context) { failed.checked = true; return; }
      var rows = data.top.slice(0, 15);
      var width = canvas.width, height = canvas.height;
      var left = 300, right = 40, top = 56, bottom = 24;
      var step = (height - top - bottom) / rows.length;
      var most = rows.length ? rows[0][1] : 1;
      context.clearRect(0, 0, width, height);
      context.fillStyle = "#ffffff";
      context.fillRect(0, 0, width, height);
      context.fillStyle = "#111111";
      context.font = "28px Georgia, serif";
      context.textBaseline = "middle";
      context.fillText(data.total + " words counted in " + data.ms + " ms", 24, 30);
      context.font = "24px Georgia, serif";
      rows.forEach(function (row, index) {
        var y = top + step * index + step / 2;
        context.textAlign = "right";
        context.fillStyle = "#111111";
        context.fillText(row[0], left - 16, y);
        var bar = Math.max(2, (width - left - right) * (row[1] / most));
        context.fillStyle = "#4a4a4a";
        context.fillRect(left, y - step * 0.3, bar, step * 0.6);
        context.textAlign = "left";
        context.fillStyle = "#111111";
        context.fillText(String(row[1]), left + bar + 10, y);
      });
      context.textAlign = "left";
      done.checked = true;
    }

    // The button keeps the focus, because moving focus into a visually hidden
    // control makes the browser scroll the column and the page appears to jump.
    button.addEventListener("click", function () {
      if (run.checked) return;
      run.checked = true;
      var text = "";
      Array.prototype.forEach.call(document.querySelectorAll("section p, section h1, section h2, figcaption"), function (node) {
        if (!node.closest("#activity")) text += " " + node.textContent;
      });
      var worker;
      try {
        worker = new Worker("../script/worker.js");
      } catch (error) {
        failed.checked = true;
        return;
      }
      worker.onmessage = function (event) { draw(event.data); worker.terminate(); };
      worker.onerror = function () { failed.checked = true; worker.terminate(); };
      worker.postMessage({ text: text, started: Date.now() });
    });
  });
})();
