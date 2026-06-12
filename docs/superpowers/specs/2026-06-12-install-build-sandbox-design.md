# Install-time build sandbox (audit finding S2)

**Date:** 2026-06-12
**Audit finding:** S2 — `tau install` builds and executes untrusted code from
arbitrary git URLs with no sandbox (severity: Medium).
**Status:** design approved, pre-implementation.

## Problem

`tau install <source>` clones an arbitrary git URL, runs `cargo build
--release` inside the clone, and afterwards spawns the freshly-built binary for
the Layer-2 capability cross-check. None of these steps run under a sandbox:

- `crates/tau-pkg/src/install.rs` — `Git::clone` of caller-supplied source.
- `crates/tau-pkg/src/install.rs` `build_rust_cargo_plugin` — `cargo build`
  executes the package's `build.rs` and proc macros at build time.
- `crates/tau-pkg/src/sandbox_check.rs` `cross_check_plugin_capabilities` —
  spawns the produced binary.

So `tau install <url>` is remote-code-execution-by-design: a malicious package
compromises the host at install time, before any capability or Layer-4 sandbox
enforcement applies.

## Constraint that shapes the whole design

The obvious fix — "call the runtime's sandbox from the installer" — is
**impossible as a direct dependency**:

```
tau-runtime-tokio/Cargo.toml  →  tau-pkg = { workspace = true }
```

`tau-runtime-tokio` (which owns `resolve_adapter`, the `SandboxAdapter` enum,
and `ProcessCapabilityGate::wrap_spawn`) **already depends on `tau-pkg`** (it
needs `tau-pkg::scope::SandboxRequirements`). Making `tau-pkg` depend on
`tau-runtime-tokio` is a hard cargo cycle.

What *is* reachable from `tau-pkg`: `tau-ports` (already a dependency), which
owns the `ProcessCapabilityGate` trait and the `CapabilityPlan` /
`Capability` types. But `ProcessCapabilityGate` uses `async fn in trait` and
is **not dyn-compatible** — that is exactly why the runtime models adapters as
a hand-rolled `SandboxAdapter` enum rather than `dyn`.

This forces a ports-and-adapters inversion, which is the hexagonally-correct
shape anyway: **the consumer (`tau-pkg`) defines the narrow port it needs; the
layer above (`tau-cli` / `tau-runtime-tokio`) implements it and passes a
concrete gate down.** The dependency arrow points downward — no cycle.

## Architecture

```
 tau-pkg  (port owner — no new upward dependency)
 ─────────────────────────────────────────────────────────────────────────
   trait InstallSandbox {                       ← sync, &dyn-safe port
       fn wrap(&self,
               plan: &CapabilityPlan,
               cmd: &mut std::process::Command)
           -> Result<InstallSandboxGuard, InstallSandboxError>;
   }

   build_envelope(package_dir, manifest) -> CapabilityPlan   ← pure fn
   cross_check_envelope()                -> CapabilityPlan   ← pure fn

   install_with_options(.., options { sandbox: Option<Arc<dyn InstallSandbox>>,
                                       allow_unsandboxed_build: bool })
       cargo build       ─► wrap(build_envelope,      &mut cmd) ─► spawn
       cross-check spawn  ─► wrap(cross_check_envelope, &mut cmd) ─► spawn
                                       ▲
                                       │ implements & injects (arrow points DOWN)
 tau-cli / tau-runtime-tokio           │
 ─────────────────────────────────────┴───────────────────────────────────
   struct RuntimeInstallSandbox(SandboxAdapter)
   impl InstallSandbox {
       fn wrap(..) = bridge to ProcessCapabilityGate::wrap_spawn (sync→async),
                     guard owns any egress-proxy task
   }
   // wired into install at the `tau install` call site
```

### Why the port is sync

`ProcessCapabilityGate::wrap_spawn` is async (the strict adapters start an
egress proxy task, etc.) and the enum is not dyn-safe. `tau-pkg`'s build path
is synchronous (`cmd.output()`), and the cross-check already bridges to async
via `block_on_in_fresh_thread`. Keeping `InstallSandbox` **sync** means:

- it is trivially `dyn`-safe (so `InstallOptions` can hold
  `Arc<dyn InstallSandbox + Send + Sync>`),
- the sync→async bridge and the proxy-task lifetime live entirely in the
  `tau-cli` adapter, not in the `tau-pkg` port,
- the underlying sandbox mechanism (landlock / seccomp / `unshare` /
  `sandbox-exec`) is applied through `pre_exec` hooks on the
  `std::process::Command`, which is itself synchronous syscall registration.

The `tokio::process::Command` used by the cross-check exposes `as_std_mut()`,
so the same sync port wraps both spawn sites.

The currency type is `tau_ports::capability_gate::CapabilityPlan` (already a
`tau-pkg` dependency), so the adapter forwards `Plan → wrap_spawn` with no
translation.

## Capability envelopes

The build and the cross-check have structurally different needs, so they get
**two fixed profiles**, each a pure function of the package — **not** the
plugin's manifest `[[capabilities]]` (those describe run-time needs; a build's
needs are different in kind).

| | net | fs.write | fs.read | child exec |
|---|---|---|---|---|
| **build** | registry hosts **+ git-dependency hosts parsed from `Cargo.toml`** | `target/` dir, `CARGO_HOME` caches, `TMPDIR` | source tree, `CARGO_HOME`, `RUSTUP_HOME`, system libs | **allowed** (cargo → rustc → cc → `build.rs` is the whole point) |
| **cross-check** | none | none | none (stdio handshake only) | none |

- **Build envelope** still lets `build.rs` run arbitrary code, but it cannot
  read `~/.ssh`, cannot write outside `target/`, and cannot exfiltrate anywhere
  except hosts the package *openly declares*.
- **Cross-check envelope** is the strictest: a well-behaved plugin needs only
  stdin/stdout to handshake. A malicious binary that tries to phone home or
  touch the filesystem during cross-check is blocked. (The adapter's baseline
  still grants whatever a dynamically-linked binary needs to `exec`; the
  *user* capability set is empty.)

### Network scope: registry + manifest-declared git hosts

The build's network allowlist is:

1. The crates.io sparse-registry hosts (`index.crates.io`, `static.crates.io`).
2. Plus the hosts extracted from `git = "..."` URLs in the package's
   **top-level** `Cargo.toml` `[dependencies]` and `[build-dependencies]`
   tables.

This builds real-world crates (Example 2 below) while keeping the allowlist
manifest-derived — an attacker cannot reach a host the package does not name.

Worked examples:

- *Normal package* (`serde`, `tokio` from crates.io): registry hosts suffice →
  builds.
- *Git-dependency package* (`foo = { git = "https://github.com/x/foo" }`):
  `github.com` is added to the allowlist because the manifest names it →
  builds.
- *Malicious `build.rs`* that reads `~/.ssh/id_rsa` and POSTs to
  `evil-attacker.com`: the fs-read is blocked by the build envelope's
  `fs.read` set; even absent that, the POST target is not on the allowlist →
  exfiltration blocked.

## Fail-closed policy

Per the project principle "any check that *could* run at build time *must*;
escape hatches are explicit `--allow-X`, never implicit":

```
 gate wraps with a real sandbox tier        → proceed (default, silent)
 gate is None  OR resolves Passthrough/None → REFUSE with a typed error that
                                              names --allow-unsandboxed-build
 --allow-unsandboxed-build (explicit)       → proceed unsandboxed + loud warning
```

`tau install` therefore **fails closed** when it cannot sandbox the build (no
landlock, no `sandbox-exec`, `--no-sandbox` already in effect, etc.). The only
way through is the explicit `--allow-unsandboxed-build` flag, which also emits
a `tracing::warn!`. There is no implicit fallback.

A `Passthrough`/tier-`None` adapter counts as "cannot sandbox": injecting a gate
that does nothing must not silently satisfy the requirement. The
`InstallSandboxGuard` (or the wrap result) carries enough signal for `tau-pkg`
to distinguish "really sandboxed" from "no-op", so the fail-closed decision is
made in `tau-pkg`, not delegated to the adapter.

## Public API changes

### `tau-pkg`

New module (e.g. `crates/tau-pkg/src/install_sandbox.rs`):

