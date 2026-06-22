# WIT host world (embedding contract)

tau's **embedding contract** (ADR-0056) is the WIT host world in
[`wit/tau-run.wit`](https://github.com/tau-rs/tau/blob/main/wit/tau-run.wit) —
`package tau:run@0.1.0`. Language-neutral embedders consume it via wit-bindgen / jco.

The host world has a **frozen minimal 3-function surface** — the ports the guest
cannot satisfy in-wasm, projected across the boundary:

| WIT host function | signature | tau-ports trait |
|---|---|---|
| `complete` | `func(request-json: string) -> result<string, string>` | `llm::LlmBackend` (JSON-serialized request/response) |
| `now-millis` | `func() -> u64` | `time::Clock` |
| `next-u64` | `func() -> u64` | `random::RandomSource` |

The surface is frozen and drift-tested (`tau-wasm-host/tests/wit_host_drift.rs`):
adding, removing, renaming, or re-shaping a host function fails the test
deliberately. Signature drift between these functions and their ports also breaks
compilation via `tau-wasm-guest/src/host_ports.rs`.

The `runner` world also **exports** `run`; that payload is not yet frozen and the
package stays `0.x` until it settles (then it graduates to `1.0.0` under ADR-0056's
embedding-contract semver). The package is named `tau:run` (it carries both the
host imports and the `run` export); ADR-0056's `tau:host` was illustrative of the
versioning mechanism.
