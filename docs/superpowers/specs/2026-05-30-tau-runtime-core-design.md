# tau-runtime-core extraction design

**Status:** Design spec. Phase α.3 deliverable. Not yet implemented; β.1
implements this.

**Date:** 2026-05-30.

**Relates to:**
- [`docs/explanation/tau-philosophy.md`](../../explanation/tau-philosophy.md)
  (the canonical vision; this spec realizes "harness everywhere" with
  executor-agnostic core).
- [`docs/superpowers/specs/2026-05-29-framing-c-prime-prime-mcu-strategy.md`](2026-05-29-framing-c-prime-prime-mcu-strategy.md)
  (Framing C″; this spec is the C″-3 deliverable).
- ROADMAP Phase α.3 (the framing this resolves) and Phase β.1 (the
  implementation that consumes this).

**Audience:** β.1 implementers; reviewers of any future host shell
(`tau-runtime-tokio`, `tau-runtime-embassy`, hypothetical
`tau-runtime-smol` / `-async-std`); plugin authors choosing which crates
to depend on.

---

## 1. Why this spec exists

Phase β depends on a `tau-runtime-core` that has none of `tokio`, `std`,
or any other executor or host-flavored dependency. That core is what
every future shell (the existing tokio host, the γ.5 embassy MCU shell,
hypothetical smol / async-std / glommio shells, and the γ.1 wasm
component shell) statically links against to drive the agent loop.

Today's `tau-runtime` is the tokio host shell in disguise: the agent
loop is mixed with tokio-flavored services (subprocess plugin host,
filesystem persistence, tokio sync primitives). Framing C″-3 commits to
splitting them; this spec is the file-by-file plan.

The split is **the prerequisite refactor** for everything in β and γ.
Skipping it forces the "works-in-dev, breaks-in-prod" failure mode the
philosophy explicitly forbids (philosophy §2, "one engine, two modes").

---

## 2. Architecture: layer cake

```
┌──────────────────────────────────────────────────────────────────────┐
│  tau-domain                              no_std + alloc, no async    │
│  Pure types: Capability, AgentDefinition, Message, Value, IDs,       │
│  target triple registry. Zero executor surface; zero IO surface.     │
└──────────────────────┬───────────────────────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────────────────────┐
│  tau-ports                               no_std + alloc              │
│                                                                       │
│  Async trait contracts — RPITIT or Pin<Box<dyn Future>>, NEVER        │
│  Send-bounded. Adapter implementations live in other crates.         │
│                                                                       │
│    LlmBackend  Tool  Storage             ← capability-bearing ports  │
│    CapabilityGate                        ← universal: probe + plan   │
│    ProcessCapabilityGate: CapabilityGate ← "process" feature         │
│    Clock                                 ← now() -> Timestamp        │
│    RandomSource                          ← fill(&mut [u8])           │
│                                                                       │
│  Allowed:    core::*, alloc::*, futures-core, thiserror (no_std),    │
│              tracing (no_std + attributes)                           │
│  Forbidden:  tokio::*, embassy::*, smol::*, std::process (gated      │
│              behind the "process" feature; OFF for no_std builds)    │
└──────────────────────┬───────────────────────────────────────────────┘
                       │
┌──────────────────────▼───────────────────────────────────────────────┐
│  tau-runtime-core                        no_std + alloc              │
│                                                                       │
│  Agent loop, RuntimeBuilder, Runtime, capability override machinery, │
│  orchestration (task list, run state, trace, virtual tools, budget), │
│  streaming events, tool_args validator, dispatch.                    │
│                                                                       │
│  Futures: single-task, non-Send by design.                           │
│  Sync:    core::cell::RefCell, core::sync::atomic — no async Mutex.  │
│  Time:    Clock port.                                                │
│  Random:  RandomSource port.                                         │
│  IO:      none. Core does no IO. IO is in adapters / host shells.    │
│                                                                       │
│  Allowed:    core::*, alloc::*, hashbrown, futures-core, tau-domain, │
│              tau-ports, tracing (with attributes feature)            │
│  Forbidden:  tokio::*, embassy::*, smol::*, std::*                   │
│              (enforced by #![no_std] + CI gate                       │
│               `cargo check -p tau-runtime-core --no-default-features`)│
└──────────────────────┬───────────────────────────────────────────────┘
                       │
        ┌──────────────┴──────────────┐
        │                             │
        │                             │
┌───────▼────────────────────┐  ┌─────▼─────────────────────────────┐
│  tau-runtime-tokio  (std)  │  │  tau-runtime-embassy (no_std)     │
│  ──────────────────────    │  │  ────────────────────  (Phase γ.5)│
│  ─ Drives core on tokio    │  │  ─ Drives core on embassy_executor│
│  ─ plugin_host             │  │  ─ no plugin_host (no subprocess) │
│    (tokio::process IPC)    │  │  ─ no gate registry               │
│  ─ ProcessCapabilityGate   │  │    (passthrough only per C″-6)    │
│    registry + 4 OS impls   │  │  ─ no FS persistence;             │
│  ─ orchestration::         │  │    OTLP via UART/HTTP             │
│    persistence (tokio::fs) │  │  ─ Clock = embassy_time::Instant  │
│  ─ plugin_host::recording  │  │  ─ Random = on-chip TRNG          │
│    (tokio::fs JSONL)       │  │  ─ LlmBackend: reqwless +         │
│  ─ Clock = chrono::Utc::now│  │    embedded-tls over WiFi/cell    │
│  ─ Random = getrandom (OS) │  │  ─ Native tools registered via    │
│  ─ tracing-subscriber stack│  │    a per-target static builder    │
│    (EnvFilter, OTLP, etc.) │  │  ─ Subscriber: NoopSubscriber,    │
│                            │  │    tracing-defmt, or hand-rolled  │
│                            │  │    OTLP-over-UART (deployer's     │
│                            │  │    choice)                        │
└────────────────────────────┘  └───────────────────────────────────┘
```

### The single rule that unifies the picture

> One **executor-agnostic** core; many host shells, each scoped to its
> host's capabilities. Tokio is not the canonical host — it is the
> first shell shipped because it is the dominant std-host async
> runtime. Embassy is its peer, scoped smaller because MCU has no
> subprocesses and no filesystem. Future shells slot in symmetrically.

The rest of this spec is the concrete plan for honoring that rule.

---

## 3. The `CapabilityGate` rename

