// useTauRun — a typed React hook that drives a single tau agent run and
// exposes its streaming RunEvents as reactive state (EPIC 5.4-a).
//
// It is a thin layer over @tau/embed-js's `TauComponent`: the component owns
// wasm instantiation and the host-import wiring (`loadTau(...)`); this hook
// owns only the React lifecycle — starting a run, accumulating its events and
// concatenated `text`, and settling to a terminal `status`. It never touches
// wasm or jco directly, so it is insulated from how the component was loaded.
//
// Single-agent only: the guest exports one prompt-only `run`, so `text` and
// `events` describe exactly one stream. Multi-agent rendering uses the
// separate TraceEvent vocab and is a later slice.

import { useCallback, useEffect, useRef, useState } from "react";
import type { RunEvent, RunInput, RunOutcome, TauComponent } from "@tau/embed-js";

/** Lifecycle of a single run. `idle` before the first `start`; `running`
 * while events stream; `done` once the stream settles (a `run-completed`
 * event or a clean end-of-stream); `error` on a `fatal-error` event or a
 * thrown iteration. */
export type TauRunStatus = "idle" | "running" | "done" | "error";

/** The terminal error surfaced by a `fatal-error` event (or a thrown
 * iteration), flattened to the two fields a UI needs. */
export interface TauRunError {
  kind: string;
  detail: string;
}

export interface UseTauRun {
  /** Begin (or restart) a run. Clears prior state, then streams the new run.
   * Calling `start` again supersedes any in-flight run. */
  start: (input: RunInput) => void;
  /** Every RunEvent seen so far, in arrival order. */
  events: RunEvent[];
  /** Concatenation of every `text-delta` event's `delta` — the assistant
   * text as it streams. */
  text: string;
  status: TauRunStatus;
  /** The run's outcome, set when a `run-completed` event arrives. */
  outcome: RunOutcome | null;
  /** The terminal error, set when a `fatal-error` event arrives (or
   * iteration throws). */
  error: TauRunError | null;
}

export function useTauRun(tau: TauComponent): UseTauRun {
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [text, setText] = useState("");
  const [status, setStatus] = useState<TauRunStatus>("idle");
  const [outcome, setOutcome] = useState<RunOutcome | null>(null);
  const [error, setError] = useState<TauRunError | null>(null);

  // Monotonic token identifying the current run. Bumped on every `start` and
  // on unmount; the streaming loop compares against it before each state
  // update so a superseded or unmounted run can never write stale state.
  const runToken = useRef(0);
  useEffect(() => () => void (runToken.current += 1), []);

  const start = useCallback(
    (input: RunInput) => {
      const token = (runToken.current += 1);
      setEvents([]);
      setText("");
      setOutcome(null);
      setError(null);
      setStatus("running");

      void (async () => {
        try {
          for await (const event of tau.run(input)) {
            if (runToken.current !== token) return;
            setEvents((prev) => [...prev, event]);
            switch (event.type) {
              case "text-delta":
                setText((prev) => prev + event.delta);
                break;
              case "run-completed":
                setOutcome(event.outcome);
                setStatus("done");
                break;
              case "fatal-error":
                setError({ kind: event.kind, detail: event.detail });
                setStatus("error");
                break;
              default:
                break;
            }
          }
          // Stream closed. If no terminal event settled the status, treat a
          // clean end-of-stream as completion.
          if (runToken.current === token) {
            setStatus((s) => (s === "running" ? "done" : s));
          }
        } catch (err) {
          if (runToken.current !== token) return;
          setError({
            kind: "UseTauRunError",
            detail: err instanceof Error ? err.message : String(err),
          });
          setStatus("error");
        }
      })();
    },
    [tau],
  );

  return { start, events, text, status, outcome, error };
}
