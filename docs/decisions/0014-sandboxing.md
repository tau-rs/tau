# ADR-0014: Sandboxing — hexagonal port + Linux native and container adapters

**Status:** Accepted
**Date:** 2026-05-03
**Deciders:** Titouan Lebocq
**Supersedes:** —
**Closes:**
- [ADR-0006](0006-tau-runtime.md) §13 — the v0.1 unsandboxed trust
  posture is now superseded by Layer-4 OS-level enforcement at the
  plugin spawn site. The capability model (declared in tau.toml,
  granted via project override, enforced at the runtime kernel)
  is now backed by real kernel primitives on Linux + container
  isolation cross-platform.
- Constitution G12 — "real OS-level sandboxing for plugin processes"
  is now implementable; activation by default is the next sub-project
  (see Sub-project A in [the followups doc](../superpowers/specs/2026-05-03-sandboxing-followups.md)).
**Amends:** —
**Refines:** [ADR-0002](0002-manifest-format.md) — capability declarations
in `tau.toml` now have a typed `CapabilityShape` companion that adapters
declare support for at probe time.

## Context

Phase 0 + Phase 1 (Tiers 1-3 priorities 1-11) shipped a complete
agent-loop runtime with capability declarations in plugin manifests,
project-level capability narrowing via override, and per-tool
`SessionContext.granted_capabilities` flowing into IPC plugin
processes. But the trust boundary stopped at the kernel layer:
`crate::capability_check` gates which tools the AGENT can invoke; the
plugin process itself runs unsandboxed and could ignore the capability
boundary if it wanted. Constitution G12 reserved this for a future
sub-project.

Tier 3 priority 12 closes the gap. The constraint set:

- **Cross-platform:** tau ships on Linux/macOS/Windows. Linux is the
  CI primary; macOS/Windows must at least compile and probe.
- **Hexagonal:** the runtime should not bind to a specific kernel
  primitive; new sandbox backends (WASM, remote sandboxes, macOS,
  Windows) should be additive.
- **Defense in depth:** the agent's capability check + the per-tool
  deny entries are policy; OS-level sandboxing is the enforcement
  backstop. Both must coexist.
- **Typed evolution:** capability shapes must evolve additively as new
  enforcement primitives appear (landlock V2, network egress, syscall
  arg-filters, etc).

Six discrete decisions follow.

## Decision 1 — Hexagonal port + adapter pattern

**Decision:** add a single `tau_ports::Sandbox` trait that any concrete
sandbox impl satisfies. Adapters live in dedicated workspace crates
(`tau-sandbox-native`, `tau-sandbox-container`, future
`tau-sandbox-macos`, etc). The runtime selects an adapter via a
probe-based chain configured in `<scope>/config.toml`.

**Context:** the alternative was per-platform conditional compilation
inside `tau-runtime` (`#[cfg(target_os = "linux")] use linux_sandbox::*;
#[cfg(target_os = "macos")] use macos_sandbox::*;`). That couples the
runtime to every kernel primitive and makes adapter testing impossible
in isolation.

**Consequences:**
- Each adapter is a separate crate with its own Cargo.toml, deps,
  feature flags, and test surface. This is the same pattern tau already
  uses for `tau-plugins/anthropic`, `tau-plugins/ollama`, etc.
- The trait uses `async fn` (AFIT). AFIT prevents `dyn Sandbox`, so
  the runtime's `SandboxAdapter` enum dispatches to concrete adapters
  via match. The enum implements `Sandbox` itself, so downstream
  callers take `&impl Sandbox` polymorphically.
- Future sub-projects add new adapter crates without touching the
  runtime's selection logic (only the chain config + `instantiate`
  match arm).

