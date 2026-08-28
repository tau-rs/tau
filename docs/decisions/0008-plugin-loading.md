# ADR-0008: Plugin loading mechanism — IPC over MessagePack-RPC + tau-pkg, tau-runtime, tau-domain amendments

**Status:** Accepted
**Date:** 2026-04-28
**Supersedes:** —
**Closes:** [ADR-0007](0007-tau-cli.md) §18 ("plugin loading deferred to
Phase 1+").
**Amends:** [ADR-0004](0004-tau-pkg.md) §3 (install lifecycle gains a
build step for `kind = "rust-cargo"` plugin packages),
[ADR-0006](0006-tau-runtime.md) §17 (additive `RuntimeError::Plugin*`
variants, `plugin_host` module, ten new tracing events).
**Refines:** [ADR-0005](0005-package-source-and-kind-serde.md) (custom
serde via Display/FromStr — same pattern applied to `PortKind` and
`PluginKind`).

## Context

Phase 0 sub-project 5 ([ADR-0007](0007-tau-cli.md) §18) explicitly
deferred plugin loading to Phase 1+. `tau install` already records
package source trees on disk, but the v0.1 runtime had no mechanism to
turn those source trees into running plugin processes the kernel could
dispatch through. Integration tests instead used `cfg(feature =
"test-mock")` compiled-in backends — viable for proving the kernel works
end-to-end, but not a path to user-supplied plugins.

This ADR closes that gap as the first sub-project of Phase 1 (ROADMAP
Phase 1 priority 1). It commits the plugin-loading mechanism, ships the
two new workspace crates (`tau-plugin-protocol`, `tau-plugin-sdk`) and
the per-port host module (`tau-runtime::plugin_host`), exercises the
mechanism end-to-end with two toy plugins (`echo-llm`, `echo-tool`),
retires the `test-mock` feature, and bundles the four ports the kernel
defines (`LlmBackend`, `Tool`, `Storage`, `Sandbox`) into one cohesive
loading surface.

The kernel surface is **unchanged**. Plugin loading produces
`Arc<dyn Dyn{LlmBackend,Tool,Storage,Sandbox}>` proxies that the
existing `Runtime::run` / `Runtime::run_with_history` consume without
knowing they are remote. All Phase 0 capability filtering, tracing, and
dispatch logic applies to plugin-backed implementations identically.

Per QG18, plugin trait boundaries and serve-mode-adjacent IPC schemas
require ADRs. This ADR bundles 18 design decisions plus the four ports'
IPC bindings plus the supporting tau-pkg / tau-domain amendments,
mirroring the [ADR-0006](0006-tau-runtime.md) /
[ADR-0007](0007-tau-cli.md) precedent of grouping tightly-coupled
cross-crate amendments into one ADR for design coherence. All
amendments here are solely motivated by the plugin-loading mechanism;
no promiscuous bundling.

Relevant Constitution constraints (spec §1.2):

| Constraint | Mechanism's answer |
|---|---|
| `forbid(unsafe_code)` workspace-wide | IPC fits without exception — process-level isolation, no FFI at the host boundary. |
| **G6** runtime not framework | Loading produces port proxies the kernel consumes; no new abstraction over the agent loop. |
| **G9** observable by default | Ten new tracing events; plugin-side `tracing` re-emitted via stderr; protocol recording is built in. |
| **G12** sandboxing | Out-of-scope here (priority 12), but the IPC mechanism is the *prerequisite* — the process boundary already exists; priority 12 will bolt platform sandbox primitives onto the spawn path. |
| **G15** continue-on-fail | Plugin crashes resolve in-flight calls to `RuntimeError::PluginCrashed`; host stays up. |
| **NG4** no marketplace | Plugins distribute as git URLs; tau-pkg builds from source; no registry, no signed binaries. |
| **NG9** no credential management | Plugin config is a static map passed at handshake; how plugins source secrets (env, OS keychain) is each plugin's concern, not the loader's. |
| **NG10** no telemetry | Tracing events are local; recording is an explicit user opt-in via `--record-protocol`. |

This is the first sub-project of Phase 1, and the mechanism here is the
substrate priorities 2 (real LLM-backend plugin) and 3 (real Tool
plugin) build on directly.

## Decision

### 1. Mechanism: out-of-process IPC

Plugins run as separate child processes of the host. Communication is
over the child's stdio pipes. Picked head-to-head against three
alternatives (see *Alternatives considered: mechanism* below for the
full rejection rationale):

- **dlopen / `abi_stable`** — rejected on `forbid(unsafe_code)` and on
  the inability to recover from a plugin crashing without taking the
  host down.
- **WASM / WASI** — rejected on tooling immaturity in 2026 for
  long-lived host processes hosting many guests, plus the LLM-client-
  in-WASM ecosystem still being thin (`reqwest` doesn't run cleanly).
- **Per-host-process plugin pool with shared memory** — rejected as a
  premature optimization; the IPC overhead at sub-millisecond scale is
  invisible next to LLM call latency, and the ergonomics complexity is
  high.

Out-of-process IPC fits the constitution cleanly: the process boundary
is the natural place to bolt G12 sandbox primitives onto later
(seccomp / landlock / sandbox-exec / AppContainer), and a plugin crash
resolves in-flight calls to `RuntimeError::PluginCrashed` without taking
the host down.

### 2. Wire format: MessagePack-RPC over stdio with length-prefixed framing

Each message is a length-prefixed MessagePack body:

```
+--------+--------+--------+--------+================+
|  big-endian u32 length (excl PFX) | MessagePack    |
+--------+--------+--------+--------+ message body  |
                                    +================+
```

Picked for: binary efficiency (~3–5× over JSON for tau payloads),
self-describing structure (lossless round-trip to JSON for debug),
serde-derive ergonomics on existing `tau-ports` types (no parallel
schema), and 2026 ecosystem maturity (`rmp-serde = "1"`).

Length-prefix is stricter than vanilla MessagePack-RPC requires (the
format is self-delimiting), but it gives three benefits: (1) each
message is a discrete chunk in a recording log; (2) non-Rust plugin
authors can read N bytes deterministically; (3) defensive resync if the
wire is ever corrupted. Max body size 64 MiB, configurable via
`PluginHostOptions`.

A body-size cap alone is not sufficient, because frame bodies are
decoded recursively: nesting depth is a *stack* budget, and a plugin
buys one level of it per byte (`0x91`, a one-element array, is a
one-byte container header). Frame bodies are therefore additionally
capped at `tau_plugin_protocol::MAX_DECODE_DEPTH` (128) nested
containers, matching `serde_json`'s recursion limit; deeper bodies are
rejected with `ProtocolError::BodyTooDeep`. Without that cap a ~1 KiB
frame could abort the host process with a stack overflow — see
issue #676.

The MessagePack-RPC frame shapes are:

| Frame | Shape |
|---|---|
| Request | `[0, msgid: u32, method: str, params: array]` |
| Response | `[1, msgid: u32, error: ErrorObj \| nil, result: any]` |
| Notification | `[2, method: str, params: array]` |

Alternative (rejected): JSON-RPC over stdio. Smaller payloads in
MessagePack matter for streaming chunks and tool-arg blobs; JSON's
self-describing-ness is preserved by `rmp-serde`'s lossless conversion
to JSON for debug purposes.

### 3. Lifecycle: long-lived multiplexed plugin processes per host session

One plugin process per (binary, host session) lives for the whole `tau
run` / `tau chat` invocation. Picked for:

- **TLS / HTTP keepalive amortization** — a real LLM-backend plugin
  reuses connections across calls; per-call spawn would force fresh
  TLS handshakes.
- **REPL responsiveness** — `tau chat` issues many turns; spawn cost
  per turn would compound to dozens of seconds.
- **Amortized spawn cost** — the build step (Decision 6) means plugin
  binaries aren't tiny; spawning once per call would be wasteful.

Per-call spawn was rejected for compounding overhead in chat workloads;
shared-pool-across-host-invocations was rejected for v0.1 because it
adds a daemon supervision concern that the constitution doesn't
motivate yet.

### 4. Concurrency: JSON-RPC `id`-correlated multiplexing; concurrent by default

A single plugin process handles multiple in-flight requests by msgid
correlation. The SDK's runner dispatches each `Request` frame on a
fresh `tokio::spawn`, so plugins are concurrent by default. An opt-in
serial mode is provided for plugins with non-`Send` state (e.g., a
plugin holding a `&mut` SQLite connection); the SDK exposes this via a
runner option, not a separate runner type.

Alternatives rejected:

- **One request at a time per plugin** — would block REPL streaming on
  any other in-flight call; trivially wrong for the multi-tool case.
- **Per-port dedicated processes** — already the lifecycle (one process
  per loaded port); concurrency is orthogonal.

### 5. Streaming: notification-based via `stream.chunk` + final response

Streaming `llm.stream` calls (see Decision 18 below — wire method is
`llm.stream`, not `llm.complete_streaming` as the spec drafted) are
modelled on the LSP `partialResult` precedent. Host sends an
`llm.stream` request with msgid=N. Plugin emits zero or more
`stream.chunk` notifications referencing N:

```
[2, "stream.chunk", [N, <CompletionChunk>]]
```

Plugin terminates by sending the regular response on msgid=N carrying
`{ stop_reason, usage }`. On error mid-stream, the plugin sends an
error response on msgid=N and the host SDK propagates a `LlmError` into
the stream's `Err` arm.

Host-side, a `stream_router` collects `stream.chunk` notifications into
an mpsc channel terminated by the final response, exposing a `Pin<Box<dyn
Stream<Item = Result<CompletionChunk, LlmError>>>>` to the runtime. The
runtime consumes it identically to an in-process implementation.

Alternative rejected: streaming-over-newline-delimited-JSON on a
sidecar pipe. Complexity-for-no-gain; the MessagePack-RPC notification
form already covers this case and round-trips cleanly through the
recording sink.

### 6. Discovery + build: `[plugin]` table + `kind = "rust-cargo"` build at install time

Plugin packages declare a `[plugin]` table in their `tau.toml`:

```toml
[plugin]
provides = "llm_backend"  # llm_backend | tool | storage | sandbox
kind     = "rust-cargo"   # only kind in v0.1; #[non_exhaustive]
bin      = "echo-llm"     # cargo bin target name
```

`tau install` shells out to `cargo build --release --bin <plugin.bin>`
in the cloned package directory at install time. Stdout / stderr stream
to host's tracing as `target = "tau_pkg::build"` (INFO for stdout, WARN
for stderr). On non-zero exit, the lockfile is NOT written; the cloned
source tree is retained for inspection / `tau install --force`. Future
kinds (`PythonPip`, `NodeNpm`, `Prebuilt`) are additive enum variants on
the `#[non_exhaustive]` `PluginKind`.

Alternatives rejected:

- **Build at run time, not install time** — would surface compile errors
  on first `tau run`, breaking the install-time error contract; would
  also slow first-run.
- **Pre-built binary distribution** — incompatible with NG4 (no
  marketplace) at v0.1; deferred until a `Prebuilt` kind variant lands.
- **Manifest-as-binary-only-distribution** — same as above; no
  marketplace path means no path to ship binaries.

### 7. Handshake: host-initiated `meta.handshake` with strict validation

Host first sends `[0, 1, "meta.handshake", [{ protocol_version, port,
trace_context, config }]]`. Plugin replies with `protocol_version`,
`provides`, `plugin_name`, `plugin_version`, `methods`, `schemas`. The
host validates:

| Rule | Failure variant |
|---|---|
| `protocol_version` matches host's | `ProtocolVersionMismatch { host, plugin }` |
| `provides` matches the manifest's `[plugin] provides` | `ProvidesMismatch { manifest, plugin_advertised }` |
| `methods` includes all required for the port | `MissingRequiredMethod { method }` |
| Reply parses as a valid `HandshakeResponse` | `Malformed { detail }` |
| Reply arrives within `handshake_timeout_ms` (default 5000) | `Timeout` |

On any failure: kill the process, return
`RuntimeError::PluginHandshakeFailed { plugin, reason }` with a
structured `HandshakeFailureReason`. The plugin is not given a chance
to re-handshake; the contract is strict.

The `provides` cross-check (manifest vs. advertised) is the security-
critical step: a tool plugin tagged `provides = "llm_backend"` in its
`tau.toml` but advertising `tool` at handshake (or vice versa) would
otherwise let one port masquerade as another. Mismatch is fatal at
handshake time.

Alternative rejected: handshake-less protocol (assume protocol v1, no
schema introspection). Foreseeable cost of cross-version handling
without a handshake (silent decoding errors mid-call) outweighs the
~5 ms handshake cost amortized over an entire host session.

### 8. Shutdown: `meta.shutdown` notification + graceful timeout chain

Host sends `[2, "meta.shutdown", []]` on host exit. SDK closes
in-flight call handles, runs the plugin's shutdown hook (if
implemented), exits within `shutdown_timeout_ms` (default 2000). After
timeout: SIGTERM, wait 500 ms, then SIGKILL. The full sequence is:

1. Send `meta.shutdown` notification.
2. Wait for child exit, up to `options.shutdown_timeout` (default 2 s).
3. If still alive: SIGTERM, wait 500 ms.
4. If still alive: SIGKILL.
5. Tracing event `plugin.exited { plugin, exit_code, signal, clean }`.

Alternative rejected: closing stdin only and relying on the SDK's
read-loop seeing EOF. Less explicit, harder to instrument, and
asymmetric with the request-shaped lifecycle of every other host→plugin
operation. The notification form mirrors `meta.handshake`'s
request-shape and reads naturally in protocol recordings.

### 9. Observability: plugin-side `tracing` events JSON-encoded to stderr

Plugin SDK ships a `tracing-subscriber` JSON layer pre-configured to
write events to stderr. Host's stderr task reads each line, parses each
as a JSON tracing event, and re-emits via `tracing::Event::dispatch`
with `target = format!("plugin::{}", plugin_name)`. Lines that fail
JSON parse are emitted raw at WARN as
`tracing::warn!(target = "plugin::{name}::raw", "{line}")`.

This means a tau user setting `RUST_LOG=tau=debug,plugin::echo-llm=debug`
sees the plugin's own tracing alongside host tracing in one stream,
properly leveled, with no special configuration on the plugin author's
side. The SDK provides the layer; the plugin author writes
`tracing::info!("turn ready")` and it shows up in the host log.