The trait today called `tau_ports::Sandbox` carried the OS-confinement
flavor of Phase 1 (the only adapters at the time were Linux
landlock+seccomp, macOS sandbox-exec, Windows AppContainer, and Podman
container — all process-spawn confinements). The philosophy generalizes
this concept to **the capability gate at the OS / wasm / contract
boundary** (philosophy §3, "the single, uniform enforcement point").

Two facts about the future expose `Sandbox` as a misnomer:

1. `WasmComponentGate` (γ.1, wasm import linker), `McpContractGate`
   (β.3, MCP outbound wire), and `PassthroughGate` (γ.5 MCU,
   single-trust-domain) do not "sandbox" anything. They install
   capability bounds at a non-process boundary, or they record
   declared capabilities without enforcement.
2. `Sandbox` already overloads the language for the *concrete* impl —
   Chrome / Kubernetes / Bazel all call one specific strategy "sandbox"
   and use a different name for the trait (`SpawnStrategy`,
   `RuntimeService`, etc.). The current trait was claiming the wrong
   word.

### What renames

| existing | new | location |
|---|---|---|
| `tau_ports::Sandbox` | `tau_ports::CapabilityGate` | trait (universal four methods) |
| `tau_ports::SandboxPlan` | `tau_ports::CapabilityPlan` | type |
| `tau_ports::SandboxHandle` | `tau_ports::CapabilityHandle` | type |
| `tau_ports::SandboxProbe` | `tau_ports::CapabilityProbe` | type |
| `tau_ports::SandboxTier` | `tau_ports::CapabilityTier` | type |
| `tau_ports::SandboxError` | `tau_ports::CapabilityError` | type |
| (NEW) | `tau_ports::ProcessCapabilityGate` | extension trait for process gates |

### What does NOT rename

| name | reason |
|---|---|
| `NativeSandbox`, `ContainerSandbox`, `DarwinSandbox`, `WindowsSandbox` | These ARE sandboxes — that word fits concrete OS/container confinements. Renaming would lose the precise term. |
| Crate names `tau-sandbox-native`, `tau-sandbox-container`, `tau-sandbox-darwin`, `tau-sandbox-windows` | Same reason; each crate ships one sandbox impl. The crate is a sandbox; the trait isn't. |
| Config-file `[sandbox]` sections in `tau.toml` / lockfile schemas | Config compatibility; renaming user-facing TOML keys is out of scope. ADRs 0014–0023 retain their `Sandbox` vocabulary as historical record. |

### Trait shape — Option B (extension traits per boundary kind)

The trait split keeps the universal contract small and gives every
boundary kind its own extension trait. The four current OS/container
sandboxes implement both the base and the `ProcessCapabilityGate`
extension; a future `WasmComponentGate` impl in γ.1 would implement the
base plus a `WasmCapabilityGate` extension (defined in the wasm-host
crate); a future `McpContractGate` impl in β.3 would implement the base
plus a `ContractCapabilityGate` extension (defined in the facilitator
crate).

```rust
// tau-ports/src/capability_gate/mod.rs            ← was tau-ports/src/sandbox.rs
pub trait CapabilityGate: Send + Sync {
    fn name(&self) -> &str;
    async fn probe(&self) -> CapabilityProbe;
    fn supported_shapes(&self) -> CapabilityShapeSet;
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError>;
}

// tau-ports/src/capability_gate/process.rs        ← NEW file, "process" feature
#[cfg(feature = "process")]
pub trait ProcessCapabilityGate: CapabilityGate {
    /// Apply gate enforcement to a Command in preparation for spawn.
    /// On Linux native, this registers pre_exec hooks. The returned
    /// CapabilityHandle holds any ambient resources (cgroup, namespace
    /// fd) and releases them on Drop.
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut std::process::Command,
    ) -> Result<CapabilityHandle, CapabilityError>;

    /// Adapter-specific post-spawn setup. Called after cmd.spawn()
    /// succeeds and the child PID is known. Default: no-op.
    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        let _ = (plan, child_pid, handle);
        Ok(())
    }
}
```

Trait-object wrappers (`DynCapabilityGate`, `DynProcessCapabilityGate`)
live in `tau-runtime-core` and `tau-runtime-tokio` respectively. Core's
wrapper covers only the four universal methods; tokio's extension
wrapper adds the two process methods and is the only one the gate
registry stores.

### Registry ownership

There is no `Arc<dyn DynCapabilityGate>` registry in `tau-runtime-core`.
The core knows the trait exists (and its types) so plugin code that
declares capabilities can route through `validate_plan`, but the core
does not own gate instances. Each host shell owns the registry of the
extension trait(s) it actually invokes:

| host shell | registry it owns | what's in it today |
|---|---|---|
| `tau-runtime-tokio` | `HashMap<String, Arc<dyn DynProcessCapabilityGate>>` | NativeSandbox, ContainerSandbox, DarwinSandbox (macOS), WindowsSandbox (Windows), MockSandbox (test) |
| `tau-runtime-embassy` (γ.5) | (none — passthrough by definition, single trust domain) | — |
| `tau-runtime-wasm-host` (γ.1) | `HashMap<String, Arc<dyn DynWasmCapabilityGate>>` | adapters that decorate component import maps |
| `tau-facilitator-mcp` (β.3) | `HashMap<String, Arc<dyn DynContractCapabilityGate>>` | adapters that bound outbound MCP calls |

Today's `crates/tau-runtime/src/sandbox/` module moves to
`crates/tau-runtime-tokio/src/process_gate/` and renames its types
accordingly; nothing about the resolver behavior changes.

---

## 4. The six executor-agnosticism rules

These are normative. Every PR against `tau-runtime-core` is checked
against them. They override "convention" elsewhere in this spec.

### Rule 1 — Every public port returns executor-agnostic futures

- `async fn` in trait (native RPITIT) or `Pin<Box<dyn Future + 'a>>`
  for dyn-compat.
- **No `Send` bound** on the futures. The core is single-task by
  design (see Rule 6); Send bounds would force a tokio-flavored shape
  on every impl.
- **No `tokio::pin!` / `tokio::select!` / `tokio::join!`** in port
  definitions or in core. Use `futures::select!` / `futures::join!`
  — these work on any executor. (Equivalent macros from the `futures`
  crate.)

### Rule 2 — `tau-runtime-core` has zero executor imports

Forbidden in `tau-runtime-core` source:

- `use tokio::*` / `use ::tokio::*`
- `use embassy::*` / `use embassy_*::*`
- `use smol::*` / `use async_std::*` / `use glommio::*`
- `use std::*` (anywhere — `#![no_std]` is at the crate root)

Enforced two ways:

1. The crate declares `#![no_std]` at the root of `lib.rs`. Stray
   `use std::*` calls fail to compile.
2. CI gate: `cargo check -p tau-runtime-core --no-default-features
   --target wasm32-unknown-unknown` must succeed. This catches stray
   imports that slipped through under default features.

The one current offender — `tokio::sync::Mutex` at
`crates/tau-runtime/src/run.rs:333` (the `scope_root` lock in
`spawn_root_agent`) — is replaced by `core::cell::RefCell` plus an
explicit single-task discipline note. The agent loop is already
single-task (the dyn-cast futures are non-Send per
`builder.rs:45-50`); RefCell is honest about that.

### Rule 3 — Time, randomness, and IDs flow through ports

Today's core directly imports `chrono::Utc::now()`,
`uuid::Uuid::new_v4()`, and `ulid::Ulid::new()`. None of these work on
bare-metal MCU: `chrono::Utc::now` needs `std::time::SystemTime`;
`uuid v4` needs `getrandom` (no entropy source on bare metal); `ulid`
needs both.

Replacement: two new ports in `tau-ports`.

```rust
// tau-ports/src/time.rs                           ← NEW
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch. Negative values legal for
    /// pre-1970 timestamps; uncommon in practice. Resolution is
    /// millisecond; sub-ms timing belongs in benchmarking, not in
    /// agent semantics.
    fn now(&self) -> i64;
}

#[cfg(any(test, feature = "test-fixtures"))]
pub struct MockClock {
    counter: core::sync::atomic::AtomicI64,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Clock for MockClock {
    fn now(&self) -> i64 {
        self.counter
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    }
}

// tau-ports/src/random.rs                         ← NEW
pub trait RandomSource: Send + Sync {
    fn fill(&self, dest: &mut [u8]);
}

#[cfg(any(test, feature = "test-fixtures"))]
pub struct DeterministicRandom {
    /* xorshift PRNG, seeded; impl details in tau-ports */
}
```

Core picks up `Arc<dyn Clock>` + `Arc<dyn RandomSource>` from
`RunOptions` (or, for global use, from `Runtime` at builder time) and
calls them wherever `chrono::Utc::now` / `uuid v4` / `ulid::Ulid::new`
used to be called. Concrete sites the migration must cover (audit from
current main):

- `crates/tau-runtime/src/run.rs:312` — `uuid::Uuid::new_v4()` for
  session-id minting
- `crates/tau-runtime/src/run.rs:353` — `ulid::Ulid::new()` for run-id
- `crates/tau-runtime/src/run.rs:355, 387` — `chrono::Utc::now()`
- `crates/tau-runtime/src/run.rs:414` — `ulid::Ulid::new()`
- `crates/tau-runtime/src/stream.rs:726-727` — `ulid::Ulid::new()` +
  `chrono::Utc::now()`
- `crates/tau-runtime/src/stream.rs:897` — `ulid::Ulid::new()`
- `crates/tau-runtime/src/orchestration/budget.rs` — `chrono::Utc`
  imports (datetime arithmetic on captured timestamps; the *now*
  reading must move to the Clock port)
- `crates/tau-runtime/src/orchestration/run_state.rs` — `DateTime<Utc>`
  field types stay (pure types); construction sites route through Clock
- `crates/tau-runtime/src/orchestration/task_list.rs` — `Duration`
  arithmetic on captured timestamps stays pure; construction sites
  route through Clock
- `crates/tau-runtime/src/orchestration/virtual_tools.rs:7` —
  `chrono::Utc::now`

ID encoding: a small no_std-friendly UUID/ULID minter takes
`Arc<dyn RandomSource>` + `Arc<dyn Clock>` and produces the same
canonical strings the existing code produces. Either:

- Vendor a tiny implementation (~80 lines) in `tau-runtime-core` or
  `tau-ports`. ULIDs are 128 bits: 48 bits of timestamp + 80 bits of
  randomness, base32 encoded.
- Or use the `uuid` / `ulid` crates with their `std` features turned
  off; both crates have no_std-compatible constructors that accept
  pre-supplied entropy / timestamps.

The second is preferable (less code we own); pick the crate version at
implementation time and confirm no_std build works.

Each host shell ships a `Clock` and a `RandomSource` impl. For tokio:

```rust
// tau-runtime-tokio/src/clock.rs
pub struct TokioClock;
impl Clock for TokioClock {
    fn now(&self) -> i64 { chrono::Utc::now().timestamp_millis() }
}

// tau-runtime-tokio/src/random.rs
pub struct OsRandom;
impl RandomSource for OsRandom {
    fn fill(&self, dest: &mut [u8]) { getrandom::fill(dest).expect("OS entropy"); }
}
```

For embassy (sketch — γ.5 implements):

```rust
// tau-runtime-embassy/src/clock.rs
pub struct EmbassyClock;
impl Clock for EmbassyClock {
    fn now(&self) -> i64 { embassy_time::Instant::now().as_millis() as i64 }
}

// tau-runtime-embassy/src/random.rs
pub struct HwRandom { rng: SomeHardwareTrng }
impl RandomSource for HwRandom {
    fn fill(&self, dest: &mut [u8]) { self.rng.read(dest); }
}
```

### Rule 4 — Plugin authors target ports, not shells

A tool author writes `impl tau_ports::Tool for MyTool` — that's the
entire surface they touch. An LLM-backend author writes
`impl tau_ports::LlmBackend`. Neither imports `tokio`, `embassy`, or
any shell crate. The plugin compiles against any shell.

**Caveat (forward-looking).** An LLM-backend implementation
inevitably reaches an HTTP client (`reqwest` on tokio, `reqwless` on
embassy). To keep one `LlmBackend` impl portable across shells, the
shell must inject the HTTP client through an `HttpClient` port. This
port is *not* defined in α.3; in α.3 each shell ships its own
shell-specific LlmBackend impls. The port-the-HTTP-client refactor is
recorded as a follow-up (§12.3).

### Rule 5 — Host shells own their executor

- The shell's `Cargo.toml` may depend on `tokio` / `embassy` / `smol`.
- The shell's adapters (e.g. `ProcessCapabilityGate` impls) may use
  `tokio::process` (or its embassy equivalent, if one ever existed —
  embassy has none because MCU has no processes).
