// Every reaction on this page is CSS over the checked state of a form control.
// This script therefore never changes the document: it reads storage, restores
// answers by setting `checked`, and flips hidden state boxes. A reading system
// that re-lays out a document when it mutates has nothing to react to.
(function () {
  var KEY = "one-small-step-quiz-v1";
  var TOTAL = 5;

  function ready(run) {
    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", run);
    else run();
  }

  function load() {
    try {
      var raw = localStorage.getItem(KEY);
      var value = raw ? JSON.parse(raw) : null;
      if (!value || typeof value !== "object") return { answers: {}, best: null };
      return { answers: value.answers || {}, best: typeof value.best === "number" ? value.best : null };
    } catch (error) {
      return { answers: {}, best: null };
    }
  }

  function save(state) {
    try { localStorage.setItem(KEY, JSON.stringify(state)); } catch (error) {}
  }

  ready(function () {
    var quiz = document.getElementById("quiz");
    var state = load();

    Object.keys(state.answers).forEach(function (question) {
      var input = quiz.querySelector('input[name="' + question + '"][value="' + state.answers[question] + '"]');
      if (input) input.checked = true;
    });

    var scoreField = quiz.querySelector(".score");
    var bestField = document.getElementById("best");

    function correctCount() {
      var correct = 0, answered = 0;
      Array.prototype.forEach.call(quiz.querySelectorAll("fieldset[data-question]"), function (set) {
        var chosen = set.querySelector("input:checked");
        if (!chosen) return;
        answered += 1;
        if (chosen.closest("label").getAttribute("data-correct") === "true") correct += 1;
      });
      return { correct: correct, answered: answered };
    }

    function render() {
      var score = correctCount();
      scoreField.value =
        score.answered === 0
          ? "Answer the five questions below."
          : score.answered < TOTAL
            ? "Answered " + score.answered + " of " + TOTAL + ". Correct so far: " + score.correct + "."
            : "You scored " + score.correct + " of " + TOTAL + ".";
      bestField.value =
        state.best === null
          ? "No score saved yet."
          : "Best so far: " + state.best + " of " + TOTAL + ", remembered by the book.";
    }

    quiz.addEventListener("change", function (event) {
      if (event.target.type !== "radio") return;
      state.answers[event.target.name] = event.target.value;
      var score = correctCount();
      if (score.answered === TOTAL && (state.best === null || score.correct > state.best)) state.best = score.correct;
      save(state);
      render();
    });

    quiz.addEventListener("reset", function () {
      state.answers = {};
      save(state);
      // The reset clears the controls after this event, so read the new state next.
      setTimeout(render, 0);
    });

    render();

    var trip = document.getElementById("trip-form");
    var flags = {
      name: document.getElementById("trip-state-name"),
      group: document.getElementById("trip-state-group"),
      done: document.getElementById("trip-state-done"),
      blocked: document.getElementById("trip-state-blocked"),
    };
    function show(which) {
      Object.keys(flags).forEach(function (key) { flags[key].checked = key === which; });
    }

    // A reading system may withhold form submission entirely. If the submit event
    // never arrives, say so rather than appearing to do nothing.
    var submitting = false;
    trip.querySelector('button[type="submit"]').addEventListener("click", function () {
      submitting = false;
      setTimeout(function () { if (!submitting) show("blocked"); }, 0);
    });

    trip.addEventListener("submit", function (event) {
      submitting = true;
      event.preventDefault();
      if (!trip.elements.name.value.trim()) { show("name"); trip.elements.name.focus(); return; }
      if (!trip.elements.group.value) { show("group"); trip.elements.group.focus(); return; }
      show("done");
    });
  });
})();
