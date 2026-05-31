# ADR-0006: tau-runtime kernel + Tool capabilities amendment

> **Updated 2026-05-31 (β.1.4):** kernel now lives in `tau-runtime-core`
> (no_std + alloc, executor-agnostic); the tokio host shell is
> `tau-runtime-tokio`. References below to `tau-runtime` should be read
> as the union of the two crates unless otherwise noted.

**Status:** Accepted
**Date:** 2026-04-28
**Supersedes:** —
**Amends:** ADR-0003 §2 "Four trait shapes" (the `Tool` trait gains an
additive `capabilities()` default method).

## Context

tau-runtime (sub-project 4, ROADMAP row 4) is the kernel that runs agents:
it loads pre-constructed plugin instances, dispatches the multi-turn agent
loop against an `LlmBackend` and the registered `Tool`s, enforces
capability declarations at runtime, and emits structured logs. It is the
first sub-project that orchestrates plugin instances rather than just
defining their data shapes (tau-domain), trait boundaries (tau-ports), or
installation (tau-pkg). Per ROADMAP row 4 the v0.1 scope is the **solo
path** only; multi-agent orchestration (G10) is sub-project 5+.

Per QG18, public API additions and plugin-trait amendments require ADRs.
This ADR records BOTH the kernel design AND the additive
`Tool::capabilities() -> &[Capability]` amendment to ADR-0003. The two are
tightly coupled: typed enforcement at runtime (G14) requires the
trait-level declaration, and the amendment exists solely because tau-runtime
needs it. Bundling avoids splitting one motivated change across two ADRs;
future tau-ports amendments motivated by their own sub-projects will get
their own ADRs (no promiscuous bundling).

Relevant Constitution constraints: G6 (tau is a runtime, not a framework),
G7 (package manager is the only way to add extensions), G9 (observable by
default), G10 (solo + orchestration paths; sub-project 4 is solo only),
G11 (typed addressed messages), G14 (capabilities declared at install AND
enforced at runtime), G16 (overhead bounds), NG6 (no persistent agent
memory), NG9 (tau does not redact for the caller), NG12 (runtime not
framework), QG2 (`thiserror` everywhere), QG3 (panics are bugs), QG5
(parsers earn proptests), QG18 (ADRs for public API + trait changes).

ADR-0006 also notes a small set of mid-implementation **additive
constructors** added during the run-loop work: `Message::new`,
`AgentStatus::failed`, `CompletionRequest::new`,
`LlmProviderMessage::{user, assistant, tool_result}`, `ToolUse::new`,
`TokenUsage::new`, `SessionContext::new`, `PackageId::new`. They were
forced by `#[non_exhaustive]` on those types blocking struct-literal
construction across crates and were registered in the same commit as
Task 10 (the run loop). All are additive (no existing API broken). The
kernel decisions in this ADR depend on those constructors existing.

## Decision

### 1. Pure kernel skeleton scope

Sub-project 4 ships a kernel testable in isolation against tau-ports'
mock plugins (`MockLlmBackend`, `MockTool`, `MockStorage` under the
`test-fixtures` cargo feature). Real plugin packages — concrete
Anthropic / OpenAI backends, real filesystem / shell / HTTP tools — land
in sub-project 5 (tau-cli) and beyond. tau-runtime accepts pre-constructed
plugin instances via the builder; it does not load packages from disk at
v0.1 (no cdylib, no dlopen). The integration tests at
`crates/tau-runtime/tests/run_*.rs` exercise the loop end-to-end against
mocks; sub-project 5 wires the same kernel to real plugins.

Trigger to revisit: when sub-project 5+ lands real plugins and the kernel
needs to surface a serve-mode protocol on top of the embeddable Rust API.

### 2. Async public API