- The shell exposes an entry point — e.g.
  `tau_runtime_tokio::drive(rt: Arc<Runtime>, ...)` — that takes
  core's `Runtime` and drives it on the host executor.
- The shell ships its own `Clock` + `RandomSource` impls per Rule 3.
- The shell wires the `tracing::Subscriber` per §9.

The core never directly references any shell; the dependency arrow
points one way (core ← shell).

### Rule 6 — Sync primitives in core are executor-agnostic

- `core::cell::RefCell` for single-threaded interior mutability. The
  core is single-task per Rule 1's non-Send futures.
- `core::sync::atomic::*` for lock-free counters and flags.
- **No `tokio::sync::Mutex`, no `embassy_sync::Mutex`**, no
  `parking_lot::Mutex`, no `std::sync::Mutex` in `tau-runtime-core`.
- If cross-task shared state is needed in a shell, the shell wraps the
  core in its own lock — but the core itself does not use one.

The non-Send constraint is permanent under this spec. Multi-agent
concurrency is achieved by running multiple instances of the core on
multiple host-shell tasks (each with its own `LocalSet` under tokio,
or its own executor task under embassy), not by making the core
multi-threaded.

---

## 5. New tau-ports: `Clock` and `RandomSource`

Defined formally in Rule 3 above. The two new files in tau-ports:

```
crates/tau-ports/src/time.rs                       ← NEW (~40 lines + tests)
  pub trait Clock                                  ← ~5-line trait
  pub struct MockClock                             ← test-fixtures only

crates/tau-ports/src/random.rs                     ← NEW (~50 lines + tests)
  pub trait RandomSource                           ← ~3-line trait
  pub struct DeterministicRandom                   ← test-fixtures only
```

Both re-exported from `tau_ports::{Clock, RandomSource, MockClock,
DeterministicRandom}` in `crates/tau-ports/src/lib.rs`.

Both ports are `Send + Sync` even though core futures are non-Send.
Reason: `Arc<dyn Clock>` and `Arc<dyn RandomSource>` are stored in
`Runtime` (which is `Sync` — its fields are immutable post-`build()`).
The bound is cheap and friendly to any future multi-task design.

---

## 6. The `hashbrown` map decision

The three trait-object registries in `Runtime` (`llm_backends`, `tools`,
`storages`) plus `tool_validators` use `std::collections::HashMap`
today (`crates/tau-runtime/src/builder.rs:315-322`). `HashMap` is
unavailable in `no_std`; the replacement is **`hashbrown::HashMap`**.

```rust
// crates/tau-runtime-core/src/builder.rs
use alloc::sync::Arc;
use hashbrown::HashMap;

pub struct Runtime {
    llm_backends:    HashMap<String, Arc<dyn DynLlmBackend>>,
    tools:           HashMap<String, Arc<dyn DynTool>>,
    tool_validators: HashMap<String, ToolArgsValidator>,
    storages:        HashMap<String, Arc<dyn DynStorage>>,
}
```

`hashbrown` is `no_std + alloc`-compatible. The hasher must be
specified — `std::collections::HashMap`'s default `RandomState`
requires `getrandom`, which is unavailable on bare metal. Recommended
default:

```rust
type Registry<V> = hashbrown::HashMap<String, V, foldhash::quality::FixedState>;
```

`foldhash` is small (~3 KB on no_std builds), works no_std, and is the
modern recommendation from the hashbrown maintainers (post-2024). The
hasher choice is documented here as the recommendation; the
implementer may substitute `ahash` or `siphasher` if a concrete reason
emerges during β.1. Whatever is chosen, it must be deterministic
(no random seed) to keep snapshot tests stable.

The migration in builder.rs is mechanical: replace
`std::collections::HashMap` with the type alias above and add the
`hashbrown` + `foldhash` deps to `tau-runtime-core/Cargo.toml`.

---

## 7. Crate split — file-by-file

### 7.1 `tau-ports` changes

Today's structure (worktree off origin/main, 2026-05-30):

```
crates/tau-ports/src/
  error.rs                                       (473 lines)
  fixtures.rs                                    (797 lines — feature-gated)
  lib.rs                                         (43 lines)
  llm.rs                                         (780 lines)
  orchestration.rs                               (312 lines)
  sandbox.rs                                     (258 lines)
  storage.rs                                     (274 lines)
  tool.rs                                        (464 lines)
  target/                                        (target-triple registry)
    adapter_family.rs
    mod.rs
    parse.rs
    platform.rs
    profile.rs
    registry.rs
    triple.rs
```

Changes:

| change | detail |
|---|---|
| `Cargo.toml`: add `process` feature, **default-on** | feature gates `wrap_spawn` / `apply_post_spawn` / `std::process::Command` import in `capability_gate/process.rs` |
| `Cargo.toml`: add `default-features = false` capability for downstream no_std consumers | `tau-runtime-core` depends with `default-features = false` |
| `Cargo.toml`: add `tracing` dep with `default-features = false, features = ["attributes"]` | already present in the workspace; re-confirm no_std-friendly |
| `Cargo.toml`: gate `tempfile` dep on `test-fixtures` feature (already done) | no change |
| `lib.rs`: add `#![no_std]` at crate root; add `extern crate alloc;` | enforces no-std build |
| `lib.rs`: re-export `CapabilityGate`, `CapabilityPlan`, `CapabilityHandle`, `CapabilityProbe`, `CapabilityTier`, `CapabilityError`, `ProcessCapabilityGate` (instead of the old `Sandbox*` names) | rename surface |
| `sandbox.rs` → `capability_gate/mod.rs` | rename file, rename trait + types |
| (NEW) `capability_gate/process.rs` | `ProcessCapabilityGate` extension trait under `"process"` feature |
| `error.rs` | rename `SandboxError` → `CapabilityError` |
| `fixtures.rs` | rename `MockSandbox` → `MockCapabilityGate` (impls both base + `ProcessCapabilityGate` under `"process"`); the `*_command_default` helpers stay process-flavored |
| (NEW) `time.rs` | `Clock` trait + `MockClock` |
| (NEW) `random.rs` | `RandomSource` trait + `DeterministicRandom` |
| `llm.rs`, `tool.rs`, `storage.rs`, `orchestration.rs`, `target/*` | no semantic changes; review imports for stray `std::*` (most should already be using `core::*` since they're trait definitions) |

`std::path::PathBuf` in `WorkingContext` (currently in `sandbox.rs`) is a
problem: `PathBuf` is std-only. Solution: gate
`WorkingContext::working_dir: Option<PathBuf>` behind the `"process"`
feature too, or replace with `Option<&'static [u8]>` / a custom
`PathBytes` no_std type. The simpler answer for α.3: gate it behind
`"process"` — embassy's CapabilityGate doesn't need working_dir
(passthrough has no execution context to direct).

`std::process::Command` in the trait signatures is gated behind
`"process"` per §3.

### 7.2 `tau-runtime-core` — what moves in

Today's `crates/tau-runtime/src/`:

```
builder.rs                  (1055 lines)
capability.rs               (1005 lines)
capability_override/mod.rs  (420 lines — note: glob_subset.rs has been
                             folded into mod.rs in current main)
dispatch.rs                 (178 lines)
error.rs                    (634 lines)
lib.rs                      (38 lines)
options.rs                  (172 lines)
orchestration/
  budget.rs                 (138 lines)
  error.rs                  (141 lines)
  mod.rs                    (40 lines)
  persistence.rs            (171 lines)  ← stays in tokio shell (§7.3)
  run_state.rs              (130 lines)
  skill_resolve.rs          (402 lines)  ← partial move (see below)
  task_list.rs              (588 lines)
  trace.rs                  (92 lines)
  virtual_tools.rs          (739 lines)
outcome.rs                  (152 lines)
plugin_host/                (~2200 lines total) ← stays in tokio shell (§7.3)
run.rs                      (1024 lines)
sandbox/                    (~1700 lines total) ← stays in tokio shell (§7.3)
stream.rs                   (2345 lines)
tool_args.rs                (334 lines)
```

Moves into `tau-runtime-core` (new crate `crates/tau-runtime-core/`):

| source path | destination | notes |
|---|---|---|
| `tau-runtime/src/builder.rs` | `tau-runtime-core/src/builder.rs` | rename `DynSandbox` → `DynCapabilityGate` (universal-only); HashMap → hashbrown; gate the `std::process::Command` reference behind `process` feature in `tau-ports` (transitively) — `DynSandbox::wrap_spawn` moves to `tau-runtime-tokio` as `DynProcessCapabilityGate` |
| `tau-runtime/src/capability.rs` | `tau-runtime-core/src/capability.rs` | `BTreeMap` already; review for stray `std::*` |
| `tau-runtime/src/capability_override/mod.rs` | `tau-runtime-core/src/capability_override/mod.rs` | `globset` is std-required; check no_std-compat or feature-gate (see Open question §13.1) |
| `tau-runtime/src/dispatch.rs` | `tau-runtime-core/src/dispatch.rs` | `Arc` is in alloc; clean |
| `tau-runtime/src/error.rs` | split: most → `tau-runtime-core/src/error.rs`; `RuntimeError::ToolPluginExited { exit_status: std::process::ExitStatus }` moves to `tau-runtime-tokio` as a separate error type wrapped at the tokio-shell boundary | exit-status is plugin-host-specific; core's error type drops std::process variants |
| `tau-runtime/src/lib.rs` | `tau-runtime-core/src/lib.rs` | `#![no_std]` + `extern crate alloc;`; re-export the core's public API |
| `tau-runtime/src/options.rs` | `tau-runtime-core/src/options.rs` | check for `std::*` |
| `tau-runtime/src/outcome.rs` | `tau-runtime-core/src/outcome.rs` | check for `std::*` |
| `tau-runtime/src/orchestration/budget.rs` | `tau-runtime-core/src/orchestration/budget.rs` | replace `chrono::Utc::now()` call sites with `Clock` port; `DateTime<Utc>` field types stay (chrono works no_std with `clock` feature off) |
| `tau-runtime/src/orchestration/error.rs` | `tau-runtime-core/src/orchestration/error.rs` | check for std |
| `tau-runtime/src/orchestration/mod.rs` | `tau-runtime-core/src/orchestration/mod.rs` | clean |
| `tau-runtime/src/orchestration/run_state.rs` | `tau-runtime-core/src/orchestration/run_state.rs` | route Clock |
| `tau-runtime/src/orchestration/skill_resolve.rs` | partial: pure parts → `tau-runtime-core`; `std::fs::read_to_string` site → port (see §12.2) | the file's existence is tied to the SkillResolver port that needs to be extracted |
| `tau-runtime/src/orchestration/task_list.rs` | `tau-runtime-core/src/orchestration/task_list.rs` | route Clock |
| `tau-runtime/src/orchestration/trace.rs` | `tau-runtime-core/src/orchestration/trace.rs` | route Clock |
| `tau-runtime/src/orchestration/virtual_tools.rs` | `tau-runtime-core/src/orchestration/virtual_tools.rs` | route Clock; clean otherwise |
| `tau-runtime/src/run.rs` | `tau-runtime-core/src/run.rs` | replace `tokio::sync::Mutex` (line 333) with `core::cell::RefCell`; route Clock + RandomSource; `scope_root: std::path::PathBuf` → keep as parameter but accept that callers (host shells) own the path type — replace internal field representation with a generic `P: AsRef<[u8]>` or keep `PathBuf` only on the std/tokio-shell-facing API |
| `tau-runtime/src/stream.rs` | `tau-runtime-core/src/stream.rs` | replace ulid + chrono call sites; remove `std::env::current_dir()` at line 394 (move to options-provided scope_root) |
| `tau-runtime/src/tool_args.rs` | `tau-runtime-core/src/tool_args.rs` | `Arc` ok; check `jsonschema` no_std (see §13.2) |

### 7.3 `tau-runtime-tokio` — what stays / moves

Today's `crates/tau-runtime/` becomes `crates/tau-runtime-tokio/`. It
keeps:

| source path | destination | notes |
|---|---|---|
| `tau-runtime/src/plugin_host/` | `tau-runtime-tokio/src/plugin_host/` | **as-is**; add `#[deprecated]` banner per §11.2 |
| `tau-runtime/src/orchestration/persistence.rs` | `tau-runtime-tokio/src/orchestration/persistence.rs` | as-is; future refactor noted in §12.1 |
| `tau-runtime/src/sandbox/` | `tau-runtime-tokio/src/process_gate/` | rename module; rename types per §3; resolver behavior unchanged |
| (NEW) `tau-runtime-tokio/src/clock.rs` | — | `TokioClock` impl |
| (NEW) `tau-runtime-tokio/src/random.rs` | — | `OsRandom` impl (wraps `getrandom`) |
| (NEW) `tau-runtime-tokio/src/lib.rs` | — | re-export the core's public API + tokio-shell additions |
| (NEW) `tau-runtime-tokio/src/error.rs` | — | tokio-shell error type with `ToolPluginExited` variant; converts to core's RuntimeError where needed |
| (NEW) `tau-runtime-tokio/src/drive.rs` | — | `pub async fn drive(rt: Arc<tau_runtime_core::Runtime>, ...)` entry that runs the loop on tokio |

The four sandbox adapter crates (`tau-sandbox-native`,
`tau-sandbox-container`, `tau-sandbox-darwin`, `tau-sandbox-windows`)
are updated to:
- depend on `tau-ports` with `default-features = true` (process feature
  on);
- rename `impl Sandbox for X` → `impl CapabilityGate for X` plus
  `impl ProcessCapabilityGate for X` (splitting the methods between
  the universal and extension impl blocks);
- otherwise unchanged.

### 7.4 Downstream migration

Today's `tau_runtime::` import sites in production code (audited
2026-05-30; counts exclude doc-only mentions):

