# @tau/angular

An Angular service that exposes a `tau build wasm` component's `RunEvent`
stream as RxJS Observables (EPIC 5.4-b). Thin layer over
[`@tau/embed-js`](../embed-js): the component owns wasm instantiation and the
host-import wiring; this service only adapts the run's `AsyncIterable` into
Observables.

## Usage

```ts
import { Component, inject } from "@angular/core";
import { loadTau } from "@tau/embed-js";
import { TauRunService } from "@tau/angular";

@Component({
  selector: "tau-chat",
  standalone: true,
  template: `<pre>{{ text | async }}</pre>`,
})
export class ChatComponent {
  private readonly tauRun = inject(TauRunService);
  private readonly tau = loadTau({ complete: (req) => lookUpCassetteResponse(req) });

  text = from(this.tau).pipe(switchMap((t) => this.tauRun.text(t, { prompt: "hello" })));
}
```

## API

```ts
@Injectable({ providedIn: "root" })
class TauRunService {
  run(tau: TauComponent, input: RunInput): Observable<RunEvent>;  // completes on stream close
  text(tau: TauComponent, input: RunInput): Observable<string>;   // scan() of text-delta
}
```

- `run` emits every `RunEvent` in arrival order and **completes** when the
  run's stream closes — just after the terminal `run-completed` (or
  `fatal-error`) event. `fatal-error` is delivered as an ordinary `next`
  value, not an Observable `error`, so subscribers see the whole stream.
- `text` emits the cumulative assistant text on each `text-delta`.

## Scope

Single-agent only — the guest exports one prompt-only `run`. Multi-agent
rendering uses the separate `TraceEvent` vocab and is a later slice.

## Develop

```sh
npm install
npm test        # vitest — service driven against a fake TauComponent
npm run typecheck
```