Alternative rejected: sidecar tracing transport (separate file, IPC
channel). Cost of separate channel ≫ value: stderr is already there,
already lossless, and already conventionally used for diagnostic output.

### 10. Trace context: flat `trace_context` field in handshake

The `meta.handshake` request carries
`trace_context: { run_id, agent_id, root_span_id }`. Plugin SDK
injects these as fields on every tracing event the plugin emits. True
distributed-span stitching (per-call parent-span IDs threaded through
notifications, span lifecycle propagation) is deferred to G14 perf
budgets / priority 13.

Alternative rejected: per-call trace context on every Request frame.
Substantial wire-overhead for marginal diagnostic value at v0.1; the
flat-context-per-handshake form is sufficient because each host
session has one run-id. Per-call context lands as an additive future
field on Request `params` when priority 13 motivates it.

### 11. Config delivery: static, in `meta.handshake`

Plugin config (the `[agents.<id>.config]` table from project `tau.toml`
or the equivalent for the active agent) is serialized to JSON and
passed once in the handshake. Per-call config overrides are not
supported in v0.1 — additive future field on Request `params` when
demand materializes.

Alternative rejected: streaming config updates via a notification.
Adds a config-mutation surface that the constitution doesn't motivate
at v0.1; static handshake-time config is sufficient for echo plugins,
HTTP-LLM plugins, and tool plugins anticipated in priorities 2 and 3.

