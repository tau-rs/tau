# @tau/react

A typed React hook for streaming a `tau build wasm` component's `RunEvent`s
into your UI (EPIC 5.4-a). Thin layer over
[`@tau/embed-js`](../embed-js): the component owns wasm instantiation and the
host-import wiring; this hook owns only the React lifecycle.

## Usage

```tsx
import { loadTau } from "@tau/embed-js";
import { useTauRun } from "@tau/react";

const tau = await loadTau({ complete: (req) => lookUpCassetteResponse(req) });

function Chat() {
  const { start, text, status, outcome, error } = useTauRun(tau);
  return (
    <>
      <button onClick={() => start({ prompt: "hello" })} disabled={status === "running"}>
        Run
      </button>
      <pre>{text}</pre>
      {status === "error" && <p role="alert">{error?.kind}: {error?.detail}</p>}
    </>
  );
}
```

`useTauRun(tau)` returns:

| field     | type                                            | meaning                                             |
| --------- | ----------------------------------------------- | --------------------------------------------------- |
| `start`   | `(input: RunInput) => void`                     | begin (or restart) a run; supersedes any in-flight  |
| `events`  | `RunEvent[]`                                     | every event so far, in arrival order                |
| `text`    | `string`                                         | concatenation of every `text-delta` `delta`         |
| `status`  | `"idle" \| "running" \| "done" \| "error"`      | run lifecycle                                        |
| `outcome` | `RunOutcome \| null`                             | set on `run-completed`                              |
| `error`   | `{ kind: string; detail: string } \| null`       | set on `fatal-error` (or a thrown iteration)         |

`status` settles to `done` on a `run-completed` event or a clean
end-of-stream, and to `error` on `fatal-error`. A superseded or unmounted run
can never write stale state (guarded by a monotonic run token).

## Scope

Single-agent only — the guest exports one prompt-only `run`, so `text`/`events`
describe exactly one stream. Multi-agent rendering uses the separate
`TraceEvent` vocab and is a later slice.

## Develop

```sh
npm install
npm test        # vitest — hook driven against a fake TauComponent
npm run typecheck
```
