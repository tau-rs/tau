# Windows AppContainer adapter — Phase 2 (Strict-tier enforcement)

**Date:** 2026-08-09
**Status:** design approved, pre-implementation.
**Supersedes (Phase 2 of):** ADR-0023 (Windows AppContainer scaffold), spec
`2026-05-09-sandbox-windows-design.md`.
**New ADR:** ADR-0066 (to be written in PR3).
**Base:** `main` at or after `04a546db`.

## Goal

Graduate `crates/tau-sandbox-windows` from a Phase-1 stub (probe returns
`Unavailable`, ACL/spawn are no-ops) into a **truthful Strict-tier adapter** that
enforces **filesystem + process isolation** via Windows AppContainer. Once the
adapter probes `Available { tier: Strict }` and the runtime registry routes to it
on Windows, `resolve_adapter(SandboxRequirements::default())` succeeds on Windows,
and the 10 install-path Tier-2 tests currently gated with
`#[cfg_attr(windows, ignore = "…Phase-2 stub")]` un-gate.

**Network egress is explicitly out of scope** and is handled fail-closed (see
"Network egress: deferred" below). It becomes its own follow-on EPIC.

## Why this is not a green-field design

The enforcement model was already decided in ADR-0023 and the 2026-05-09 spec:
**AppContainer + `CreateProcessAsUserW` + (eventually) `tau-sandbox-proxy`**.
ADR-0023 shipped the scaffold and named three coupled Phase-2 blockers:

1. `tau-sandbox-proxy::spawn_proxy` is `cfg(unix)` (UnixListener).
2. `std::process::Command` on Windows has no `pre_exec`; attaching
   `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` requires `CreateProcessAsUserW`.
3. Real Win32 calls in `acl.rs`.

This spec resolves (2) and (3), and **defers (1)** entirely. It also makes two
peripheral flips that the graduation forces (registry platform set, target-triple
status).

## The 10 gated tests, and what actually blocks them

The gated tests (`cmd_install.rs` ×2, `cmd_list.rs` ×2, `cmd_uninstall.rs` ×2,
`cmd_update.rs` ×4) each perform a real `tau install` of a `file://` fixture as
setup. `install.rs` resolves a Strict adapter
(`resolve_adapter(SandboxRequirements::default())`, install.rs:65). On Windows
every registry candidate is rejected (Native = `LinuxAndDarwin` platform mismatch;
Container = no docker; Remote = none; Passthrough = tier `None` < `Strict`) →
`NoAdapterMatches` → install fails closed → the setup `.success()` assertion fails.

**Load-bearing fact:** the fixture is `kind = "tool"` with `capabilities = []` and
only a `tau.toml` (no `Cargo.toml`, no `[plugin]` table). So install's
`build_plugin_if_needed` returns `None`, the cross-check is skipped, and
**`gate.wrap()` is never called**. The adapter is only *resolved*, never *used*.

Consequence: **making the 10 tests green requires only `probe → Available{Strict}`
+ the registry flip — zero real enforcement.** That is a trap: flipping the probe
while `acl.rs`/spawn stay no-ops would make a real `kind = "rust-cargo"` install on
Windows resolve a "Strict" adapter, pass the fail-closed S2 check, and run
untrusted `build.rs` **unsandboxed** — reintroducing the exact RCE the install-build
sandbox (`2026-06-12-install-build-sandbox-design.md`) closes on Linux/macOS. The
probe would be lying.

Therefore the completion bar for this EPIC is **truthful enforcement**, proven by
its own Windows integration tests, *before* the probe is flipped. The 10 tests are
a trailing side-effect, not the goal.

## Enforcement model (Camp-2 exec-wrapper)

tau is already a "set kernel-enforced policy, then exec the target and step back"
system on every platform: Linux (landlock + seccomp via `pre_exec`), macOS
(`sandbox-exec` wrapper binary), egress (`tau-net-bridge` wrapper binary). tau runs
**no IPC broker and does no syscall brokering anywhere**. The Windows adapter
follows the same philosophy. AppContainer is a kernel-enforced capability/ACL model
and **does not require a broker** (the broker in Chromium/Firefox is for the *extra*
restricted-token/interception tightening, not for AppContainer itself).

### Filesystem + process isolation

- **Per-spawn AppContainer profile.** `wrap_spawn` creates a uniquely-named profile
  (`tau-sbx-<pid>-<counter>`), derives its SID, and grants FS ACLs to that SID on
  the plan's read/write paths. The returned `CapabilityHandle::Drop` revokes the
  ACLs (reverse order) and deletes the profile — leak-safe, identical to the
  darwin/native lifetime pattern. Per-spawn unique names scope any leak.