### 12. Sandboxing: deferred to priority 12

Plugin processes in v0.1 run with full host privileges. Toy plugins are
author-trusted; real plugins (priorities 2 and 3) are user-trusted via
the install-time provenance of `tau install <git-url>`. OS-level
sandbox primitives (seccomp / landlock / sandbox-exec / AppContainer)
land in their own sub-project (priority 12). The IPC mechanism here is
the *prerequisite* — the process boundary already exists; priority 12
will bolt platform sandboxing onto the spawn path.

Alternative rejected: minimal sandbox at v0.1 (e.g., closing all FDs,
clearing env). Half-measures invite false security claims; better to
ship full-privilege at v0.1 and document it explicitly than ship a
patchwork that gets read as "sandboxed".

### 13. SDK shape: per-port generic runner functions; no proc macros

Plugin author implements the *same* `tau_ports::*` trait the kernel
uses. The SDK exposes a free function per port:

```rust
pub async fn run_llm_backend<T: LlmBackend + Send + Sync + 'static>(
    plugin: T,
) -> Result<(), SdkError>;
pub async fn run_tool<T: Tool + Send + Sync + 'static>(plugin: T)
    -> Result<(), SdkError>;
pub async fn run_storage<T: Storage + Send + Sync + 'static>(plugin: T)
    -> Result<(), SdkError>;
pub async fn run_sandbox<T: Sandbox + Send + Sync + 'static>(plugin: T)
    -> Result<(), SdkError>;
```

Plugin author's `main.rs`:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_llm_backend(MyBackend::new()).await?;
    Ok(())
}
```

That's the entire surface. No `#[derive(Plugin)]`. No `#[async_trait]`.
No magic. Plain Rust.

Alternatives rejected:

- **Proc-macro `#[plugin]`** — adds compile-time dependencies and a
  parallel trait surface that diverges from the kernel's. Rejected on
  G6 (runtime not framework) and constitution simplicity bias.
- **Single `run_plugin(provides, plugin)` runner** — would force a
  parallel trait union; per-port runners are clearer, type-safer, and
  the runtime cost of having four `async fn`s in the SDK is zero.
- **Plugin-as-library with embed-host pattern** — would require the
  plugin author to write the `main` themselves and reach into the SDK;
  per-port runner functions hide the framing / handshake / shutdown
  loop entirely.

### 14. `Configure` trait + two runner flavors per port

For plugins that consume the `meta.handshake` `config` field, the SDK
exposes a `Configure` trait:

```rust
pub trait Configure {
    type Config: serde::de::DeserializeOwned;
    fn from_config(config: Self::Config) -> Result<Self, ConfigError>
    where Self: Sized;
}
```

Two flavors per port runner: `run_llm_backend(plugin)` (no config —
plugin already constructed) vs `run_llm_backend_with_config::<T>()` (T
impls Configure; runner constructs T from handshake config). Plugin
author picks based on whether they need config.

`ConfigError` has typed variants: `Decode(#[from] serde_json::Error)`,
`MissingField(&'static str)`, `InvalidValue { field, detail }`. No
`Internal` variant — the same discipline as Decision 18 below applies
SDK-wide.

Alternative rejected: a single runner that accepts a `Box<dyn Fn(Value)
-> Result<T, ConfigError>>` constructor. More verbose at the call site
and forces the plugin author to box a closure; two named functions are
clearer.

### 15. Testing: layered (mock stdio peer + real-spawn integration)

Tests are layered for correctness vs. mechanism validation:

- **Mock stdio peer** (`FakeStdioPeer` in `tau-plugin-protocol` behind
  `test-support` feature): used by `tau-plugin-sdk` runners and
  `tau-runtime::plugin_host` `Ipc*` adapters to drive each side
  against a deterministic peer. Fast (~2 s per crate), no subprocess
  overhead, full control over crash / timeout / malformed responses.
- **Real-spawn integration** in `tau-cli`: `cargo build` → `tau run
  echo-agent "..."` against the real `echo-llm` / `echo-tool`
  processes. Slower (~10 s) but proves the full mechanism end-to-end
  including the build step, handshake, dispatch, streaming, shutdown,
  and tracing re-emit.