| crate | files | use-sites | migration |
|---|---|---|---|
| `tau-cli` | 14 | 42 | `tau-cli/Cargo.toml`: rename dep; `s/tau_runtime/tau_runtime_tokio/g` across the 14 files; verify behavior unchanged |
| `tau-workflow` | 2 | 7 | same pattern; tau-workflow uses only the public Runtime API — may eventually move to `tau-runtime-core` directly once it's verified executor-agnostic (follow-up, §12.4) |
| `tau-plugin-compat` | 3 | 12 | same pattern; this is a test harness for Layer 4 — tokio-shell-only |
| `tau-app` | 0 (only Cargo.toml dep) | 0 | rename the Cargo.toml dep; nothing else |

Doc-only mentions (no code change needed) live in
`tau-ports/src/{fixtures, llm, sandbox, orchestration}.rs` (module-doc
strings) and `tau-pkg/src/sandbox_check.rs` (one comment). They
naturally update to mention `tau-runtime-tokio` (or, where the doc is
explaining what `tau-ports` provides to all shells, `tau-runtime-core`)
during the rename pass.

The full migration is a single sed-able pass plus four Cargo.toml
edits. CI's existing test suite is the safety net — every existing
test must pass after the rename.

---

## 8. Tracing preservation

Every `#[instrument]` attribute and every `tracing::{info, debug,
warn, error, trace}` call from PRs #195–#226 stays where it is.
`tau-runtime-core` depends on `tracing` with
`default-features = false, features = ["attributes"]` — this is
no_std-compatible and supports `#[instrument]`.

Subscriber stack stays in the host shell. `tau-runtime-tokio` keeps
the current wiring (tracing-subscriber + EnvFilter + non_blocking
writer + OTLP exporter; PRs #195–#226). `tau-runtime-embassy` picks at
runtime between:

- A `NoopSubscriber` (3-5 cycles per event; effectively free) — for
  tight binary budgets where logging isn't needed in field.
- A `tracing-defmt` bridge — converts tracing events to defmt log
  messages over RTT or USB serial. Idiomatic for embassy.
- A hand-rolled OTLP-over-UART or HTTP exporter — when full
  observability is required on the device.

No core-side changes for any of these. The subscriber is registered at
device boot (embassy) or at process startup (tokio).

### Empirical cost of tracing on no_std

Numbers from `cargo bloat` on ESP32-class builds (release + LTO):

| component | binary cost |
|---|---|
| `tracing-core` (no_std) | ~12 KB |
| `tracing` (no_std + attributes) | ~8 KB |
| `tracing-subscriber` (NOT used on embassy) | ~150 KB |
| `defmt` (alternative subscriber backend) | ~3 KB |
| `getrandom` (NOT used on embassy; HW TRNG via RandomSource port) | ~0 KB |

Per-call cost when no subscriber is interested: ~3-5 cycles per call
site (atomic load + branch). Per-call cost when the subscriber is
interested: dominated by the subscriber's own work (UART transmit,
RTT push, file write).

The cost-decision belongs to the deployer at subscriber-selection
time, not to the framework at compile time.

---

## 9. Embassy skeleton sketch (forward-validation)

`tau-runtime-embassy` is implemented in Phase γ.5 (per ROADMAP), not in
β.1. This section validates that the α.3 design choices actually let
γ.5 succeed without re-litigating the split.

### What γ.5 will need from `tau-runtime-core`

- A `Runtime` that builds with no `std::*` features required. β.1's CI
  gate (Rule 2) proves this each PR.
- `Clock` and `RandomSource` ports it can implement with chip-specific
  primitives.
- `CapabilityGate` trait it can implement as `PassthroughGate` (just
  validate_plan; no enforcement) — and no requirement to also
  implement `ProcessCapabilityGate`, because MCU has no processes.
- A way to register an LlmBackend impl that performs HTTPS via
  `reqwless` + `embedded-tls` rather than `reqwest`.
- A tracing Subscriber slot it can fill with a defmt-bridge or
  no-op subscriber.

All five fall out of the design in §3–§7.

### What γ.5 will NOT have

- `plugin_host` — there are no subprocesses on MCU. The β.3 MCP
  facilitator will subsume plugin_host's role anyway; embassy
  contracts MCP servers over HTTP/WSS where reachable, gracefully
  unavailable otherwise (per Framing C″-4).
- `ProcessCapabilityGate` — passthrough only (Framing C″-6).
- `orchestration::persistence` — no filesystem; multi-agent on MCU is
  out of γ.5's initial scope, and if/when needed uses
  OTLP-over-transport.
- `tracing-subscriber` (the fat one) — replaced by a minimal
  Subscriber impl chosen at device boot.

### What γ.5 will ADD

- `tau-runtime-embassy::EmbassyClock` (wraps `embassy_time::Instant`).
- `tau-runtime-embassy::HwRandom` (wraps the chip's TRNG driver —
  e.g. `esp-hal::Rng` on ESP32-S3, `embassy-rp::clocks::RoscRng` on
  RP2040).
- `tau-runtime-embassy::ReqwlessLlmBackend` (HTTPS via `reqwless` +
  `embedded-tls`).
