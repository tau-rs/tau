# ADR-0019: Per-host network filter — machinery shipped, strict.rs integration deferred (sub-project F)

**Status:** Accepted
**Date:** 2026-05-06
**Deciders:** Titouan Lebocq
**Supersedes:** —
**Closes:**
- Sub-project F from the priority-12 followups — per-host network filtering machinery (`probe`, `validate`, `resolve`, `netns`, `rules`, `handle`, `orchestrator`). Full strict.rs integration deferred to task 6.5.
**Refines:**
- [ADR-0014](0014-sandboxing.md) — the v0.1 `unshare_flags_for_plan` over-permissive fallback is still in place; task 6.5 will wire `apply_per_host_filter` into `wrap_spawn`.

## Context

### The v0.1 gap

Priority-12 sandboxing (ADR-0014) stripped `CLONE_NEWNET` when `Network(Http)` was in the plan, leaving the child in the parent's network namespace. This meant any plugin with an HTTP capability could reach any host — the plan's `hosts` allow-list was declared but not enforced at the kernel level. `tau-sandbox-native::net::unshare_flags_for_plan` recorded a `tracing::warn!` once-per-process to document this over-permissive behavior.

The priority-12 followups doc (written 2026-05-03) identified sub-project F — per-host egress filtering via nftables-in-netns — as the fix.

### Sub-project F's design

The spec (`docs/superpowers/specs/2026-05-06-sandbox-net-filter-design.md`) and plan (`docs/superpowers/plans/2026-05-06-sandbox-net-filter.md`) describe a layered approach:

1. Keep `CLONE_NEWNET` always; child runs in fresh empty netns.
2. Create a veth pair in the parent and move one end to the child netns, assign IPs.
3. Resolve hostnames in `Capability::Network(NetCapability::Http { hosts })` to IPs (DNS lookup with timeout, multi-record A + AAAA).
4. Generate an nftables ruleset in the child netns: allow egress to resolved IPs + DNS; drop everything else.
5. Require `CAP_NET_ADMIN` inside the user namespace.
6. Hard-refuse when prerequisites (nft/ip/nsenter binaries, CAP_NET_ADMIN-in-userns) are absent.

### What shipped (PR #35, commit d4438ae)

The full machinery module `tau-sandbox-native::net_filter` was implemented and unit-tested:

- `probe` — checks nft/ip/nsenter binaries and CAP_NET_ADMIN-in-userns availability.
- `validate` — rejects wildcard hosts (`*`, `*.x.y`) at plan validation time.
- `resolve` — multi-record (A + AAAA) DNS lookup per host via `tokio::net::lookup_host`.
- `netns` — veth/netns setup helpers (create netns, veth pair, move peer, assign IPs).
- `rules` — nftables ruleset generation (deterministic nft DSL, verified by insta snapshots).
- `handle` — `NetFilterHandle` RAII guard that tears down the veth/netns on Drop.
- `orchestrator` — `apply_per_host_filter(plan, child_pid)` composing probe → resolve → netns → rules → apply.

26+ unit tests across these modules. A dedicated CI job (`test-net-filter / linux`) exercises the real probe + resolve + helpers inside privileged Docker.

### What did NOT ship — the task 6.5 deferral

Task 6 originally specified wiring `apply_per_host_filter` into `strict.rs::apply_strict`. During implementation it became clear this requires a structural change to the `Sandbox` trait:

- `Sandbox::wrap_spawn` returns a `pre_exec` closure that runs **inside the forked child**, before `execve`. The child PID is not yet known at this point.
- `apply_per_host_filter` needs the **child PID** to move the veth peer into the child's netns (via `ip link set <peer> netns <pid>`).

The clean solution is a new trait method `Sandbox::apply_post_spawn(plan, child_pid) -> NetFilterHandle` that runs **concurrently with the child's pre_exec phase** while the child blocks on a sync pipe. The child unblocks the pipe only after the parent has completed `apply_post_spawn`. This "parent-side post-spawn hook with sync pipe rendezvous" pattern is more than a single task — it requires extending the `Sandbox` trait in `tau-ports` and updating all adapters + the runtime's `plugin_host`. Task 6.5 will land this integration.

Until task 6.5 ships, `unshare_flags_for_plan` retains its v0.1 fallback behavior.

## Five design decisions