- **Phase 0 `test-mock` feature is retired**: `tau-cli`'s `cfg(feature
  = "test-mock")` blocks are deleted; `cmd::run` and `cmd::chat` rewire
  through `plugin_host` exclusively. The `no-default-features-cli` CI
  job continues to run unchanged (it never relied on `test-mock`).

Alternatives rejected:

- **Real-spawn only** — would slow per-crate test feedback to
  unacceptable levels; the mock peer covers ~80% of correctness cases.
- **Mock peer only** — would leave the build step, the actual stdio
  framing, and the `Command::spawn` path untested. Phase 0's bias
  against trigger-pathless code disallows this.

### 16. Toy plugins: `echo-llm` + `echo-tool` under `crates/tau-plugins/`

Two toy plugins ship in this sub-project as workspace members:

- **`echo-llm`**: `LlmBackend` plugin returning canned responses from
  config (static `canned_text` or scripted `script[turn]`). Test-only
  modes (`crash_after_handshake`, `delay_response_ms`,
  `error_on_method`) gate via config flags for deterministic failure-
  path testing.
- **`echo-tool`**: `Tool` plugin echoing its `text` arg back as a text
  content block.

Storage and Sandbox toy plugins are explicitly deferred — `tau-runtime`
doesn't wire those ports into the agent loop in v0.1, so their toy
plugins land alongside their host-wiring sub-project.

Toy plugins build under `cargo build --workspace`; CI catches breakage
immediately. They are excluded from any `--release` artifact tau might
publish (they're test fixtures, not products).

Alternative rejected: ship toy plugins for all four ports. Storage and
Sandbox aren't kernel-exercised in v0.1, so toy plugins for them would
test a mechanism with no consumer — dead-code-shaped tests that earn
nothing.

### 17. Debug tier: protocol recording + live decode + 3 new subcommands

Five debug capabilities ship in v0.1:

1. **Protocol recording** (`--record-protocol <path>`): every plugin
   process spawned during this `tau` invocation has its host-side
   framer wrapped in a `RecordingTap` that mirrors all frames to the
   path as JSONL. Format includes `{ts, plugin, dir, msgid, method,
   frame: <base64>}`. Replayable.
2. **Live wire-decode tracing**: when
   `RUST_LOG=tau_runtime::plugin_host::wire=debug`, the read-loop and
   writer-mutex emit a tracing event per frame with the decoded body
   pretty-printed as JSON. Decode happens lazily; at higher log levels
   the tracing subscriber's filter rejects the event before any decode
   work happens.
3. **`tau plugin protocol decode <path>`**: human-readable transcript
   reader for recording files. Filters by plugin / method / time.
4. **`tau plugin run <binary> [--interactive | --script]`**: standalone
   plugin runner. Interactive REPL drives method calls and prints
   responses; scripted form replays a `script.jsonl`.
5. **`tau plugin describe <name>`**: resolves an installed plugin from
   the lockfile, spawns it, runs `meta.handshake`, prints metadata,
   runs `meta.describe` on each method, dumps schemas. Plugin shut
   down cleanly after.

Three of the five (recording, live decode, describe) are nearly free
byproducts of the framer + handshake design. Two (decode CLI,
interactive runner) are small new subcommands, ~150 LOC each. Total
debug-tier surface ~400 LOC; small enough to ship in v0.1 without
slipping the sub-project.

Alternative rejected: ship debug tier as a separate sub-project. The
binary wire format is opaque enough that without these tools, a
malformed frame would be near-impossible to debug; shipping the
mechanism without them would be hostile to plugin authors.

### 18. Errors: four new typed `RuntimeError` variants; one new `InstallError`; NO new `Internal`

`tau-runtime::RuntimeError` gains four variants:

```rust
PluginSpawnFailed { plugin: String, source: io::Error }
PluginHandshakeFailed { plugin: String, reason: HandshakeFailureReason }
PluginCrashed { plugin: String, exit_status: ExitStatus, stderr_tail: String }
PluginContractViolation { plugin: String, detail: String }
```

`HandshakeFailureReason` is a structured sub-enum with five variants
(per Decision 7).

`tau-pkg::InstallError` gains two:

```rust
BuildFailed { exit_status: ExitStatus, stderr_tail: String }
CargoNotFound
```

`tau-plugin-protocol::ProtocolError`, `tau-plugin-sdk::SdkError`, and
`tau-plugin-sdk::ConfigError` are similarly typed-variant-only.

**No new `Internal` variants ship anywhere.** The mechanical CI test
`crates/tau-domain/tests/escape_hatch_registry.rs` continues to gate
against accidental additions. This codifies the discipline that
`PluginContractViolation` and `Sandbox(_)` are not `Internal` escapes —
each new variant has a concrete reachable failure path backed by ≥1
test, per the Phase-0 mid memo and ADR-0007 §17.

## Alternatives considered (mechanism)

The single biggest decision in this ADR is Decision 1 (out-of-process
IPC). Three alternatives were considered head-to-head before
committing.

### A. dlopen / `abi_stable` (in-process dynamic linking)

Dynamically link plugin `.so` / `.dylib` / `.dll` into the host's
address space; calls go through `abi_stable`'s ABI-stable trait
objects.

**Rejected** because:

- Workspace's `forbid(unsafe_code)` would need an exception for the
  dlopen call site and for `abi_stable`'s opaque pointer marshalling.
  Constitution-level workspace lints are not worth bending for one
  feature.
- A plugin crashing (`abort`, segfault, `panic` across an FFI boundary)
  takes the entire host down. `tau chat` evaporating because a tool
  plugin panic'd is a UX regression we can't accept.
- ABI evolution is a permanent tax: every `tau-ports` field addition
  forces an ABI bump. The IPC mechanism's wire-stable serialization
  insulates the host crate from this.
- Loading-time overhead (dlopen + ABI trait construction) is roughly
  comparable to spawn cost; the supposed in-process performance win is
  thin once the LLM call dominates.

### B. WASM / WASI

Plugins compile to WASM modules; host instantiates them in
wasmtime / wasmer.

**Rejected** because:

- 2026 ecosystem maturity for hosting-many-long-lived-guests is still
  thin. Component-model adoption is patchy.
- LLM-client-in-WASM (`reqwest`-style HTTP) doesn't run cleanly without
  WASI extensions that aren't universally available — defeats the
  purpose of plugin authors writing real backends.
- Capability-based security via WASI is a real win, but it's a v0.2+
  story; v0.1 priority 12 will deliver OS-level sandboxing on the IPC
  path with broader scope (not just compute, also network and FS).
- Build complexity for plugin authors (WASM target, no native crates)
  is high enough that priority 2's first real LLM backend would have
  to fight the ecosystem to ship.

WASM remains attractive for a future "untrusted plugin" tier. The IPC
mechanism doesn't preclude that direction; it simply doesn't pay the
ecosystem cost in 2026.

### C. Shared-memory ring buffer with one host process and per-plugin worker threads

Single process, plugins linked statically, communicate over a
shared-memory ring buffer.

**Rejected** because:

- Premature optimization: IPC overhead at sub-millisecond scale is
  invisible next to LLM call latency (typical 200 ms – 5 s). The 100 µs
  serialize+pipe write cost shows up nowhere in the budget.
- Static linking forces every plugin into the host's binary; defeats
  user-supplied plugins entirely.
- Re-introduces all of (A)'s crash-blast-radius problems with extra
  ring-buffer reasoning on top.

## Cross-references

- **Supersedes** [ADR-0007](0007-tau-cli.md) §18: the "plugin loading
  deferred to Phase 1+" decision is now closed by this ADR.
- **Builds on** [ADR-0004](0004-tau-pkg.md) §3: the install lifecycle
  gains a build step (Decision 6) for `kind = "rust-cargo"` plugin
  packages. `LockedPackage` gains an optional `plugin: Option<LockedPlugin>`
  field; lockfile schema bumps v1 → v2 with auto-upgrade on next
  install.
- **Follows the pattern of** [ADR-0005](0005-package-source-and-kind-serde.md):
  `PortKind` and `PluginKind` use custom serde via Display/FromStr,
  serializing as `"llm_backend"` / `"rust-cargo"` strings rather than
  adjacent-tagged objects. Same ergonomics, same TOML-friendliness.
- **Amends** [ADR-0006](0006-tau-runtime.md) §17 (additive vocabulary):
  four new `RuntimeError::Plugin*` variants, the `plugin_host` module,
  and ten new tracing events join the kernel's vocabulary. The §6
  capability filter remains correct under IPC because it runs *before*
  the request crosses the wire — the plugin never sees tools the agent
  isn't authorized to use. Integration tests verify this.
- **Builds on** [ADR-0007](0007-tau-cli.md) §15: `Runtime::run_with_history`
  is the kernel entry point `tau chat` threads through `plugin_host`-
  produced `Arc<dyn DynLlmBackend>` proxies. Both the §14 capability-
  filter amendment and the §15 `run_with_history` amendment apply
  unchanged to plugin-backed implementations.
- **ROADMAP**: closes Phase 1 priority 1; substrate for priorities 2
  (real LLM-backend plugin) and 3 (real Tool plugin), each its own
  sub-project building on this mechanism.

## Consequences

### Positive

- Plugin loading is real. Users can `tau install <git-url>` a plugin
  package, and the next `tau run` / `tau chat` will spawn it as a
  subprocess and dispatch through it.
- Two new workspace crates (`tau-plugin-protocol`, `tau-plugin-sdk`)
  are stable, documented, and tested. Plugin authors target plain Rust
  with the same `tau_ports::*` traits the kernel uses; no proc macros,
  no derive, no parallel trait surface.
- The kernel surface is unchanged. All Phase 0 capability filtering,
  tracing, dispatch, and `Runtime::run_with_history` paths apply to
  plugin-backed implementations identically.
- Crash isolation is real (G15): a plugin process crashing resolves
  in-flight calls to `RuntimeError::PluginCrashed` and the host stays
  up. `tau chat` recovers by surfacing the error to the REPL.
- Observability is real (G9): ten new tracing events on the host side,
  plugin-side tracing re-emitted via stderr at `target =
  plugin::<name>`, protocol recording one flag away.
- Debug tier ships in v0.1: `tau plugin describe`, `tau plugin run`,
  `tau plugin protocol decode`, plus `--record-protocol` and live wire
  decode tracing. Binary wire format opacity is fully addressed.
- Priorities 2 (real LLM backend) and 3 (real Tool plugin) are
  unblocked. Each is now its own sub-project building on a stable
  IPC mechanism.

### Negative

- **Per-call serde overhead** at sub-millisecond scale (MessagePack
  encode + pipe write + plugin decode + plugin re-encode + host
  decode). Invisible next to LLM latency, but real, and visible in
  micro-benchmarks. Documented; performance budgets land with priority
  13.
- **Lockfile schema bump v1 → v2.** Existing v0.1 installations
  auto-upgrade on next `tau install`, but older `tau` binaries reading
  a v2 lockfile surface `LockfileVersionTooNew`. Documented in release
  notes and in the migration section below.
- **Plugin processes run with full host privileges in v0.1.** OS-level
  sandboxing is priority 12 (a separate sub-project); v0.1 documents
  this explicitly. Toy plugins are author-trusted; real plugins
  (priorities 2 and 3) inherit the user-trust of `tau install
  <git-url>`.
- **One port per package, one binary per port.** Multi-port plugins
  (one binary providing both an `LlmBackend` and a related `Tool`) are
  deferred for simplicity. Will become an additive future field on
  `PluginManifest` if priority 2/3 plugins motivate it.
- **No auto-restart / circuit-breaker.** A crashed plugin returns
  `RuntimeError::PluginCrashed` and the user re-invokes. Documented;
  deferred indefinitely (re-invocation is acceptable for the developer-
  tool target audience per NG11).
- **`test-mock` feature retired.** Pre-existing tests that gated on
  `feature = "test-mock"` rebuild against `echo-llm` / `echo-tool`. CI
  matrix gains three build jobs (`tau-plugin-protocol`,
  `tau-plugin-sdk`, `tau-plugins`). Total required checks 12 → 15.
- **`#[tokio::main]` already coupled `tau-cli` to tokio (per ADR-0007
  §2);** `tau-runtime` now expands its `tokio` features to include
  `process` and `io-util` for the host side. Documented. The runtime
  stays async-runtime-agnostic via the `LlmBackend`/`Tool` traits;
  only the plugin-host module commits to tokio, mirroring tau-cli's
  binary-side commitment.