`Runtime::run` and `Runtime::run_default` are `async fn`. tau-ports'
`LlmBackend::complete`, `Tool::invoke`, `Storage::*` etc. are native
`async fn in trait` (per ADR-0003 §1). Wrapping them in a sync facade
would require `tokio::runtime::Handle::current().block_on(...)` at every
call site, which fails outside a running runtime and pushes complexity
onto the kernel for no value. The kernel's library code is async-runtime
agnostic — bare `async fn` and `.await` only, no `tokio::sync`,
`tokio::spawn`, or `tokio::select!`. `tokio` is a dev-dependency only;
callers (typically tau-cli) bring the runtime at the binary level.

Trigger to revisit: a concrete use case where async forces unwanted
complexity on a small embedding.

### 3. Builder pattern construction with name-keyed registries

`Runtime::builder().with_*(...)*.build()` is the public construction path.
The builder accumulates plugin instances under their `name()`; this enables
dispatch by name in messages (`Address::Tool(String)`) and allows
build-time detection of name collisions within a kind. The builder methods
are `with_llm_backend(impl LlmBackend + 'static)`,
`with_tool(impl Tool<Session = ()> + 'static)`, and
`with_storage(impl Storage + 'static)` — generics, NOT `Box<dyn Trait>` —
because the underlying plugin traits are not dyn-compatible (native
`async fn in trait` is not object-safe in Rust 1.93).

The builder boxes through internal wrapper traits (`DynLlmBackend`,
`DynTool`, `DynStorage`) with blanket impls for `T: Trait + 'static`. This
is the standard "box once at the dyn-cast boundary" pattern. The wrapper
types are `pub` so future tau-runtime helpers can refer to them, but they
are **not part of the public API contract** — they are an implementation
detail and will be removed when tau-ports gains a `trait_variant`-generated
dyn-compatible variant.

Build-time validations: at least one LLM backend must be registered;
no name collisions within a kind (two LLM backends, two tools, or two
storages with the same `name()`). Both produce typed `BuildError` variants
(`NoLlmBackend`, `NameCollision { kind, name }`).

Trigger to revisit: when tau-ports gains native dyn-compatibility (the
dyn-shim becomes deletable boilerplate).

### 4. `Session = ()` v0.1 tool limitation

The kernel's tool registry stores `Arc<DynTool>` where the underlying
trait is `Tool<Session = ()>`. Stateless tools (the common case) work
directly; stateful tools must wrap themselves in `tau_ports::StatelessAdapter`
or wait for a future `DynTool` extension once erased associated types
stabilize on stable Rust. Accepting arbitrary `Session` types from external
plugins would require erased-associated-type machinery beyond v0.1's scope.

Trigger to revisit: the first stateful tool that genuinely cannot be
expressed via `StatelessAdapter` — e.g., a long-lived database connection
that must persist across `invoke` calls within one agent run.

### 5. Multi-turn batch loop, not streaming

`LlmBackend::complete` (batch) is the v0.1 surface used by the agent loop.
`LlmBackend::stream` exists in tau-ports (per ADR-0003 §2) but tau-runtime
does not invoke it at v0.1. Streaming integration is purely additive (a
new `Runtime::run_streaming` method) and does not break the batch surface
when it lands.

Trigger to revisit: a streaming-UX-driven use case (TUI rendering tokens
as they arrive, latency-sensitive interactive flows).

### 6. Caller-supplied manifest

`Runtime::run(agent_def, package_manifest, initial_message, options)` takes
the package manifest as a parameter rather than fetching it itself. tau-runtime
has no `tau-pkg` dependency. Callers (tau-cli, in-process embedders) call
`tau_pkg::read_manifest` and feed the result in. The dependency direction
stays crisp: tau-runtime → tau-domain, tau-ports; tau-cli → tau-runtime,
tau-pkg. This keeps tau-runtime embeddable in environments where tau-pkg
is absent (e.g., a server consuming pre-fetched manifests over the wire).

Trigger to revisit: an embedded use case where a caller cannot reasonably
fetch the manifest itself.

### 7. Typed capability enforcement (additive amendment to ADR-0003)

