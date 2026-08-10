// TauRunService — an Angular service that exposes a single tau agent run as
// RxJS Observables (EPIC 5.4-b).
//
// A thin layer over @tau/embed-js's `TauComponent`: the component owns wasm
// instantiation and host-import wiring (`loadTau(...)`); this service only
// adapts the run's `AsyncIterable<RunEvent>` into Observables. It never
// touches wasm or jco directly, so it is insulated from how the component was
// loaded.
//
// Single-agent only: the guest exports one prompt-only `run`, so a run is one
// stream. Multi-agent rendering uses the separate TraceEvent vocab and is a
// later slice.

import { Injectable } from "@angular/core";
import { from, type Observable } from "rxjs";
import { filter, scan } from "rxjs/operators";
import type { RunEvent, RunInput, TauComponent } from "@tau/embed-js";

type TextDelta = Extract<RunEvent, { type: "text-delta" }>;

@Injectable({ providedIn: "root" })
export class TauRunService {
  /** Stream every RunEvent of one run, in arrival order. Completes when the
   * run's stream closes — i.e. just after the terminal `run-completed` (or
   * `fatal-error`) event. `fatal-error` is delivered as an ordinary `next`
   * value, not an Observable `error`, so subscribers see the whole stream. */
  run(tau: TauComponent, input: RunInput): Observable<RunEvent> {
    return from(tau.run(input));
  }

  /** The assistant text as it streams: `scan` of every `text-delta`, emitting
   * the cumulative string on each delta. Completes with the run. */
  text(tau: TauComponent, input: RunInput): Observable<string> {
    return this.run(tau, input).pipe(
      filter((event): event is TextDelta => event.type === "text-delta"),
      scan((acc, event) => acc + event.delta, ""),
    );
  }
}