### Neutral / new obligations

- Future tau-runtime / tau-pkg / tau-domain amendments motivated by
  their own sub-projects (or a non-plugin-loading consumer) get their
  own ADRs; no promiscuous bundling.
- The ten new tracing events join the [ADR-0006](0006-tau-runtime.md)
  §3.9 vocabulary as additive (non-breaking per ADR-0006 §17). Future
  renames or removals require an ADR.
- The wire vocabulary (`meta.*`, `llm.*`, `tool.*`, `storage.*`,
  `sandbox.*`, `stream.*` method namespaces) is now a public-facing
  schema. Breaking changes require a `protocol_version` bump and an
  ADR; additive method or field additions on existing methods follow
  the additive-vocabulary discipline.
- Plugin authors writing against `tau-plugin-sdk` are bound to the
  `tau_ports::*` trait surface — that's deliberate (G6: kernel and
  plugins share one trait surface, no parallel one).

## Out of scope

The spec's §2.1 explicitly defers the following topics; they belong to
their own sub-projects, not this one:

| Topic | Where it lives |
|---|---|
| Real LLM-backend plugin (Anthropic / OpenAI HTTP) | Phase 1 priority 2 |
| Real Tool plugin (`fs-read`, `shell`) | Phase 1 priority 3 |
| Capability override (project tau.toml `[agents.<id>.capabilities]`) | Phase 1 priority 4 (tier 2) |
| Transitive `requires.tools` auto-install | Phase 1 priority 5 (tier 2) |
| Schema validation for tool args | Phase 1 priority 6 (tier 2) — `RuntimeError::PluginContractViolation` activates here |
| `tau update` / `tau verify` / `tau uninstall` | Phase 1 priority 7 (tier 2) |
| Streaming LLM responses end-to-end (`Runtime::run_streaming`) | Phase 1 priority 8 (tier 2) — protocol-side streaming primitive lands here, but the kernel consumer is priority 8 |
| Multi-agent orchestration | Phase 1 priority 9 (tier 3) |
| Workflow / pipeline runner | Phase 1 priority 10 (tier 3) |
| REPL persistence | Phase 1 priority 11 (tier 3) |
| OS-level sandboxing | Phase 1 priority 12 (tier 3) |
| Performance budgets in CI | Phase 1 priority 13 (tier 4) |
| `cargo audit` + `cargo-deny` in CI | Phase 1 priority 14 (tier 4) |
| Serve mode (JSON-RPC over stdio) | Phase 1 priority 15 (tier 4) — distinct from this sub-project's IPC; "serve mode" is the *kernel* exposing JSON-RPC to *outer* tools, not plugins talking to the kernel |
| Conformance test suite | Deferred until at least two real implementations exist |
| Auto-restart / circuit breaker around crashed plugins | Deferred indefinitely |