G14 demands runtime enforcement of declared capabilities. The kernel
checks every tool invocation against the agent's package manifest: for
each capability the tool declares as required, at least one capability
in the manifest must satisfy it. Tools must therefore declare what they
require — and the only place to declare it is on the `Tool` trait itself.

The additive amendment to ADR-0003: a new method on `Tool` with a default,

```rust
fn capabilities(&self) -> &[tau_domain::Capability] { &[] }
```

is the minimum disturbance to the trait. Every existing impl (including
the four mocks under `tau_ports::fixtures`) compiles unchanged; new tools
opt in by overriding the method. Backwards-compatible.

The kernel implements the satisfies-relation in
`crates/tau-runtime/src/capability.rs`: variant-by-variant matching
(`Filesystem` only satisfies `Filesystem`, `Network` only satisfies
`Network`, etc.) with glob support for filesystem paths and HTTP hosts
via the inline `glob_matches` helper (no `globset` dependency at v0.1).
Mismatch produces `RunOutcome::Failed { status: AgentStatus::Failed
{ kind: FailureKind::PolicyDenied, .. } }` — never an `Err`. The denial
detail (agent id, package id, tool name, required kind, required detail)
is preserved in the failure message via the `CapabilityDenial` helper
type.

The amendment to ADR-0003 is **bundled** with this ADR because the trait
change is solely motivated by tau-runtime's needs and would not exist
without it. Future tau-ports trait changes motivated by their own
sub-projects will get their own ADRs.

Trigger to revisit: a richer satisfies-relation (semver bounds on hosts,
time-windowed capabilities, regex matchers, etc.).

### 8. Hard-fail on capability denial

A capability mismatch halts the agent immediately with
`AgentStatus::Failed { kind: FailureKind::PolicyDenied, .. }`. v0.1 has no
soft-fail mode (continue without the tool, surface a denial message back
to the LLM, retry with a different tool). Soft-fail is deferred to Phase-1+
via an additive `RunOptions { soft_fail_capability_denial: bool, .. }` or
similar option.

Trigger to revisit: a workflow where partial denials are recoverable
(agent retries with a different tool, or the run continues with the tool
result skipped and a denial message in the conversation).

### 9. Outcome / Error dichotomy

The split:

- `Ok(RunOutcome::Completed { .. })` — agent ran to a terminal text
  response.
- `Ok(RunOutcome::Failed { status, .. })` — agent ran but couldn't
  accomplish the task within the rules: `PolicyDenied` (capability
  mismatch), `OutOfResources` (`max_turns` reached). The agent's run
  itself was well-formed.
- `Err(RuntimeError)` — kernel-level failures: a plugin returned an error,
  dispatch lookup failed, plugin output violates the contract. The kernel
  itself can't continue.

The split makes pattern-matching at the call site clean: callers
distinguish "agent done, now decide what to do with the outcome" from
"kernel broken, log + retry". Forcing kernel-level errors into
`RunOutcome::Failed` would conflate the two.

The `RuntimeError` taxonomy: `LlmBackendNotRegistered { agent_id, backend }`,
`ToolNotRegistered { tool_name, registered }`,
`PluginContractViolation { plugin_kind, plugin_name, what, detail }`,
`Llm(LlmError)`, `Tool(ToolError)`, `Storage(StorageError)`,
`Sandbox(SandboxError)`, `Manifest(PackageManifestError)`,
`Internal { message }` (escape hatch). Each plugin-error variant composes
via `#[from]` for `?`-propagation; the
`PluginContractViolation` variant is wired but not yet trigger-pathed
(see Consequences).

Trigger to revisit: a case where the dichotomy breaks down — i.e. a
"soft" kernel error that should not terminate the run.

### 10. No retries at v0.1

Plugin errors surface via `Err(RuntimeError::*)` and terminate the run.
Callers compose retry logic externally (`tokio::time::timeout`,
exponential backoff, circuit breakers). Building retries into the kernel
commits to a retry strategy before real-world data informs the right one,
and the kernel-as-minimum stance (G6, NG12) discourages baking in policy.