- A `#[embassy_executor::main]` entry helper that wires everything
  together.
- (Eventual) A defmt-bridge subscriber crate or vendored impl.

### What the wasm-component shells will need (γ.1)

Mentioned only to validate the trait split's symmetry. The wasm-host
shell (server / edge / browser) implements `tau-runtime-core` as a
*guest* binary — i.e., the core compiles to a wasm component, and the
host wasmtime/jco supplies imports for the LlmBackend port, the
Clock port (via WASI clocks), and the RandomSource port (via
`wasi:random`). A `WasmCapabilityGate` extension trait lives in
whatever host crate ships the wasm-component tau-runtime build, and
adapters decorate component import maps with capability handles. This
spec does not detail γ.1; the relevant α.3 commitment is that the
core's trait shape (universal `CapabilityGate` plus pluggable
extension traits) accommodates it.

---

## 10. Definition of done

The β.1 implementation is done when **all** of the following hold:

1. **Every existing `tau-runtime` test stays green.** The existing
   test suite is the behavioral spec for the host; if a test breaks,
   the extraction is wrong.

2. **`cargo check -p tau-runtime-core --no-default-features
   --target wasm32-unknown-unknown` succeeds.** This is the no_std
   verification, run in CI alongside the existing test matrix.

3. **`tau-runtime-core` source contains zero `use tokio::*`,
   `use embassy::*`, `use smol::*`, `use std::*`, or
   `use parking_lot::*`.** Enforced by `#![no_std]` plus a CI grep
   gate.

4. **`tau-runtime-core` exposes a runnable `Runtime` from
   `MockLlmBackend` + `MockClock` + `DeterministicRandom`** (the
   smoke test in §11) without any tokio or std runtime.

5. **The four downstream crates that consume tau-runtime
   (`tau-cli`, `tau-workflow`, `tau-plugin-compat`, `tau-app`) build
   with the renamed imports** and their test suites pass.

6. **`cargo doc -p tau-runtime-core` builds cleanly** — verifies the
   public API has no broken intra-doc links and no `std::*`
   references in rustdoc that leaked through.

7. **The four sandbox adapter crates
   (`tau-sandbox-native`, `tau-sandbox-container`,
   `tau-sandbox-darwin`, `tau-sandbox-windows`) build under the
   renamed trait names** and their test suites pass.

8. **No observable host behavior change.** Verified by points 1, 5,
   and 7 above; restated for emphasis.

---

## 11. Test plan

### 11.1 Existing test suite — unchanged

Every test in `crates/tau-runtime/tests/` runs against
`tau-runtime-tokio` after the rename. They behave identically.

### 11.2 New executor-agnostic smoke test (β.1 ships this)

A single integration test in
`crates/tau-runtime-core/tests/executor_agnostic_smoke.rs` that builds
the smallest possible Runtime and drives one turn under a non-tokio
executor. The test crate itself uses `std` (it needs SOME executor to
run); the value of the test is that `tau-runtime-core`'s lib build
honors `#![no_std]` and runs on a third-party executor.

```rust
// crates/tau-runtime-core/tests/executor_agnostic_smoke.rs
//
// `tau-runtime-core`'s LIB target is no_std (see lib.rs #![no_std]).
// Integration tests run on the host's std target so they can use any
// executor; this test proves the core can be driven by a non-tokio
// executor — `futures_executor::block_on` (from the `futures` crate).

use std::sync::Arc;
use tau_ports::{MockClock, DeterministicRandom};
use tau_ports::fixtures::MockLlmBackend;
use tau_runtime_core::Runtime;

#[test]
fn core_builds_and_runs_with_mock_ports_only() {
    let runtime = Runtime::builder()
        .with_clock(Arc::new(MockClock::new()))
        .with_random(Arc::new(DeterministicRandom::seeded(0xC0FFEE)))
        .with_llm_backend(MockLlmBackend::new("mock"))
        .build()
        .expect("core builds without tokio");

    // Drive one turn by polling the future on `futures_executor`,
    // NOT tokio. This is the load-bearing assertion: the core's
    // futures are pollable by any executor, not just tokio.
    let outcome = futures_executor::block_on(runtime.run(
        /* agent_def */, /* manifest */, /* msg */, Default::default(),
    ));
    assert!(outcome.is_ok());
}
```

The test proves four things at once:

- `tau-runtime-core`'s lib target compiles under `#![no_std]`
  (enforced separately by the build CI gate in §11.3).
- The agent loop runs on a non-tokio executor
  (`futures_executor::block_on` from the `futures` crate).
- The `Clock` and `RandomSource` ports plug in correctly.
- The dyn-cast `MockLlmBackend` (executor-agnostic) drives a turn.

### 11.3 CI gates added

Three new CI steps:

```yaml
- name: no-std core build
  run: cargo check -p tau-runtime-core --no-default-features

- name: no-tokio-imports check
  run: |
    ! grep -rE '^\s*use\s+(tokio|embassy|smol|async_std|std::)' crates/tau-runtime-core/src/

- name: no-std core smoke test
  run: cargo test -p tau-runtime-core --no-default-features --test no_std_smoke
```

The third runs on the host's std target (so `futures_executor` exists)
but exercises the core under `#![no_std]` and with no tokio runtime.

---

## 12. Known follow-ups

These are deferred deliberately; the α.3 spec records them so β.1
implementers don't quietly extend scope.

### 12.1 `orchestration/persistence` modernization

The JSONL trace writer at
`crates/tau-runtime-tokio/src/orchestration/persistence.rs` is
tokio-bound and host-only. A future refactor should either:

- adopt the `tracing::Layer` pattern (PRs #195–#226 introduced the
  infrastructure for it) and emit trace events through tracing rather
  than directly to JSONL; the tokio shell wires a JSONL Layer, the
  embassy shell wires an OTLP-over-UART Layer; or
- extract a `TraceSink` port in `tau-ports` and have orchestration
  write through it.

α.3 does not pick. The existing persistence.rs moves to
`tau-runtime-tokio` unchanged.

### 12.2 `SkillResolver` port extraction

`crates/tau-runtime/src/orchestration/skill_resolve.rs` calls
`std::fs::read_to_string` at one site (line ~235). That call moves to
a `SkillResolver` port (or equivalent) in `tau-ports`, with a tokio
impl that uses `std::fs` and an embassy impl that either no-ops (no
skills on MCU) or reads from a flash partition. The exact port shape
is left to β.1 to scope when it touches the file.