### Decision 1 — Q1.A: Hard refuse on prerequisite miss

**Decision:** When `nft`, `ip`, or `nsenter` binaries are absent, or when `CAP_NET_ADMIN` cannot be granted inside an unprivileged user namespace, the sandbox returns `SandboxError::NetFilter { message: String }` rather than silently falling back to the v0.1 over-permissive behavior.

**Rationale:** A security primitive that silently degrades to "allow everything" is not a security primitive. Operators who deploy tau on systems without nftables (e.g. Alpine with only iptables) must explicitly configure an alternative adapter. A hard refuse surfaces the configuration gap immediately rather than silently violating the `hosts` allow-list.

**Consequences:**
- Deployments on nftables-free Linux distros will see a clear error and must configure the container adapter or an alternative.
- GHA host runners block uid_map writes, making CAP_NET_ADMIN-in-userns unavailable on standard CI — this forced the privileged Docker CI strategy (see Errata).

**Alternatives considered:**
- Probe-and-skip (fallback to v0.1): less rigorous; the `hosts` allow-list would be silently unenforced. Rejected.

### Decision 2 — Q2.A: `nft` + `ip` + `nsenter` CLI shell-out (not Rust bindings)

**Decision:** All netns and nftables operations use shell-out to the `nft`, `ip`, and `nsenter` system binaries, mirroring the container adapter's pattern of shell-outing to `docker`/`podman`.

**Rationale:** The `rustables` and `nftnl-rs` crates are less mature than the system binaries and would require C library FFI bindings or unsafe netlink code. The system binaries are present on any modern Linux system that meets tau's other requirements. Shell-out is auditable (the exact commands are visible in logs), keeps the Rust code surface small, and follows the precedent set by `tau-sandbox-container`.

**Consequences:**
- Three binary probes are required at startup: `nft --version`, `ip version`, `nsenter --version`.
- Shell-out errors produce `SandboxError::NetFilter { message }` via `error.to_string()` conversion.
- The nft DSL generated is deterministic and verified by insta snapshots.

**Alternatives considered:**
- `rustables` / `nftnl-rs`: larger surface, less mature, more code. Rejected per spec Q2.A.

### Decision 3 — Q3.A: One-shot DNS resolution at wrap_spawn time

**Decision:** Hostname resolution happens once at `apply_per_host_filter` invocation time (i.e., at wrap_spawn / post-spawn time). Multi-record (A + AAAA) per host. TTL refresh is deferred.

**Rationale:** One-shot resolution is sufficient for the majority of plugin workloads (a plugin that needs to call `api.anthropic.com` will resolve the same IPs throughout its lifetime). TTL refresh would require a background task that updates nftables rules while the plugin is running — significantly more complex and deferred to a future sub-project.

**Consequences:**
- A plugin running for many hours might encounter DNS changes mid-run. Accepted as a known limitation.
- `tokio::net::lookup_host` was added to `tau-sandbox-native`'s production dependencies (the `net` feature of the `tokio` workspace dep).

**Alternatives considered:**
- TTL-respecting refresh: deferred as described.

### Decision 4 — Q4a.A1: Reject wildcards at plan validation

**Decision:** Wildcard hosts (`*` and `*.x.y` forms) are rejected by `validate` with a clear error at plan-validation time, before any network setup begins.

**Rationale:** Wildcards would require per-DNS-response dynamic rule updates to be meaningful. A static nftables ruleset cannot meaningfully enforce "any IP that resolves from `*.example.com`". Rejecting wildcards eagerly makes the constraint explicit.

**Consequences:**
- Plans that declare `hosts: ["*"]` (the "any host" escape hatch) will fail validation when the per-host filter is active.
- Plugin authors who need unrestricted network access should declare no Network capability and use a non-strict tier.
- Superseded in part by ADR-0059: hosts are now a `HostSet`; `"*"` is a decode error and `"any"` is the sentinel.

**Alternatives considered:**
- Allow wildcards and expand at resolution time: impractical with static nftables rules. Rejected.

### Decision 5 — Q4b.B2: Tests bind on parent-veth IP via env var

**Decision:** Integration tests for the netns + veth + nftables setup bind a mock server on the parent-side veth IP (injected via `TAU_NET_PARENT_VETH_IP` environment variable) rather than via DNAT auto-routing.