Trigger to revisit: a Phase-1+ retry config that materially helps in a
common case.

### 11. `tracing` for structured logs

Per G9 (observable by default). `tracing = "0.1"` is workspace-pinned.
Callers compose the subscriber (`tracing_subscriber::fmt()` + filter); per
NG9 tau does not redact for the caller.

The kernel emits ~22 events across 9 subsystems on a happy-path run (the
spec originally enumerated ~45; some merged during implementation as
neighboring events collapsed into one). Spans: `runtime.agent_run`,
`runtime.turn`, `llm.complete`, `dispatch.tool`, `capability.check`,
`tool.session_open`, `tool.invoke`, `tool.session_close`. Events:
`runtime.run_started/completed/failed/loop_terminated/max_turns_reached`,
`llm.request_built/response_received/token_usage/stop_reason/tool_use_emitted`,
`dispatch.tool_resolved`,
`capability.required_loaded/granted_loaded/satisfies_check/allow/deny`,
`tool.args_received/result_received/invoke_failed/session_open_failed/session_close_failed`,
`message.added`. Sensitive-data discipline: args + message contents are
previewed (256 chars) only at `DEBUG`; full content only at `TRACE`. API
keys / credentials are never logged by the kernel.

Trigger to revisit: a structural change to the vocabulary (event renames,
span reorganization) — additive changes are non-breaking.

### 12. Per-runtime storage scoping

`Storage` plugins are stateful (open with namespaces, get / put / list /
delete). At v0.1 the runtime registry stores one `Arc<DynStorage>` per
name; agent-instance-scoped namespaces (e.g.
`Namespace { agent_instance: AgentInstanceId, scope: "session" }`) isolate
data between concurrent runs against the same backend. The kernel does
not currently invoke `Storage::*` from the run loop — first-class storage
use by the loop is a Phase-1+ feature; v0.1 ships the registry slot so
tools can resolve storages by name when invoked.

Trigger to revisit: tools that need per-run storage isolation guarantees
beyond what namespaces give.

### 13. Sandbox skipped at v0.1

The `Sandbox` trait remains in tau-ports as a forward-compat anchor (per
ADR-0003 §6 provisional caveat). The kernel never invokes
`Sandbox::create`. Real sandboxing requires OS-level work (Linux
namespaces, seccomp, macOS seatbelt, WASI capabilities, or Firecracker
VMs) that is out of scope for sub-project 4. The
`RuntimeError::Sandbox(_)` variant exists for forward compat and is
unused on every code path at v0.1.

Trigger to revisit: when a real sandbox plugin lands (Phase-1+) or when
an integration demands sandboxed tool execution.

### 14. No `Runtime::shutdown`

`drop` is sufficient. Plugin destructors run, `Arc<dyn Plugin>` reference
counts drop to zero, plugin internals (HTTP clients, file handles) clean
up via their own `Drop` impls. An explicit `shutdown(self)` consuming
method would commit the kernel to a shutdown phase order (drain in-flight
runs, flush logs, close sessions in a known order) before any consumer
needs are known.

Trigger to revisit: a long-running daemon use case where graceful
shutdown ordering matters.

### 15. `max_turns` default = 16

Empirical range for typical agentic loops. Configurable per-run via
`RunOptions::max_turns`. 16 is high enough that simple single-turn
responses + a handful of tool roundtrips fit comfortably; low enough
that a runaway loop terminates in bounded time. Hitting the cap returns
`Ok(RunOutcome::Failed { status: AgentStatus::Failed { kind:
FailureKind::OutOfResources, .. } })` with the partial conversation
preserved.

Trigger to revisit: empirical evidence that the default is wrong (real
agents complete in 2–3 turns and 16 is over-budgeted, or real agents
need 30+ for typical work).

### 16. `all_messages` always included in `RunOutcome`