**Alternatives considered:**
- Single sandbox crate with `cfg(target_os)` modules — rejected (locks
  in the dispatch model; adapter authors can't test in isolation).
- Trait objects (`Arc<dyn Sandbox>`) — blocked by AFIT.
- Generic-over-`S: Sandbox` everywhere — works for plan validation
  (`validate_plan_against_adapter<S>`) but explodes monomorphization
  cost when the runtime holds the adapter behind `Arc`.

## Decision 2 — Linux native first; macOS, Windows, remote in future sub-projects

**Decision:** v0.1 ships two adapters: `tau-sandbox-native` (Linux
landlock + seccomp + namespaces) and `tau-sandbox-container`
(docker/podman shell-out, cross-platform). macOS sandbox-exec, Windows
AppContainer, and remote backends (Vercel Sandbox, Sandcastle) are
future sub-projects.

**Context:** Linux is tau's primary CI target and where the most mature
unprivileged kernel sandboxing primitives exist (landlock + seccomp +
user namespaces, all stable since kernel 5.13). macOS sandbox-exec
requires libsandbox FFI (poorly documented); Windows AppContainer
requires WinAPI bindings via `windows-rs`. Both deserve their own
sub-projects rather than rushed parity.

**Consequences:**
- Non-Linux hosts probe `Unavailable` from the native adapter; the
  default chain falls back to the container adapter (which works on
  any host with Docker Desktop / Podman installed).
- macOS/Windows users who don't have a container runtime get a clear
  "no sandbox adapter available on this platform" error and exit code
  2 from `tau resolve --check-sandbox`. Refusing to start without
  sandboxing is the correct security posture (see Decision 6).
- Phase 2 includes `tau-sandbox-macos` (sub-project J) and
  `tau-sandbox-windows` (sub-project K) as named follow-ups.

**Alternatives considered:**
- Ship all three platforms together — rejected (would delay the Linux
  case for the harder platforms; landlock/seccomp work standalone).
- macOS via container only — viable today; explicit in the default
  chain. Documented as the intended posture until sub-project J lands.

## Decision 3 — Typed `CapabilityShape` vocabulary, not free-form strings

**Decision:** introduce `pub enum CapabilityShape` in `tau-domain` with
variants for each kernel-level enforcement primitive (`FilesystemRead`,
`FilesystemWrite`, `ProcessExec`, `NetworkHttp`, `AgentSpawn`,
`Custom { name }`). Adapters declare a `CapabilityShapeSet` they
support; the runtime cross-checks plan-required vs adapter-supported
shapes before spawn.

**Context:** the alternative is matching capabilities to enforcement
primitives at every adapter call site via string comparison or
ad-hoc enums. That couples each adapter to the full Capability
hierarchy and forces every adapter to know how (e.g.) `Capability::
Process(Spawn)` and `Capability::Filesystem(Exec)` both reduce to the
same kernel surface.

**Consequences:**
- `Capability::required_shape() -> CapabilityShape` is the single
  abstraction layer. Adapters never inspect `Capability` directly;
  they only ask "do I support this shape?".
- Two capabilities can map to the same shape:
  `Filesystem(Exec)` and `Process(Spawn)` both → `ProcessExec`.
  This deduplicates kernel work.
- New shapes (e.g. `FilesystemTruncate` for landlock V2) land in
  `tau-domain` with `#[non_exhaustive]` evolution. Adapters declare
  support per-shape via `supported_shapes`; unsupported shapes return
  `SandboxError::ShapeUnsupported` cleanly.
- All `Custom` capabilities reduce to `CapabilityShape::Custom { name }`.
  Adapters MAY refuse to sandbox custom shapes; the mock and native
  adapters do, the container adapter rejects via `validate_plan`.

**Alternatives considered:**
- Match on `Capability` directly — rejected (couples every adapter to
  the full domain hierarchy).
- One enum variant per `Capability` variant — rejected (loses the
  deduplication; future shapes become 1:1 with capabilities).

## Decision 4 — Pre-flight validation hierarchy (4 layers)

**Decision:** validation runs at four distinct layers, each catching
errors closer to runtime:

1. **Plugin author build (`cargo build`)** — type-state in the SDK
   (deferred to a future sub-project; `#[capabilities(...)]` proc
   macro is optional polish).
2. **`tau install`** — Layer 2 cross-check between plugin manifest
   and the binary's `CAPABILITIES` handshake response (deferred to
   sub-project B).
3. **`tau resolve --check-sandbox` / `tau check`** — Layer 3 static
   validation: plan-required shapes ⊆ adapter-supported shapes for
   every plugin in the lockfile. Shipped in v0.1.
4. **Plugin spawn** — Layer 4 runtime enforcement: `wrap_spawn` is
   called immediately before fork+exec; the OS sandbox catches
   plugin-vs-declaration drift even if Layers 2-3 missed it.