**Rationale:** DNAT auto-routing requires configuring a NAT rule in the parent netns, which adds complexity and requires additional capabilities. Injecting the parent-veth IP via env var is simpler and sufficient for test coverage. The `TAU_NET_PARENT_VETH_IP` pattern mirrors the `TAU_TESTING_ALLOW_MOCK_SANDBOX` pattern already established in the codebase.

**Consequences:**
- Tests that require live HTTP responses bind on the parent-veth IP directly.
- The env var is documented in `crates/tau-sandbox-native/src/net_filter/INTEGRATION.md`.

**Alternatives considered:**
- DNAT-based routing: more complex, higher-privilege requirement. Rejected per spec Q4b.B2.

## Errata vs. spec and plan

### 1. `SandboxError` lacks `#[non_exhaustive]` and derives `Clone+PartialEq+Eq`

The spec proposed `SandboxError::NetFilterError { source: io::Error }`. During implementation it became clear that `io::Error` does not implement `Clone`, `PartialEq`, or `Eq`, and `SandboxError` must derive all three (it is used in test assertions throughout `tau-ports`). Embedding `io::Error` directly is impossible.

**Plan correction:** the single new variant is `SandboxError::NetFilter { message: String }`. The rich `NetFilterError` enum stays internal to `net_filter::*`; the conversion at the orchestrator boundary is `NetFilterError` → `error.to_string()` → `SandboxError::NetFilter { message }`.

### 2. Phase 0 finding: GHA host runners block uid_map writes