Both `RunOutcome::Completed` and `RunOutcome::Failed` carry
`all_messages: Vec<Message>` — the full conversation: initial message,
every LLM response, every `tool_use` and `tool_result`. Callers who want
to display, persist, or audit the run get full visibility for free. An
opt-out (`RunOptions { capture_messages: false }` or
`include_full_history: false`) is deferred — at v0.1 the storage cost is
bounded by `max_turns × message_size` and is small.

Trigger to revisit: production scale where opt-out provably helps
(per-call overhead in tight inner loops, memory pressure in long-running
TUI sessions).

### 17. Structured-log vocabulary frozen at v0.1

The ~22 events listed in decision 11 are the v0.1 vocabulary; the
`tests/tracing_emission.rs` integration test asserts a known-good event
set fires on a happy-path run. Tracing event names are **not** considered
public API in the QG18 sense — additive vocabulary changes (new events,
new span fields) are non-breaking. Removal or rename of an event is a
breaking change requiring an ADR.

Trigger to revisit: a structural change (event renames, span
reorganization).

## Consequences

### Positive

- Sub-project 4 ships a working solo-path kernel with 62 unit tests + 7
  integration tests + 3 proptests, exercising every public surface and
  the core agent loop end-to-end against tau-ports' mock plugins.
- The Outcome / Error dichotomy gives callers a clean split between
  "agent done, examine outcome" and "kernel broken, log + retry". One
  `match` at the call site does both.
- `Tool::capabilities()` lands as an additive amendment with zero impact
  on existing impls — the four mocks in `tau_ports::fixtures` and any
  out-of-tree `Tool` impls compile and run unchanged.
- The tracing vocabulary is frozen at v0.1 for callers; additive
  expansion is non-breaking, so vocabulary growth in Phase-1+ does not
  invalidate existing log-analysis pipelines.
- The builder + dyn-shim pattern keeps the public API ergonomic
  (`Runtime::builder().with_tool(MyTool::new())` — no `Box::new(...)`
  boilerplate at call sites) while internally bridging to the
  not-dyn-compatible plugin traits.
- The kernel has no async-runtime dependency — downstream consumers pick
  tokio, async-std, smol, or anything else.

### Negative

- The `DynLlmBackend` / `DynTool` / `DynStorage` shim adds ~250 lines of
  boilerplate (a delegate trait + blanket impl per kind). It is removable
  when tau-ports gains a `trait_variant`-generated dyn-compatible
  variant, but not without breaking the v0.1 internal API surface.