- **Process launch via a helper binary.** A new stateless
  `tau-appcontainer-launcher.exe` performs the `CreateProcessAsUserW` dance.
  `wrap_spawn` rebuilds the `&mut std::process::Command` in place using the darwin
  idiom (`*cmd = Command::new(launcher_path); cmd.arg(...)...`), prepending the
  launcher so the *real* program runs *through* it. `plugin_host::process`, the mcp
  `transport_stdio` spawn site, and the install path are **unchanged** — they keep
  doing `command.spawn()` and receive a normal `tokio::process::Child` (the
  launcher).

### Why a launcher (not `CreateProcessAsUserW` in the adapter)

`Command::spawn()` gives back a `tokio::process::Child` with wired-up async
stdin/stdout/stderr + `id()`/`wait()`/`kill()`. Raw `CreateProcessAsUserW` gives
back a **bare OS `HANDLE`** — and there is no public way in std/tokio to turn a raw
HANDLE into a `Child`. Calling it inside the adapter would force reimplementing
`Child` (pipes via `CreatePipe`, wait via `WaitForSingleObject`, kill via
`TerminateProcess`) *and* refactoring the shared spawn path across all three call
sites. The launcher quarantines all raw-Win32 process creation inside a ~150-line
standalone exe; `plugin_host` keeps spawning through the same
`wrap_spawn(&mut Command) → spawn()` contract it uses on every platform. This is the
identical shape to the existing `tau-net-bridge` helper.

### Launcher CLI contract

```
tau-appcontainer-launcher
  --profile <appcontainer-profile-name>   # adapter already created it; launcher derives the SID
  --cap <well-known-capability-sid>        # 0..N, repeatable (Phase 2 typically empty; net deferred)
  --                                        # end of launcher args
  <real-program> <real-arg>...              # the actual plugin / cargo invocation
# stdio: inherits the launcher's own stdin/stdout/stderr (piped by tokio in the parent)
# exit:  propagates the AppContainer child's exit code
```

Launcher responsibilities (stateless — creates/cleans nothing persistent):

1. `sid = DeriveAppContainerSidFromAppContainerName(--profile)`.
2. Build `SECURITY_CAPABILITIES { AppContainerSid: sid, Capabilities: [--cap SIDs] }`.
3. `InitializeProcThreadAttributeList` + `UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES)`.
4. `STARTUPINFOEXW` with `hStdInput/Output/Error = GetStdHandle(...)` (the inherited pipes).
5. `CreateJobObject` + `SetInformationJobObject(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)`.
6. `CreateProcessAsUserW(EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED, bInheritHandles = TRUE, …)`.
7. `AssignProcessToJobObject` + `ResumeThread`.
8. `WaitForSingleObject(child)` → `GetExitCodeProcess` → `ExitProcess(code)`.

Ownership split: the **adapter** owns everything that must be cleaned up (profile +
ACLs, via `CapabilityHandle::Drop`); the **launcher** is stateless. A launcher
crash cannot leak ACL grants. Kill propagation: `plugin_host` kills the launcher →
its Job Object closes → `KILL_ON_JOB_CLOSE` kills the AppContainer child. No orphans.

### stdio flow (why `plugin_host` is unchanged)

```
tau.exe (parent)            tau-appcontainer-launcher.exe      AppContainer child
tokio Child.stdout  ◄─────  inherited STDOUT            ◄─────  child hStdOutput
tokio Child.stdin   ─────►  inherited STDIN             ─────►  child hStdInput
tokio Child.stderr  ◄─────  inherited STDERR            ◄─────  child hStdError
Child.kill()  ───────────►  Job KILL_ON_JOB_CLOSE       ───────►  child dies
Child.wait()  ◄───────────  ExitProcess(child_code)     ◄───────  child exits
```

## Network egress: deferred (fail-closed)

AppContainer processes are **blocked from loopback (`127.0.0.1`) by default**. The
only exemption, `CheckNetIsolation LoopbackExempt`, is Microsoft-documented as *for
development purposes only*, is machine-global mutable state keyed by AppContainer
SID, requires admin, is racy across concurrent installs, and grants the container
*all* loopback (not just our proxy). Our proxy listens on `127.0.0.1:8443`, so the
clean "proxy on loopback" model is not viable in production, and the named-pipe
alternative is circular (reqwest speaks TCP CONNECT via `HTTPS_PROXY`, not named
pipes; bridging it back to loopback hits the same wall).

