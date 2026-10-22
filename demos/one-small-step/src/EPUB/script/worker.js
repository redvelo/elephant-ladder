var STOP = ("a an and the of to in on at by for from with as was were is are be been it its this that these those " +
  "he his him she her they their them we our you your who which what when where while than then there here " +
  "had has have did do does not no nor but or so if into over under about before after again still yet " +
  "one two three four five six seven eight nine ten first second").split(" ");
var stop = {}; STOP.forEach(function (w) { stop[w] = true; });

self.onmessage = function (event) {
  var text = event.data.text.toLowerCase();
  var counts = {}, total = 0;
  var words = text.match(/[a-z][a-z'-]*/g) || [];
  words.forEach(function (w) {
    w = w.replace(/^'+|'+$/g, "");
    if (!w) return;
    total += 1;
    if (stop[w] || w.length < 3) return;
    counts[w] = (counts[w] || 0) + 1;
  });
  var top = Object.keys(counts).map(function (w) { return [w, counts[w]]; })
    .sort(function (a, b) { return b[1] - a[1] || (a[0] < b[0] ? -1 : 1); }).slice(0, 20);
  self.postMessage({ top: top, total: total, ms: Date.now() - event.data.started });
};