- The `Session = ()` v0.1 limitation forces stateful tools to use
  `StatelessAdapter` — surprising to plugin authors who don't read the
  docs first. The compile error if they don't (`expected
  Tool<Session = ()>`) is at least direct.
- No retries means callers compose them. Consistent with the "kernel
  does the minimum" stance (G6) but expects more from callers than a
  batteries-included runtime would.
- `RuntimeError::PluginContractViolation` is wired (Task 10) but the
  v0.1 implementation does not have a trigger path:
  `deserialize_tool_args` is a passthrough today. Phase-1+ schema
  validation will populate this variant; until then the variant is
  dead code on every observed run.
- Sandbox is skipped at v0.1 — real isolation guarantees come Phase-1+.
  The `RuntimeError::Sandbox(_)` variant is similarly dead code today.

### Neutral / new obligations

- Future tau-runtime public API additions require their own ADRs (QG18).
- The bundled tau-ports amendment (`Tool::capabilities()`) means future
  tau-ports trait changes that are NOT motivated by tau-runtime get their
  own ADRs (don't bundle promiscuously).
- Mid-implementation additive constructors (`Message::new`,
  `AgentStatus::failed`, `CompletionRequest::new`,
  `LlmProviderMessage::{user, assistant, tool_result}`, `ToolUse::new`,
  `TokenUsage::new`, `SessionContext::new`, `PackageId::new`) are now
  part of the public API surface and bound by QG18 going forward.
- The capability satisfies-relation glob matcher accepts `**`, `*`,
  exact strings, and a host-style `*.suffix` form. Other forms
  (`pre*.txt`, `*foo*`, character classes) are unsupported. Adding
  richer glob semantics is a satisfies-relation extension and additive
  (does not invalidate existing capability declarations).
- The `RuntimeError::Internal` and `BuildError::Internal` escape hatches
  are registered in `docs/explanation/escape-hatches.md` per the
  ADR-0002 escape-hatch policy; the CI registry test enforces the
  registration.
- Concurrent calls to `Runtime::run` against a shared `Storage` plugin
  rely on the storage plugin's `Send + Sync` impl and internal
  concurrency handling; the kernel does not serialize runs.

## Alternatives considered

### A. Sync public API with internal `block_on`

Rejected. The plugin traits are async; wrapping them in a sync facade
requires `tokio::runtime::Handle::current().block_on(...)` which fails
outside a running runtime. Forcing a kernel-managed runtime (e.g.,
bundling tokio inside tau-runtime) commits us to one async runtime
forever. Letting the caller pick is more flexible and matches the
runtime-agnostic stance ADR-0003 took for the trait surface.

### B. `Box<dyn LlmBackend>` registration without the dyn-shim

Rejected. `LlmBackend` (and `Tool`, `Storage`) use native
`async fn in trait` (per ADR-0003 §1) which is not dyn-compatible under
Rust 1.93 (compiler error E0038). The straightforward translation of
the spec did not compile. The wrapper-trait shim is the standard
idiomatic workaround until tau-ports gains a `trait_variant`-generated
dyn variant. See decision 3.

### C. tau-pkg dependency in tau-runtime for manifest fetching

Rejected. tau-runtime should run agents, not fetch packages. Tightening
the dependency direction (tau-runtime knows nothing about installation)
keeps tau-runtime embeddable in environments where tau-pkg is absent —
a server consuming pre-fetched manifests over the wire, an in-process
embedding in a desktop app, etc.

### D. Per-call response queue in `MockLlmBackend`

Considered for fixtures support during the integration-test phase.
Rejected for sub-project 4 because integration tests can ship an
inline `ScriptedLlm` struct (see
`crates/tau-runtime/tests/run_with_tool_calls.rs`). The fixture stays
minimal; per-call scripting is a test-helper concern that a future
sub-project can land in tau-ports if patterns demand it.

### E. Outcome-only API without `Err(RuntimeError)`

Rejected. Plugin errors and dispatch errors are kernel-level — they
break the agent's run because the kernel can't continue. Forcing them
into `RunOutcome::Failed` would conflate "agent done" with "kernel
broken" and force callers to pattern-match a tagged union for the
distinction. The `Result` split is clearer at the call site and lets
`?`-propagation work naturally for kernel errors.

### F. Top-level `RuntimeError` umbrella with one variant per kernel error

Already chosen. (This is the mirror image of ADR-0004 §12 where
per-operation enums won.) For tau-runtime, the kernel's operations all
flow through `Runtime::run`, so a single `RuntimeError` enum covering
all kernel-error paths gives callers one type to match. Splitting per
sub-operation (`LlmDispatchError`, `ToolDispatchError`,
`CapabilityCheckError`) would force callers to compose three or four
match arms for one logical "kernel failure" branch.

### G. Skip the additive `Tool::capabilities()` and resolve capabilities at install time only

Rejected. Install-time resolution alone leaves a gap: a tool's required
capabilities can change as the tool ships new versions, but the agent's
manifest is locked at install time. Runtime checking against the current
tool's declarations catches drift between install-time and run-time
state. G14 explicitly requires both halves — declared at install AND
enforced at runtime.

### H. Sandbox enforcement at v0.1

Rejected. Real sandboxing requires OS-level work (Linux namespaces,
seccomp, macOS seatbelt, WASI capabilities, or Firecracker VMs).
Sub-project 4's scope is "agent lifecycle + message passing — solo path"
(ROADMAP row 4); sandboxing belongs to a later sub-project where the
security model is the primary concern. The trait stays in tau-ports as
a forward-compat anchor (ADR-0003 §6).