**Context:** "errors found at Layer 4 (runtime) are expensive — the
plugin already crashed". Pushing each error class to the earliest
layer that can detect it is the project's discipline. Layer 3
specifically lets `tau resolve --check-sandbox` catch every
plan-vs-adapter mismatch in one pass without spawning anything.

**Consequences:**
- Layer 3 implementation: `validate_plan_against_adapter` returns
  `Result<(), Vec<SandboxValidationError>>` — ALL errors, not just
  the first. Single CLI invocation surfaces every problem.
- Layer 4 implementation: `Sandbox::wrap_spawn(plan, &mut Command)`
  validates the plan first, then applies adapter-side wrapping
  (landlock pre_exec hook, container argv rewrite, etc).
- Layer 2 (install cross-check) is documented as future work in
  sub-project B; lockfile schema v3 → v4 added the
  `LockedPlugin.required_shapes` field but the install path
  populates it empty for now (with a `tracing::warn!` migration
  note for v3 lockfiles).

**Alternatives considered:**
- Single Layer-4-only validation — rejected (every error becomes a
  spawn cost; bad plans waste minutes).
- Layer 1 type-state via proc macro at v0.1 — deferred; the SDK
  evolution tradeoff isn't worth it before the lower layers prove
  themselves.

## Decision 5 — Adapter chain with probe-based selection

**Decision:** the scope's `[sandbox]` config defines an ordered
`chain: Vec<SandboxAdapterConfig>`. At runtime, each adapter is
probed in order; the first `Available` adapter whose tier ≥
`minimum_tier` is selected. Empty chain → platform default
(`[native, container]`). Mock adapter is opt-in only.

**Context:** the alternative is per-machine configuration: each user
sets the active adapter explicitly in their scope config. That makes
the same project config produce different sandboxing on different
machines, which violates the "machine-agnostic" principle from the
[tau-as-language vision](../explanation/tau-as-language.md).

**Consequences:**
- The same `<scope>/config.toml` works across Linux dev, Linux CI,
  macOS dev (with Docker), and Windows dev (with Docker Desktop).
  The probe at startup discovers what the local host can offer.
- `minimum_tier` is the security floor: a project that says
  `minimum_tier = "Strict"` will refuse to start if only Light is
  available. This is opt-in stricter posture for projects that need it.
- Adapter probes cache their result behind `OnceCell` so repeated
  spawns don't re-probe.
- The `select_adapter` function returns `NoAdapterAvailable { tried }`
  with per-adapter rejection reasons when no adapter is usable, so the
  user sees "tried: native (kernel < 5.13), container (no docker on
  PATH)" rather than just "no adapter available".