- `pub trait InstallSandbox: Send + Sync { fn wrap(&self, plan, cmd) -> Result<InstallSandboxGuard, InstallSandboxError>; }`
- `pub struct InstallSandboxGuard` — RAII cleanup handle; also reports whether a
  real sandbox tier was applied (for the fail-closed check).
- `pub enum InstallSandboxError` (`thiserror`, `#[non_exhaustive]`).
- `pub fn build_envelope(package_dir: &Path, manifest: &PackageManifest) -> CapabilityPlan`
- `pub fn cross_check_envelope() -> CapabilityPlan`
- Cargo.toml git-host parsing helper (top-level `[dependencies]` +
  `[build-dependencies]` only).

New `InstallOptions` fields (`#[non_exhaustive]`, so additive):

- `pub sandbox: Option<Arc<dyn InstallSandbox>>`
- `pub allow_unsandboxed_build: bool`

`InstallOptions` currently `#[derive(Debug, Clone)]`; the `Arc<dyn …>` field
requires a manual `Debug` impl (or a `Debug` supertrait on `InstallSandbox`).

New `InstallError` variant (`#[non_exhaustive]`, additive):

- `InstallError::UnsandboxedBuildRefused` — emitted when the build cannot be
  sandboxed and `allow_unsandboxed_build` is false; message names the flag.

`build_rust_cargo_plugin` and `cross_check_plugin_capabilities` gain a
`gate: Option<&dyn InstallSandbox>` parameter (threaded from `InstallOptions`)
and wrap their `Command` before spawn.

### `tau-cli`

- `struct RuntimeInstallSandbox(SandboxAdapter)` implementing
  `tau_pkg::InstallSandbox`, bridging sync→async to
  `ProcessCapabilityGate::wrap_spawn` and owning the egress-proxy task lifetime
  inside the returned guard.
- At the `tau install` command call site: resolve the adapter (reusing the
  existing `resolve_adapter` path), build a `RuntimeInstallSandbox`, set
  `InstallOptions.sandbox`, and surface `--allow-unsandboxed-build` as a CLI
  flag mapped to `InstallOptions.allow_unsandboxed_build`.

## Testing

TDD throughout. `tau-pkg` unit/integration tests (`-p tau-pkg`,
`CARGO_TARGET_DIR=target/agent-s2`):

- `build_envelope` produces the expected `fs.write` / `fs.read` / `net` sets
  for a sample package dir.
- Cargo.toml git-host parsing: registry-only manifest → registry hosts;
  manifest with a `git =` dep → host added; `build-dependencies` git dep →
  host added; malformed / workspace-inherited → not added (fail-closed).
- `cross_check_envelope` is empty (no net, no fs, no child exec).
- Fail-closed: a `MockInstallSandbox` reporting "no real tier" + a build path →
  `InstallError::UnsandboxedBuildRefused`; same with `allow_unsandboxed_build =
  true` → proceeds (build skipped/stubbed in test) + warning.
- A `MockInstallSandbox` is invoked with the build envelope before the cargo
  spawn and with the cross-check envelope before the binary spawn (assert
  `wrap` call ordering and the plan passed each time).
- `InstallError::UnsandboxedBuildRefused` display names the flag.
- `#[non_exhaustive]` discipline test for the new error enum.

The end-to-end "build actually runs under landlock/seccomp" behavior is
verified by the `tau-cli` adapter wiring and exercised on Linux CI; pure
`tau-pkg` tests use the mock gate (no real sandbox required, matching the
existing `skip_cross_check` test pattern).

## Out of scope (deferred, noted not built)

- Git-host parsing for **workspace-member**, **target-specific
  (`[target.'cfg(..)'.dependencies]`)**, and **dev-dependency** tables. v1
  parses top-level `[dependencies]` + `[build-dependencies]` only; anything
  else fails closed and the user falls back to `--allow-unsandboxed-build`.
- Offline/vendored-dependency build mode (`--offline` + `Cargo.lock`
  presence) as a zero-network alternative.
- Sandboxing `Git::clone` itself (the clone writes only into the staging
  tempdir and runs the git binary, not package code; lower priority than the
  build + cross-check execution paths). Tracked as a possible follow-up.
```
