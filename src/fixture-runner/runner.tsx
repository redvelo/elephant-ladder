import { useEffect, useRef, useState } from "react";
import type { RunnerFixtureDescriptor } from "./fixture";
import type { RunnerCheck } from "./reports";
import { startFixtureRun, type RunnerState } from "./run";
import "./runner.css";

export default function FixtureRunner() {
  const frameHost = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<RunnerState>("starting");
  const [checks, setChecks] = useState<RunnerCheck[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [fixture, setFixture] = useState<RunnerFixtureDescriptor | null>(null);

  useEffect(() => startFixtureRun(frameHost, { setState, setChecks, setError, setFixture }), []);

  const passed = checks.filter((check) => check.passed).length;
  return (
    <main className="fixture-runner" data-runner-state={state}>
      <header className="fixture-runner-header">
        <div>
          <p className="fixture-runner-kicker">Elephant Ladder / Qualification</p>
          <h1>Fixture runner</h1>
          <p className="fixture-runner-deck">
            Active publication behavior, observed in the packaged host.
          </p>
        </div>
        <p className="fixture-runner-state" aria-live="polite">
          <span>Run state</span>
          <strong>{state}</strong>
        </p>
      </header>
      <section className="fixture-runner-frame" aria-label="Rendered publication">
        <header className="fixture-runner-panel-header">
          <div>
            <p className="fixture-runner-label">Publication surface</p>
            <h2>{fixture?.label ?? "Loading fixture"}</h2>
          </div>
          <p>{fixture?.policyProfile ?? "Resolving policy"}</p>
        </header>
        <div className="fixture-runner-surface" ref={frameHost} />
      </section>
      <section className="fixture-runner-summary" aria-live="polite">
        <header className="fixture-runner-panel-header fixture-runner-results-header">
          <div>
            <p className="fixture-runner-label">Native evidence</p>
            <h2>Terminal summary</h2>
          </div>
          <p className="fixture-runner-score">
            <strong>{checks.length === 0 ? "--" : `${passed}/${checks.length}`}</strong>
            <span>{checks.length === 0 ? "collecting" : "checks passed"}</span>
          </p>
        </header>
        {error === null ? null : <pre>{error}</pre>}
        <div className="fixture-runner-table-scroll">
          <table>
            <thead>
              <tr>
                <th>Check</th>
                <th>Result</th>
                <th>Observed evidence</th>
              </tr>
            </thead>
            <tbody>
              {checks.map((check) => (
                <tr key={check.label} data-check-result={check.passed ? "passed" : "failed"}>
                  <td>{check.label}</td>
                  <td>
                    <span className="fixture-runner-verdict">{check.passed ? "pass" : "fail"}</span>
                  </td>
                  <td>{check.detail}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </main>
  );
}
