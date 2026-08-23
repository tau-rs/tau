# ADR-0023: Windows AppContainer adapter — Phase 1 scaffold

**Status:** Accepted (scaffold); Phase 2 superseded by [ADR-0067](0067-sandbox-windows-appcontainer-phase2.md)
**Date:** 2026-05-09
**Deciders:** Titouan Lebocq
**Related:** [ADR-0014 — Sandboxing](0014-sandboxing.md), [ADR-0022 — macOS sandbox-exec adapter](0022-sandbox-darwin.md)

## Context

After ADR-0022 shipped a macOS adapter, Windows remained the last host
without a `RegistryKind::Native` resolution. The followups doc reserved
this as sub-project K ("Windows AppContainer adapter").

The Windows analogue of landlock + seccomp is **AppContainer** — a
Win32 kernel primitive (Windows 8+) that runs a process under a
restricted SID with capability-based access controls. Microsoft's
`windows` crate provides safe FFI bindings (MIT/Apache-2 licensed —
FOSS-compliant).

Two structural blockers prevent landing the full adapter in one PR:

1. **`tau-sandbox-proxy` is `cfg(unix)`-gated** — uses
   `tokio::net::UnixListener` for the parent-side accept loop. Windows
   needs a TCP-loopback or named-pipe variant before the macOS
   defense-in-depth pattern ports across.
2. **`std::process::Command` on Windows can't be intercepted at spawn
   time** — there is no `pre_exec` analogue. To attach
   `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` to the new process the
   adapter must call `CreateProcessAsUserW` directly, which means
   either refactoring `tau-runtime::plugin_host::process` to support
   per-adapter spawn customisation or replacing `Command::spawn`
   entirely on the Windows path.

The dev environment is also a constraint: AppContainer is a Windows
kernel primitive that Wine doesn't reproduce, so iteration on this
adapter is push-to-CI only (~5–7 min per `windows-latest` cycle) until
a UTM Windows 11 ARM VM lands as a separate sub-project.

## Decision

Ship Phase 1 — **the scaffold** — now, defer Phase 2 (real Win32
calls) to a follow-up PR.

Phase 1 ships:

- `crates/tau-sandbox-windows` workspace member.
- `profile.rs::build_appcontainer_caps(plan)` — pure function that
  translates a `SandboxPlan` into `AppContainerCaps { fs_read_paths,
  fs_write_paths, has_http, has_process_spawn }`. 7 unit tests; runs
  on any platform.
- `acl.rs` — Win32-shape API (`create_appcontainer_profile`,
  `delete_appcontainer_profile`, `grant_access`, `revoke_access`) but
  **stub implementations** that return `Ok(())` without calling Win32.
  No `windows` crate dep.
- `spawn.rs::register_appcontainer_for_command` — stub; documents the
  spawn-side blocker.
- `WindowsSandbox` impl `Sandbox`. **Probe returns `Unavailable` on
  Windows** explicitly — Phase 1 is honest that this adapter doesn't
  yet sandbox anything. `wrap_spawn` returns
  `SandboxError::Unavailable` for HTTP plans (proxy needs UDS→TCP
  conversion).
- Runtime registry wired: `SandboxAdapter::Windows` cfg-gated to
  Windows; `instantiate(RegistryKind::Native)` returns `Windows` on
  Windows. The resolver still falls back to `Passthrough` because the
  probe declines, so behaviour is unchanged from today.

Phase 2 (deferred) needs three coupled changes:

1. Real Win32 calls in `acl.rs` via the `windows` crate
   (`CreateAppContainerProfile`, `DeriveAppContainerSidFromAppContainerName`,
   `SetEntriesInAclW`, `SetNamedSecurityInfoW`,
   `DeleteAppContainerProfile`).
2. UDS→TCP (or named-pipe) variant of `tau-sandbox-proxy`.
3. `CreateProcessAsUserW` spawn integration (either refactor
   `plugin_host::process` for per-adapter spawn, or bypass `Command`
   on Windows).

## Consequences

Positive:

- Workspace structure, cfg-gating, and runtime registry routing land
  ahead of the platform-specific work; Phase 2 is purely additive.
- Pure-logic profile generation is testable on any host, so Phase 2
  can iterate against existing tests without `windows-latest` cycles
  for the translation logic.
- Spec at `docs/superpowers/specs/2026-05-09-sandbox-windows-design.md`
  documents all four phases and risks; Phase 2 has a clear starting
  point.

Negative:

- Phase 1 doesn't sandbox anything on Windows — the probe is honest
  about this, so users see `Unavailable` rather than thinking they're
  protected. Documented as the trade-off for shipping the scaffold
  early.
- Phase 2 requires either a Windows dev environment or push-to-CI
  iteration. UTM Windows 11 ARM VM is the future unlock.

## References

- Spec: `docs/superpowers/specs/2026-05-09-sandbox-windows-design.md`
- PR: #46 (`feat(sandbox-windows): scaffold Windows AppContainer adapter`)
- Commit: `6a58c53`