## Migration

Per spec §11:

| Concern | Path |
|---|---|
| **Lockfile schema** | TOML `version = 1` → `version = 2`. v1 lockfiles auto-upgrade on the next `tau install` (re-read manifests, re-build any `[plugin]` packages, write v2). Older `tau` binaries reading a v2 lockfile surface the existing `LockfileVersionTooNew` error path — no new error variant needed. |
| **Existing v0.1 installations** | `LockedPackage { plugin: None }` for legacy installs. `tau ls` shows them as data-only; `tau run` referencing them as a plugin fails with `RuntimeError::PluginContractViolation { detail: "package is not a plugin (no [plugin] manifest table)" }`. Documented in release notes. |
| **`test-mock` feature** | Removed from `tau-cli/Cargo.toml`. `cfg(feature = "test-mock")` blocks deleted. Tests previously gated on this rebuild against `echo-llm` / `echo-tool`. The `no-default-features-cli` CI job continues to run unchanged (it never relied on `test-mock`). |
| **Existing `Runtime::run` consumers** | Unchanged. The kernel's signature is identical; the only difference is the type of `Arc<dyn DynLlmBackend>` it receives. All Phase 0 integration tests keep passing. |
| **Plugin author distribution** | Pre-amendment: no plugins to distribute. Post-amendment: `tau install <git-url>` clones + builds. No backward-incompatibility because there is no "back". |

## Plan-erratum carryovers

Items discovered during implementation that diverge from the spec
text. The wire vocabulary and trait shapes documented elsewhere in
this ADR reflect the **as-implemented** truth; this section is the
audit trail.