The spec assumed the `test-net-filter / linux` job could run on a standard `ubuntu-latest` GHA runner. During Phase 0 verification (PR #34, closed), uid_map writes returned "Operation not permitted" on the host runner — the kernel permits user namespaces but the GHA sandbox blocks writing to `/proc/<pid>/uid_map` after namespace creation.

A privileged Docker container (`docker run --privileged rust:1.95-bookworm`) DOES allow uid_map writes. This is the CI strategy selected during Phase 0 by the project owner.

**Impact:** the `test-net-filter / linux` job in `.github/workflows/ci.yml` runs inside privileged Docker. This is documented in the job definition and in `crates/tau-sandbox-native/src/net_filter/INTEGRATION.md`.

### 3. F task 6.5 deferral — architectural reason

Task 6 originally assumed `apply_per_host_filter` could be wired into `strict.rs::apply_strict` using the existing `Sandbox::wrap_spawn` machinery. The architectural conflict:

- `wrap_spawn` provides a `pre_exec` hook that runs in the forked child BEFORE execve. The child PID is not accessible from within `pre_exec` (it is the PID of the current process at that point, not what the parent sees).
- The veth peer must be moved into the child's netns using `ip link set <peer> netns <pid>` — a parent-side operation that requires the child's PID as seen from the parent.

These two requirements are fundamentally incompatible with the current `Sandbox::wrap_spawn` API surface. The correct fix is:

```
trait Sandbox {
    // Existing
    fn wrap_spawn(&self, plan: &SandboxPlan, cmd: &mut Command) -> Result<Box<dyn SandboxHandle>>;

    // New in task 6.5
    fn apply_post_spawn(
        &self,
        plan: &SandboxPlan,
        child_pid: u32,
    ) -> Result<Box<dyn SandboxHandle>>;
}
```

The `plugin_host` orchestrator would:
1. Call `wrap_spawn` to install the pre_exec hooks (landlock + seccomp namespaces).
2. Spawn the child; the child blocks on a sync pipe.
3. Call `apply_post_spawn(plan, child_pid)` to run the per-host filter setup.
4. Signal the sync pipe; the child unblocks and proceeds to execve.

This requires extending the `Sandbox` trait in `tau-ports`, updating all adapter implementations, updating the `plugin_host` spawn orchestration, and updating the `MockSandbox` fixture. Task 6.5 will land this integration.

## Consequences

### Positive

- All sub-project F machinery is in place and unit-tested (~26 tests across probe, validate, resolve, netns, rules, handle).
- Insta snapshots verify the nft DSL is deterministic — a regression in rule generation will surface immediately.
- The CI job `test-net-filter / linux` exercises the real probe + resolve + helpers in privileged Docker.
- The `SandboxError::NetFilter { message: String }` variant in `tau-ports` provides a clean error boundary between the internal `NetFilterError` hierarchy and the public port API.
- The task 6.5 integration scope is clearly bounded: extend `Sandbox` trait, wire `apply_post_spawn` in `plugin_host`, add sync pipe, update adapters and mock.

### Negative

- Per-host filtering is NOT yet enforced at runtime — `strict.rs` still uses the v0.1 fallback (drops `CLONE_NEWNET` when `Network(Http)` is in plan; child inherits parent netns).
- The 3 `#[ignore]`'d Layer 4 container × HTTP plugin tests in `tau-plugin-compat/tests/layer4_container.rs` remain ignored (they need real F integration at the strict.rs level).
- 4 integration tests in `tests/strict_net_filter.rs` are `#[ignore]`'d stubs pending task 6.5.
- CI infrastructure requires privileged Docker for the `test-net-filter / linux` job — a slightly elevated CI privilege level.

### Neutral

- The followups doc gap row "per-host network filtering is over-permissive" stays open until task 6.5 ships.
- `tokio` `net` feature was added to `tau-sandbox-native` production deps; this is a small dependency surface increase.
- Required CI checks rise 14 → 15 (new `test-net-filter / linux` job).

## Alternatives considered

### `rustables` / `nftnl-rs` Rust bindings instead of `nft` CLI shell-out

Rejected. The Rust netfilter binding crates are less mature than the system binaries, require C library FFI or unsafe netlink code, and don't meaningfully reduce the privilege requirements. Shell-out mirrors the container adapter's proven pattern. Rejected per spec Q2.A.

### Embed `NetFilterError` directly in `SandboxError`

Impossible due to `Clone+Eq` derive requirements on `SandboxError`. `io::Error` does not implement `Clone` or `Eq`. The `message: String` flattening at the boundary is the correct approach. Rejected per plan errata above.

### Probe-and-skip contingency for CI (fallback to v0.1 on GHA)

Considered during Phase 0 when the uid_map write failure was discovered. Rejected because it would make the test suite contingent on a capability that varies across environments — privileged Docker provides a clean, reproducible test surface. Path B (privileged Docker) was selected by the project owner.

### Larger scope for task 6 (full strict.rs integration)

The architectural conflict (child PID unavailable during `wrap_spawn` / `pre_exec`) was identified early. Absorbing the `Sandbox` trait surgery into task 6 would have significantly expanded scope. Deferring to a focused task 6.5 keeps the PR reviewable and the machinery independently testable.

## References

- Spec: [`docs/superpowers/specs/2026-05-06-sandbox-net-filter-design.md`](../superpowers/specs/2026-05-06-sandbox-net-filter-design.md)
- Plan: [`docs/superpowers/plans/2026-05-06-sandbox-net-filter.md`](../superpowers/plans/2026-05-06-sandbox-net-filter.md)
- INTEGRATION.md: `crates/tau-sandbox-native/src/net_filter/INTEGRATION.md` — deleted with the `net_filter` module per ADR-0020.
- PR #35 (merged 2026-05-06 at commit d4438ae): per-host network filter machinery
- Phase 0 verification PR #34 (closed): GHA uid_map write failure discovery
- ADR-0014: [`0014-sandboxing.md`](0014-sandboxing.md) — original sandbox design
- Followups doc: [`../superpowers/specs/2026-05-03-sandboxing-followups.md`](../superpowers/specs/2026-05-03-sandboxing-followups.md)

## Addendum (2026-05-06): F task 6.5 shipped — Native adapter integration

Sub-project F task 6.5 wired the per-host network filter machinery into the Native sandbox adapter's spawn lifecycle. Phase 1 PR #37 lands the architectural changes:

### Trait extension

A new `Sandbox::apply_post_spawn(&self, plan, child_pid, &mut handle) -> Result<(), SandboxError>` trait method runs concurrently with the child's `pre_exec` phase while the child blocks on a sync pipe. The default no-op implementation covers `Mock` / `Container` / `Light` / `Passthrough`. Only `NativeSandbox` overrides it.

`SandboxHandle` gains (cfg(unix)):
- `sync_write_fd: Option<OwnedFd>` — when present, dropping the handle closes the fd, releasing the child with EOF on its blocking read.
- `nest_handle(Box<dyn Send>)` — append nested cleanup guards. The `NetFilterHandle` is nested here so the parent veth tears down LIFO-ordered after the child's process exits.
- `signal_post_spawn_complete() -> io::Result<()>` — writes 1 byte to the sync pipe, releasing the child.

### Native adapter wiring

`apply_strict` (in `crates/tau-sandbox-native/src/strict.rs`):
- Pre-allocates a `VethSubnet` if the plan has `Network(Http)` (the impure `setup_veth_pair_with_subnet` runs later, in `apply_post_spawn`).
- Sets `TAU_NET_PARENT_VETH_IP` env var on the spawned `Command` BEFORE `cmd.spawn()` so the child can read its parent's bind IP.
- Creates a sync pipe via `nix::unistd::pipe()`; the child reads 1 byte in `pre_exec` between `unshare` and `seccomp`, blocking until the parent signals completion.
- Returns `(SandboxHandle, Option<VethSubnet>)` so `wrap_spawn` can stash the subnet.

`NativeSandbox`:
- Caches the F probe result in a `OnceLock<Result<(), NetFilterError>>` (probe runs once at first `validate_plan` call for a `Network(Http)` plan).
- `validate_plan` hard-refuses `Network(Http)` plans on F-unavailable hosts (no nft binary, no CAP_NET_ADMIN-in-userns, etc.) — surfaces a clear `SandboxError::NetFilter` instead of silently degrading.
- Per-spawn `Mutex<HashMap<RawFd, VethSubnet>>` keyed by `sync_write_fd` lets `apply_post_spawn` look up the pre-allocated subnet for each child.
- `apply_post_spawn` override: looks up the subnet, calls `apply_per_host_filter(plan, child_pid, subnet)`, nests the returned `NetFilterHandle` in the `SandboxHandle`.

`unshare_flags_for_plan` is flipped: ALWAYS returns `CLONE_NEWUSER | CLONE_NEWNET`. The v0.1 over-permissive fallback (drop `CLONE_NEWNET` when `Network(Http)` is in plan, leaving the child in the parent netns) is removed.

### Runtime caller

`tau-runtime::plugin_host::process::spawn_and_handshake` now calls `adapter.apply_post_spawn(plan, child_pid, &mut handle).await` followed by `handle.signal_post_spawn_complete()` after `cmd.spawn()`. All three post-spawn failure modes (child.id None, apply_post_spawn error, signal error) map uniformly to `RuntimeError::SandboxWrapFailed { plugin, source }` so callers can match on a single variant.

### Out of scope — carried-over follow-ups

Two items did NOT ship in Phase 1 and are tracked as gap rows in `docs/superpowers/specs/2026-05-03-sandboxing-followups.md`:

1. **Container-adapter network filtering**: the 3 `layer4_container.rs` HTTP plugin tests (anthropic / ollama / openai cassette-replay) remain `#[ignore]`'d. They use the Container adapter (Docker), which inherits the trait's no-op `apply_post_spawn`. F's veth IP is a NativeSandbox construct; inside a Docker container the host is not reachable at that IP. Container-adapter network plumbing is a separate sub-project.

2. **strict_net_filter integration tests hang in CI**: all 4 `strict_net_filter.rs` tests are `#[ignore]`'d. When `test-net-filter / linux` (privileged Docker) actually ran them, all 4 hung past 60s. Notably `no_network_cap_socket_denied_by_seccomp` also hangs (no `Network(Http)` in plan, no `apply_post_spawn` runs), suggesting the hang is in the seccomp `KillProcess` / `cmd.output()` propagation path on Linux — not in F-specific code. Needs a real-Linux debugging session to repro and fix.

The `unshare_flags_for_plan` flip + `validate_plan` hard-refuse + post-spawn integration are validated by the lib unit tests + clippy under both default and `integration-tests` features.

## Addendum (2026-05-07): Superseded by ADR-0020

The veth + nftables + CAP_NET_ADMIN design has been replaced by a userspace HTTP-CONNECT proxy (see [ADR-0020 — Sandbox proxy](0020-sandbox-proxy.md)). The `tau-sandbox-native::net_filter` module described in this ADR was deleted in PR <TBD>. Reasons: privileged-Docker friction, 7 `#[ignore]`'d tests it left blocked, and a hang in the strict_net_filter integration tests under privileged-Docker CI.