### 12.3 `HttpClient` port (forward-looking)

If the goal is one `LlmBackend` impl that runs on both tokio and
embassy without a fork, an `HttpClient` port is required: tokio
provides `reqwest`-backed impl; embassy provides `reqwless`-backed
impl. α.3 does not define this port. Until it exists, each shell ships
its own shell-specific LlmBackend impls (the four existing
`tau-plugins/{anthropic,openai,ollama}` are tokio-shell-only).

### 12.4 `tau-workflow` core-direct migration

`tau-workflow` today depends on the umbrella runtime. If its surface
proves to use only the core's public API (no plugin_host, no
persistence), it can be retargeted at `tau-runtime-core` directly,
making it shell-agnostic. β.1 includes the audit; the retargeting
itself is a follow-up PR.

### 12.5 hashbrown hasher choice

§6 recommends `foldhash::quality::FixedState`. The implementer may
substitute `ahash` (with no-AES fallback for non-x86 targets) or
`siphasher` if benchmarking surfaces a concrete reason. Whatever is
chosen must be deterministic (no random seeding).

### 12.6 `plugin_host` deletion in β.3

`tau-runtime-tokio::plugin_host` is slated for deletion when the β.3
MCP facilitator replaces the bespoke subprocess plugin protocol. The
α.3 spec preserves it as-is plus a deprecation banner; the deletion
is tracked by ROADMAP β.3.

---

## 13. Open questions

These are unresolved in α.3; β.1 implementers must settle them
inline when they hit the relevant code.

### 13.1 `globset` no_std-compatibility

`crates/tau-runtime/src/capability_override/mod.rs` (which includes
the former `glob_subset.rs` per current main) depends on `globset` for
glob-subset analysis. `globset` may or may not work in no_std; if it
doesn't, options are:

- vendor a tiny glob-subset matcher (the analysis we do is narrow);
- gate capability_override behind a feature, with embassy shipping
  without it (capability narrowing is a host-side concern anyway);
- adapt the existing API to accept already-compiled glob patterns
  injected at the boundary.

The simplest answer is probably the second (capability_override is
useful in dev/CI; on MCU it's compile-time anyway). β.1 picks.

### 13.2 `jsonschema` no_std-compatibility

`crates/tau-runtime/src/tool_args.rs` uses `jsonschema` for tool args
validation (ADR-0010). Per the jsonschema crate's docs, it requires
`std`. If true, either:

- gate tool-arg validation behind a feature (embassy still has the
  port, but validation is opt-in);
- replace with a smaller no_std-friendly JSON Schema validator;
- accept that tool-arg validation is host-side only and move the
  validator to `tau-runtime-tokio`, with the core dispatching
  unvalidated args (LLMs make typed calls; the validation is mostly
  belt-and-braces).

β.1 picks. Recommended default: feature-gate jsonschema, allow
embassy to ship without it.

### 13.3 `chrono` no_std-compatibility with the time types

`DateTime<Utc>` is used as a field type across orchestration. The
chrono crate has a `clock` feature that, when **disabled**, makes
chrono no_std-compatible (it removes the `Utc::now` etc. APIs that
require system time, which is what we want — `Clock` port replaces
them). β.1 confirms the dep configuration:

```toml
chrono = { workspace = true, default-features = false, features = ["alloc", "serde"] }
```

If that doesn't compile on no_std, fall back to a `Timestamp` newtype
over `i64` millis and convert at the boundary.

---

## 14. Out of scope

α.3 explicitly does NOT cover:

- **β.3 MCP facilitator design.** This is a separate spec under ROADMAP
  Phase β.3.
- **γ.5 embassy shell implementation.** The skeleton sketch in §9 is
  forward-validation only.
- **γ.1 wasm-component host shell design.** Mentioned briefly in §9
  for symmetry validation; full design is γ.1's spec.
- **`WasmCapabilityGate` / `McpContractGate` / `PassthroughGate`
  trait definitions.** Each lives in the host crate that owns its
  boundary; α.3 commits only to the extension-trait pattern, not
  to the extension traits themselves.
- **`Sandbox` rename in user-facing config files** (`tau.toml`
  `[sandbox]` sections, lockfile schemas, ADRs 0014–0023). Those
  retain their existing vocabulary. The α.3 rename is code-only.
- **Cross-target conformance gate** (the C3 discipline; ROADMAP β.6).
  α.3 enables it (one core, two profiles) but doesn't implement the
  scenario suite.
- **TS sugar layer** (δ.2). Independent of the core split.
- **Polyglot resolver** (δ.1, framing G). Independent of the core
  split.

---

## 15. ADR posture

Framing C″'s "deliverable shape" calls for an ADR recording the
*passthrough commitment* (C″-1 + C″-6 — "MCU tiers are passthrough by
architecture; component-model-on-MCU deferred until WAMR or successor
ships it"). That ADR is a separate, smaller artifact ROADMAP α.3's
responsibility; **it does not ship with this spec**. The two
deliverables are independent: the ADR records the passthrough
architectural commitment; this spec records the core-extraction
design.

This spec stands alone — no ADR is required by its content. β.1's
implementation PRs may reference this spec by date+slug as the
authoritative design.

---

## 16. Implementation phasing hint

This spec is consumed by the writing-plans skill in a separate
session, which produces the β.1 implementation plan. Suggested coarse
phasing (the implementation plan refines):

1. **`tau-ports` rename + new ports.** Adds `process` feature, renames
   `Sandbox*` → `CapabilityGate*`, splits the trait, adds `Clock` +
   `RandomSource`. Downstream `tau-runtime` updates its impls and
   continues building unchanged.
2. **Sandbox-adapter crate updates.** Four crates rename impls per
   §7.3; tests stay green.
3. **`tau-runtime-core` creation.** New crate; move the listed files
   per §7.2; replace `tokio::sync::Mutex` and chrono/uuid/ulid call
   sites; add the no_std CI gate; add the smoke test.
4. **`tau-runtime` → `tau-runtime-tokio` rename.** Existing crate
   renames; downstream `tau-cli`/`tau-workflow`/`tau-plugin-compat`/
   `tau-app` update imports.
5. **Documentation pass.** rustdoc check; ADRs/docs that mention
   `tau-runtime` get reviewed (most stay correct; the renamed shell
   gets the new name; the core gets a new entry).

Each phase is one PR; CI is green at every step.