1. **`tau-ports` had no `serde` feature** at the start of the sub-
   project (discovered in Task 1, RESOLVED in Task 9). The plan's
   Cargo.toml templates initially prescribed
   `tau-ports = { workspace = true, features = ["serde"] }` for both
   `tau-plugin-protocol` and `tau-plugin-sdk`, but `tau-ports/Cargo.toml`
   exposed only `default` and `test-fixtures`. Task 1 dropped the
   non-existent feature flag. Task 9 added the `serde` feature to
   `tau-ports/Cargo.toml` (propagating to `tau-domain/serde` +
   `uuid/serde`), gated all `Serialize` / `Deserialize` derives behind
   it, and restored `features = ["serde"]` on the two new crates'
   `tau-ports` deps. `Namespace` / `Key` use `serde(into = "String",
   try_from = "String")` to preserve their validating constructors.
2. **Wire method `llm.complete_streaming` is named `llm.stream`** in
   the implementation. Spec §4.3 / §4.4 / §4.5 / §4.6 reference
   `llm.complete_streaming`; those references are stale. The actual
   `tau_ports::LlmBackend` trait method is `stream` (not
   `complete_streaming`), so the SDK runner dispatches `llm.stream`
   over the wire. The streaming notification method `stream.chunk` is
   unchanged. **The canonical wire vocabulary is `llm.complete`,
   `llm.stream`, `stream.chunk`** — this ADR is the authoritative
   record. Future docs / spec edits should align.
3. **`CompletionChunk::Done` is named `CompletionChunk::Finish`** in
   actual `tau-ports`. The spec's `CompletionChunk::Done { stop_reason,
   usage }` should be read as `CompletionChunk::Finish { stop_reason,
   usage }`. The terminal-marker semantics are identical; only the
   variant name differs.
4. **`Tool` is stateful** with an `init` / `invoke` / `teardown`
   lifecycle (a Phase 0 design decision that the plan did not surface
   until Task 9). The SDK's `run_tool` runs the full lifecycle per
   `tool.call`. Host-side `IpcTool` (Task 15) maps a single `tool.call`
   RPC into one full plugin-side init→invoke→teardown sequence. The
   stateless `fn(args) -> result` model the spec implied is incorrect;
   the actual contract is the lifecycle the kernel already uses
   in-process.
5. **`with_dyn_*` builder methods on `RuntimeBuilder`** were added in
   Task 15 (`with_dyn_llm_backend`, `with_dyn_tool`, `with_dyn_storage`,
   `with_dyn_sandbox`). Necessary because IPC adapters return
   `Arc<dyn Dyn*>` (the dyn-compatible shim trait) but the existing
   `with_*` methods are bounded on the native `LlmBackend` / `Tool` /
   etc. traits, which IPC adapters can't implement directly (native
   `async fn` in trait isn't dyn-compatible). The `Dyn*` shim traits
   already exist in tau-runtime per Phase 0 ADR-0006; the new
   `with_dyn_*` methods simply expose them as builder entry points.
   Existing `with_*` callers are unchanged.

These five carryovers are recorded for fresh-eyes review (Task 25) and
for future reference. None of them changes the *decisions* in this
ADR — they document where the implementation's vocabulary diverged
from the spec's.

## Implementation reference

Tasks 1–22 of the implementation plan
(`docs/superpowers/plans/2026-04-28-plugin-loading.md`) deliver this
ADR's decisions:

| Tasks | Decisions |
|---|---|
| 1 | Workspace scaffold; `tau-plugin-protocol` + `tau-plugin-sdk` crates |
| 2 | `tau-domain::PluginManifest` / `PortKind` / `PluginKind` (Decision 6 schema) |
| 3–6 | Wire framer, `Frame` enum, error envelope, handshake/shutdown payload types, `FakeStdioPeer` (Decisions 2, 5, 7, 8, 15) |
| 7–10 | SDK tracing layer, handshake builder, per-port runners, `Configure` trait (Decisions 9, 10, 13, 14) |
| 11–12 | tau-pkg `[plugin]` manifest parsing, `BuildOptions`, install-time build, `LockedPlugin`, lockfile v1→v2 (Decision 6) |
| 13–17 | tau-runtime `plugin_host` module, spawn + handshake + stderr re-emit + shutdown, `IpcLlmBackend` / `IpcTool` / `IpcStorage` / `IpcSandbox`, `stream_router`, `RecordingSink::JsonlFile` (Decisions 1, 3, 4, 5, 7, 8, 9, 17, 18) |
| 18 | `echo-llm` + `echo-tool` toy plugins (Decision 16) |
| 19 | Drop `test-mock` feature; rewire `cmd::run` + `cmd::chat` (Decision 15) |
| 20 | `--record-protocol` global flag; `tau plugin {describe, run, protocol decode}` subcommands (Decision 17) |
| 21 | `tau-cli` real-spawn integration tests (Decision 15) |
| 22 | CI gains three build jobs; branch protection updated (Decision 18 audit gate) |
| 23 | This ADR + index update |
| 24 | Final local verification + mark PR ready |
| 25 | ADR-0008 fresh-eyes review (24 h or self-review per QG22) — flips status from Proposed to Accepted |
| 26 | Plan sign-off + ROADMAP + branch-protection update + squash merge |

The status of this ADR flips from **Proposed** to **Accepted** at Task
25 sign-off.
