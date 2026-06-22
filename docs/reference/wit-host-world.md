# WIT host world

tau publishes the **embedding contract** — the WIT host world — as a
frozen WIT package that any host embedding `tau-wasm-guest` must implement
(ADR-0055/ADR-0056). It is the second of tau's two public contracts; the
first is the [IR JSON Schema](ir-json-schema.md).

- **Package:** `tau:host@0.1.0`
- **World:** `runner`
- **Interface:** `host`
- **Source:** [`wit/tau-host.wit`](https://github.com/tau-rs/tau/blob/main/wit/tau-host.wit)
- **Drift test:** `crates/tau-ports/tests/wit_host_drift.rs`

## Host functions

| WIT host fn | Signature | Backing port |
|---|---|---|
| `complete` | `func(request-json: string) -> result<string, string>` | `tau_ports::llm::LlmBackend::complete` |
| `now-millis` | `func() -> u64` | `tau_ports::time::Clock::now` (i64 ms, bridged) |
| `next-u64` | `func() -> u64` | `tau_ports::random::RandomSource::fill` (the trait primitive) |

**Principle: host imports = inference + nondeterminism only.** Native tools,
the MCP-cassette replayer, skills, and the context pipeline are all compiled
into the guest; only the three crossing-points above require a host-side
implementation.

## WIT ⊊ ports

The WIT surface is a strict subset of the port traits. Every host function
maps to a concrete port method, but the ports expose more than the WIT world
requires — the guest requests only what it cannot satisfy in-wasm. The drift
test (`wit_host_drift.rs`) enforces this containment at compile time: if a
port method required by the WIT world is removed or renamed, the test fails to
compile.

## Signedness note

`now-millis` returns `u64`; the backing `Clock::now` returns `i64` (signed
milliseconds since the Unix epoch). The host bridge casts `i64 → u64` at the
boundary. Both sides document this as "milliseconds since Unix epoch"; the
sign difference is a WIT-vs-Rust surface mismatch absorbed by the bridge, not
a semantic difference.

## Stability

The package identifier `tau:host@0.1.0` is semver-versioned. ADR-0056 governs
stability: any additive change bumps the minor version; any breaking change
bumps the major version and requires a migration path. The frozen WIT file is
the authoritative artifact; prose documentation is informative only.