Decision: the Phase-2 adapter enforces **FS + process isolation only** and **fails
closed on network**:

- `supported_shapes()` **drops `NetworkHttp`** (keeps `FilesystemRead`,
  `FilesystemWrite`, `ProcessExec`).
- `wrap_spawn` **refuses any plan carrying an HTTP capability** with a typed
  `CapabilityError` (the current stub already does this).

This is honest: it enforces a real Strict FS/exec envelope and fails closed on
network rather than lying. It is consistent with tau's fail-closed principle.

**Consequence — real `rust-cargo` builds do not run under the Windows sandbox in
this EPIC.** `build_envelope` (`2026-06-12-install-build-sandbox-design.md`) *always*
includes registry network hosts, so every real `rust-cargo` install carries a net
shape → fails closed on Windows → the user falls back to `--allow-unsandboxed-build`
until the egress follow-on lands. The 10 gated tests are unaffected (their fixture
has no build and no net; `wrap` is never called). The adapter's FS enforcement is
real and is proven via FS-only integration plans (below), not via a live cargo build.

**Follow-on (separate spec, referenced from ADR-0066):** *Windows sandbox network
egress* — solve the loopback-exemption-vs-named-pipe problem properly, add a
TCP/named-pipe transport to `tau-sandbox-proxy`, restore `NetworkHttp` to
`supported_shapes`, and un-defer net.

## Worked example (illustrative — requires the deferred net follow-on for the proxy leg)

`tau install --global <rust-cargo pkg>` whose malicious `build.rs` reads
`~/.ssh/id_rsa` and POSTs to `evil.com`, while legitimately needing the toolchain +
`target/` + crates.io:

- FS: read `~/.rustup`, `~/.cargo`, source tree → **granted**; write `target`, `%TEMP%`
  → **granted**; read `~/.ssh/id_rsa` → **ACCESS DENIED** (SID never on that ACL).
- Net (once the follow-on lands): direct `evil.com` → blocked (no `internetClient`);
  proxied → `evil.com ∉ allowlist` → 403, `index.crates.io ∈ allowlist` → forwarded.

In *this* EPIC the net leg fails closed (build refused), so this end-to-end flow is
exercised only after the egress follow-on. The FS-denial half is enforced and tested
now.

## Component map

| File | Change | PR |
|---|---|---|
| `crates/tau-sandbox-windows/src/bin/tau-appcontainer-launcher.rs` | **NEW** — Win32 `CreateProcessAsUserW` + job object + stdio inherit; `cfg(windows)` | 1 |
| `crates/tau-sandbox-windows/Cargo.toml` | `windows` crate under `[target.'cfg(target_os = "windows")'.dependencies]`; declare the `[[bin]]` | 1 |
| `crates/tau-sandbox-windows/src/acl.rs` | real Win32: `create_appcontainer_profile`, `delete_appcontainer_profile`, `derive_sid`, `grant_access`, `revoke_access` (were stubs) | 2 |
| `crates/tau-sandbox-windows/src/lib.rs` | `wrap_spawn`: FS ACLs + launcher rebuild + refuse-HTTP; `supported_shapes` drops `NetworkHttp`; **probe stays `Unavailable`** | 2 |
| `crates/tau-sandbox-windows/src/lib.rs` | `probe → Available { tier: Strict }` | 3 |
| `crates/tau-sandbox-windows/src/profile.rs` | pure `build_appcontainer_caps` (exists); extend only if a field is missing | 2 |
| `crates/tau-sandbox-windows/tests/strict_integration.rs` | **NEW** — `#![cfg(all(target_os = "windows", feature = "integration-tests"))]` enforcement proof | 2 |
| `crates/tau-runtime-tokio/src/process_gate/registry.rs` | `Native` platform set `LinuxAndDarwin → +windows`; update `native_is_linux_and_darwin` test | 3 |
| `crates/tau-ports/src/target/registry.rs` | `windows-native-strict` `Reserved → Available` | 3 |
| `.github/workflows/tier2.yml` | `nextest / windows` job gains `--features integration-tests` | 2 |
| `crates/tau-cli/tests/{cmd_install,cmd_list,cmd_uninstall,cmd_update}.rs` | remove the 10 `#[cfg_attr(windows, ignore = …)]` gates | 3 |
| `docs/decisions/0066-*.md`; `docs/decisions/0023-sandbox-windows-scaffold.md` | new ADR-0066; mark 0023 Phase-2-superseded | 3 |

## Phasing (3 PRs, each independently green)