**Alternatives considered:**
- Fixed per-platform default — rejected (can't override; can't add
  Mock for tests; can't compose).
- User picks adapter by name — rejected (works on dev machine but
  breaks CI; loses the machine-agnostic guarantee).
- Auto-probe without config — happens at v0.1 when chain is empty,
  but the user can opt into explicit chain config when they want
  control.

## Decision 6 — Mock adapter is opt-in only, never silent fallback

**Decision:** the `MockSandbox` adapter (which accepts every plan
without enforcement) is admissible only when explicitly listed as
`{ kind = "mock" }` in the scope config. The default chain
(`[native, container]`) does NOT include Mock. When neither native
nor container is available, the runtime returns
`SandboxChainError::NoAdapterAvailable` with exit code 2 — it does
NOT silently fall through to Mock.

**Context:** the security posture difference between "I asked for
sandboxing and got Mock" and "I asked for sandboxing and got an
explicit error" is enormous. A silent Mock fallback means a user
believes their plugins are sandboxed when they aren't.

**Consequences:**
- macOS/Windows users without Docker Desktop see a clear error and
  must either install Docker or explicitly opt into Mock with full
  knowledge of the security implications.
- CLI integration tests need a way to use Mock; this is mediated by
  the env var `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` which gates the
  Mock instantiation in production builds. Documented as a debt
  item (sub-project H) — a dedicated test-only binary would be
  cleaner, but the env-var approach is the v0.1 pragmatic solution.
- The `tracing::warn!` from `unshare_flags_for_plan` (when
  `Network(Http)` is requested and per-host filtering hasn't shipped
  yet) follows the same fail-loud principle: under-enforcement is
  surfaced, not silenced.

**Alternatives considered:**
- Silent fall-through to Mock — rejected (silent under-enforcement
  is the dangerous failure mode this decision exists to prevent).
- Build-time feature flag — rejected (every build that didn't enable
  the flag would silently break tests; runtime opt-in is more honest).
- Refuse Mock entirely outside `cfg(test)` — too restrictive; CLI
  integration tests via `cargo_bin("tau")` need Mock in some form.

## Vision

This sub-project lays the foundation for **tau as a compiled language
for agentic workflows**. See
[`docs/explanation/tau-as-language.md`](../explanation/tau-as-language.md)
for the full vision. The sandboxing port + capability shape vocabulary
+ adapter chain + probe-based machine-agnosticism are the primitives
that future Phase 2 sub-projects build on.

**Phase 2 sub-projects (named in the vision doc):**

- **A.** `tau check` standalone command (~3 weeks). Layer 3 validation
  as a first-class CLI verb, not just a flag on `tau resolve`.
- **B.** Tau target triple registry (~2 weeks). Formal naming +
  documented capability matrix per target.
- **C.** `tau build --target <triple>` + bundle format (~6 weeks).
  Content-hashed deployment artifacts.
- **D.** Capability vocabulary forward-compatibility (~2 weeks).
  Stability discipline for `CapabilityShape` evolution across tau
  major versions.
- **E.** Cross-machine reproducibility verification (~3 weeks).
  Extends `tau verify` to detect bundle tampering between build and
  run.
- **F.** Remote target backends (~4-6 weeks per backend). Vercel
  Sandbox, Sandcastle, generic remote-execution providers.
- **G.** WASM target backend (~12+ weeks). The most ambitious;
  plugins compile to `wasm32-wasip2`.

These are independent of the **immediate follow-ups** documented in
[`docs/superpowers/specs/2026-05-03-sandboxing-followups.md`](../superpowers/specs/2026-05-03-sandboxing-followups.md)
(activation by default, plugin compatibility, e2e CI infrastructure,
per-command exec gating, per-host network filter, fork-server pattern,
macOS / Windows adapters). The followups doc tracks 11 named
sub-projects that close the gaps left by v0.1 of THIS sub-project;
Phase 2 sub-projects A-G build on top of the foundation once the
followups are addressed.

## Notes

- The sandboxing port is now stable (PROVISIONAL warning dropped from
  `tau_ports::Sandbox`); evolution is via `#[non_exhaustive]` on every
  public type.
- Lockfile schema v3 → v4 is additive only. v3 lockfiles continue to
  load with a once-per-process migration warning; the `--rehash` flag
  for explicit refresh is deferred to sub-project B.
- The `[sandbox]` section in `<scope>/config.toml` is opt-in; existing
  scope configs without it use the platform default chain. Schema
  version bumped 1 → 2 (additive; v1 configs auto-upgrade with empty
  `sandbox` section).

## References

- Spec: [`docs/superpowers/specs/2026-05-02-sandboxing-design.md`](../superpowers/specs/2026-05-02-sandboxing-design.md).
- Plan: [`docs/superpowers/plans/2026-05-02-sandboxing.md`](../superpowers/plans/2026-05-02-sandboxing.md).
- Vision: [`docs/explanation/tau-as-language.md`](../explanation/tau-as-language.md).
- Followups: [`docs/superpowers/specs/2026-05-03-sandboxing-followups.md`](../superpowers/specs/2026-05-03-sandboxing-followups.md).
- Trait: [`crates/tau-ports/src/capability_gate/mod.rs`](../../crates/tau-ports/src/capability_gate/mod.rs)
  (renamed from `sandbox.rs` at Phase β.1.1, ADR-0035).
- Adapters: [`crates/tau-sandbox-native/`](https://github.com/titouanlebocq/tau/tree/main/crates/tau-sandbox-native),
  [`crates/tau-sandbox-container/`](https://github.com/titouanlebocq/tau/tree/main/crates/tau-sandbox-container).
- Runtime glue: [`crates/tau-runtime-tokio/src/process_gate/`](https://github.com/titouanlebocq/tau/tree/main/crates/tau-runtime-tokio/src/process_gate)
  (renamed from `tau-runtime/src/sandbox/` at Phase β.1.4).
- Layer 3 CLI: [`crates/tau-cli/src/cmd/resolve.rs`](../../crates/tau-cli/src/cmd/resolve.rs)
  (`--check-sandbox` flag).