- **PR1 — launcher binary.** Ship `tau-appcontainer-launcher.exe` standalone. A
  Windows integration test invokes it directly (spawn a probe under an AppContainer,
  assert isolation). No runtime wiring yet. CI-only iteration, but self-contained.
- **PR2 — real enforcement, probe still `Unavailable`.** Real `acl.rs`; `wrap_spawn`
  does FS ACLs + prepend launcher + refuse-HTTP; `supported_shapes` drops
  `NetworkHttp`; add `strict_integration.rs`; add `--features integration-tests` to
  the `nextest / windows` Tier-2 job. The adapter is fully functional but the probe
  declines, so the resolver still falls back to Passthrough — production behavior on
  Windows is unchanged. **A green Windows integration run here is the gate for PR3.**
- **PR3 — flip the switch.** `probe → Available { tier: Strict }`; registry
  `+windows`; target-triple `→ Available`; un-gate the 10 tests; ADR-0066 + mark
  0023 superseded. This is the only PR needing the `full-matrix` label. By now
  enforcement is already proven.

## Testing

- **Pure unit tests** (`profile.rs`, any host): `build_appcontainer_caps` translates
  a `CapabilityPlan` into the expected `AppContainerCaps` (read/write path sets,
  `has_http`, `has_process_spawn`). Already present; extend as needed.
- **Enforcement proof** (`tests/strict_integration.rs`, Windows CI,
  `integration-tests`): drive the adapter directly (`resolve_adapter_forced` or
  direct `WindowsSandbox`, bypassing the still-`Unavailable` probe) to assert:
  1. empty-plan AppContainer (the `cross_check_envelope` shape) — a spawned probe
     process **cannot** read a file outside any grant;
  2. read-granted plan — the probe **can** read the granted temp path but **not** a
     sibling;
  3. an HTTP-capability plan is **refused** (fail-closed).
- **The 10 un-gated tests** (Tier-2 `nextest / windows`, nightly + `full-matrix`):
  green once PR3 lands.
- **No regressions** on Linux/macOS adapters (normal Tier-0 gate) or the shared
  spawn path (untouched).

### CI wiring

Add `--features integration-tests` to the existing `nextest / windows` Tier-2 job
(`tier2.yml`) so the Windows enforcement proof and the un-gated tests run in the same
nightly/`full-matrix` lane. `windows-latest` supports AppContainer creation + ACLs +
`CreateProcessAsUserW`; no loopback exemption is needed because network is deferred.

## Verification per PR

- **PR1:** `cargo check --target x86_64-pc-windows-gnu -p tau-sandbox-windows` locally
  (macOS) for cfg-gating; launcher integration test green on `windows-latest`.
- **PR2:** `strict_integration.rs` green on `windows-latest`; Linux/macOS Tier-0 green;
  `cargo clippy`/`fmt` clean. Probe still `Unavailable` (assert no production behavior
  change).
- **PR3:** Tier-2 `nextest / windows` green with the 10 tests un-gated (per-job green
  check; raw logs via `gh api repos/tau-rs/tau/actions/jobs/<id>/logs`); target-triple
  and registry unit tests updated and green.

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| CI-only iteration (~5–7 min/cycle; no local AppContainer) | Maximize pure (`profile.rs`) and standalone (launcher) units; land enforcement proof (PR2) before the probe flip (PR3). |
| Raw-Win32 process creation complexity | Quarantined in a ~150-line launcher; mirrors the proven `tau-net-bridge` shape. |
| ACL / profile leak on crash | Adapter `CapabilityHandle::Drop` revokes + deletes; per-spawn unique profile name scopes leaks. |
| Orphaned sandboxed child | Job Object `KILL_ON_JOB_CLOSE`; killing the launcher kills the child. |
| Probe-flip becomes a security lie | Ordering: enforcement + its integration proof land (PR2) before `probe → Available` (PR3). |
| FS-grant breadth for real cargo builds unverified | Out of scope — all real `rust-cargo` builds carry a net shape → fail closed until the egress follow-on; FS path proven via FS-only integration plans. |

## Out of scope (deferred, noted not built)

- **Network egress** — the whole proxy/loopback story; its own follow-on spec + EPIC.
- **`tau-sandbox-proxy` TCP/named-pipe transport** — part of the egress follow-on.
- **Restricted-token / integrity-level tightening beyond AppContainer** (Chromium-style
  broker + interception) — tau relies on kernel-enforced policy, not brokered IPC.
- **Windows local dev environment** (UTM + Windows 11 ARM VM) — separate sub-project.
- **Per-syscall filtering** — no seccomp equivalent on Windows (AppContainer is the
  Strict envelope from the plugin's perspective).
