# Windows AppContainer network egress + FILE_TRAVERSE positive FS grants

Status: approved design, spike-confirmed (2026-08-22, two CI rounds)
Tracking: issue #622 · Spike: PR #626 · Amends: ADR-0067 §"Network egress: deferred, fail-closed"
Prior art: #587/#610 (Phase 2 adapter), #617 (stdio-after-wrap invariant), ADR-0014 (fail-closed), ADR-0020 (egress proxy model)

## Problem

ADR-0067 graduated `tau-sandbox-windows` to Available/Strict with **network
fail-closed**: `supported_shapes()` omits `NetworkHttp` and `wrap_spawn`
refuses any HTTP-carrying plan. Real `kind = "rust-cargo"` installs (whose
`build_envelope` always carries registry hosts) therefore require
`--allow-unsandboxed-build` on Windows. ADR-0067 also deferred **positive
FS-path reachability**: only leaf paths get ACL grants, and the ADR asserts an
AppContainer needs `FILE_TRAVERSE` on every ancestor to reach a nested grant.

The blocker was the **AppContainer-loopback finding**: AppContainers cannot
reach host loopback (`127.0.0.1`), so the Linux/macOS "point `HTTPS_PROXY` at
a host proxy on `127.0.0.1:8443`" model does not transfer directly, and
`CheckNetIsolation LoopbackExempt` is debug-only.

## Decision: named-pipe broker + in-container loopback bridge

Mirror Linux's `tau-net-bridge` topology exactly, substituting Windows-native
primitives for the two platform-specific hops:

```text
tau.exe (parent)
 ├─ tokio task: pipe proxy (tau-sandbox-proxy core: HostAllow +
 │      CONNECT/SNI/port validation, unchanged)
 │      listens on \\.\pipe\tau-proxy-<pid>-<n>
 │      pipe DACL: owner + THIS spawn's AppContainer package SID only
 │
 └─ spawn → tau-appcontainer-launcher.exe --profile P
              -- tau-net-bridge-win --pipe <name> -- <plugin> <args>
                     │ CreateProcessW + SECURITY_CAPABILITIES (unchanged)
                     ▼
             [AppContainer, package SID S — NO network capability SIDs]
               tau-net-bridge-win            ← first process in container
                 binds 127.0.0.1:0 (ephemeral) ← same-package-SID loopback
                 conn ⇄ named pipe ⇄ host proxy
                 spawns <plugin> as child (inherits AppContainer token)
                   with HTTPS_PROXY/HTTP_PROXY=http://127.0.0.1:<port>
```

Load-bearing properties:

- **The proxy is the only egress path.** The container gets no network
  capability SIDs, so direct outbound is denied by Windows network isolation;
  the only route out is the SID-ACL'd named pipe into the host-side allowlist
  proxy. This preserves the enforcement property the Linux netns and macOS
  SBPL profile provide.
- **No syscall brokering.** Userspace proxy + wrapper binaries, same
  philosophy as `tau-sandbox-proxy`/`tau-net-bridge`/`sandbox-exec` (ADR-0067
  §"no brokering").
- **Ephemeral bridge port, not fixed 8443.** Unlike a Linux netns, Windows
  AppContainers share the host TCP port space — a fixed in-container port
  would collide across concurrent plugin spawns and with host services. The
  bridge binds `127.0.0.1:0`, learns the port, and sets `HTTPS_PROXY` itself
  for the child it spawns (it owns the child's env, so no back-channel to the
  parent is needed).
- **Fail-closed unchanged.** If profile creation, pipe creation, or bridge
  resolution fails, `wrap_spawn` returns a typed `CapabilityError` — never a
  silent drop or silent grant (ADR-0014).

### Rejected alternatives (recorded for the ADR amendment)

- **`internetClient` capability SID + proxy on a real interface** — fatal:
  the SID grants *direct* egress, so a plugin can ignore `HTTPS_PROXY` and
  dial any host; `HostAllow` becomes advisory. Also exposes the proxy on a
  LAN-reachable interface.
- **`CheckNetIsolation LoopbackExempt`** — Microsoft-documented as
  development-only: machine-global mutable state, admin-required, racy across
  concurrent installs, grants all loopback not just the proxy.

## Spike gate (PR #626)

The design's two Windows-behavior premises are measured, not assumed, by
`crates/tau-sandbox-windows/tests/spike_appcontainer_net.rs` on the tier-2
`nextest / windows` job before implementation starts:

| # | Premise | Final result (2026-08-22, runs 32562993857 + 32563879563) |
|---|---------|-----------|
| H1 | loopback works between two processes in the same AppContainer (same package SID); container→host loopback stays blocked (control) | **CONFIRMED ✅** — full TCP round-trip inside the container; control blocked (no runner exemption). Round 1's RST 10054 was a probe artifact (child `process::exit` right after `write_all` aborts the socket); a ping/pong ack fixed it |
| H2 | a named pipe whose DACL grants the package SID opens from inside; without the ACE it is denied (control) | **CONFIRMED ✅** for the plain `\\.\pipe\<name>` namespace (round-trip works; no-ACE control denied). `LOCAL\` refuted with error 2 *not found* — that prefix remaps to a per-container namespace, so the design uses the plain name |
| H3 | (item-2 premise) a leaf-only ACL grant on a nested path is NOT readable from the container | **REFUTED** — the read succeeds with no ancestor grants: AppContainer tokens retain `SeChangeNotifyPrivilege` (bypass traverse checking). ADR-0067's FILE_TRAVERSE premise is corrected in its amendment; item 2 is tests-only |

Results are recorded in the ADR-0067 amendment; the spike PR (#626) is closed
unmerged and its branch deleted.

## Components

### 1. `tau-sandbox-proxy` — genericize the connection handler (cross-platform PR)

The policy (`HostAllow`), parsers (`connect`, `http`, `validate`), and the
per-connection state machine are already platform-agnostic; only the listener
and splice plumbing are Unix-bound. Change:

```rust
// was: async fn handle_connection(plugin_sock: &mut UnixStream, ...)
pub async fn handle_connection<S>(conn: &mut S, hosts: &HostAllow) -> std::io::Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin;
```

`spawn_proxy` (UnixListener) stays `#[cfg(unix)]` and delegates to the now-pub
generic handler. `splice_bidirectional` is already generic. No behavior
change on Unix; existing tests keep passing unchanged. The crate stays
`#![forbid(unsafe_code)]`.

### 2. `tau-sandbox-windows::pipe_proxy` — Windows listener (Win32, unsafe-scoped)

Creating a named pipe with a custom DACL requires Win32
(`CreateNamedPipeW` + SDDL security descriptor), which the proxy crate's
`forbid(unsafe_code)` disallows — so the Windows listener lives next to the
other Win32 code in `tau-sandbox-windows`:

```rust
/// Named-pipe front end for tau-sandbox-proxy on Windows. Each accepted
/// pipe connection is handed to tau_sandbox_proxy::handle_connection.
pub(crate) fn spawn_pipe_proxy(
    hosts: tau_sandbox_proxy::HostAllow,
    profile: &acl::AppContainerSid,   // pipe DACL grants this package SID
) -> std::io::Result<PipeProxyHandle>;

pub(crate) struct PipeProxyHandle {
    pipe_name: String,                // tau-proxy-<pid>-<n> (plain namespace)
    task: tokio::task::JoinHandle<()>,// accept loop; Drop aborts
}
```

Pipe name uses the plain `\\.\pipe\` namespace — H2a measured it working
end-to-end from inside the container, while `LOCAL\` names remap to a
per-container namespace and are not visible (H2b, error 2). The accept loop uses
`tokio::net::windows::named_pipe::NamedPipeServer`; the first instance is
created via `ServerOptions` with raw security attributes (the one unsafe
call, scoped like `acl.rs`). DACL: owner implicit + one `(A;;GA;;;<package
SID>)` ACE — mirrors the S6 private-dir hardening (no other local user, and
no *other* AppContainer, can dial the proxy; proven by the H2 control).

### 3. `tau-net-bridge-win` — in-container bridge binary

New bin in `tau-sandbox-windows` (mirrors
`tau-sandbox-native/src/bin/tau-net-bridge.rs`):

```text
tau-net-bridge-win --pipe <name> -- <program> <args>...
  1. bind 127.0.0.1:0, learn <port>
  2. spawn <program> with HTTPS_PROXY/HTTP_PROXY (+ lowercase) =
     http://127.0.0.1:<port>; child inherits the AppContainer token
  3. accept loop: each TCP conn ⇄ fresh CreateFile(\\.\pipe\<name>) duplex
  4. exit with the child's exit code; kill listener when child exits
```

Pure std + no Win32 (client side of a named pipe is `std::fs::OpenOptions`),
so the bin compiles everywhere (non-Windows stub main like the launcher).
Arg parsing is a pure module unit-tested on any host, like `launcher_args`.

### 4. `wrap_spawn_windows` rewire

- Replace the `has_http → Err(Unsupported)` early-return with: build
  `HostAllow` from the plan (same match as darwin `lib.rs:167-183`), call
  `spawn_pipe_proxy`, and nest its handle in the `CapabilityHandle`
  (mirrors darwin's `handle.nest_handle`).
- Rebuild becomes `launcher --profile P -- tau-net-bridge-win --pipe <name>
  -- <orig-program> <orig-args>` for HTTP plans; non-HTTP plans keep the
  current shape. Bridge exe resolved like the launcher
  (`TAU_NET_BRIDGE_WIN_PATH` env override, else PATH).
- Grant read+execute ACL on the bridge exe (and keep granting nothing else
  new): the container must be able to image-load it.
- `supported_shapes()` gains `NetworkHttp`; probe `details` becomes
  `"AppContainer (FS + process isolation + proxied egress)"`; update
  `supported_shapes_is_fs_and_exec` test and any tier registry strings in
  `tau-runtime-tokio`.
- **#617 invariant untouched:** the rebuild still happens inside
  `wrap_spawn`, callers still set piped stdio + `kill_on_drop` after it.
  `capability_sids` stays empty — the launcher's `--cap` plumbing remains
  dormant (kept for future capability needs, not used by this design).

### 5. Positive-FS reachability (item 2) — tests-only, per H3

H3 refuted ADR-0067's premise: leaf-only ACL grants ARE reachable at
nested paths (AppContainer tokens retain `SeChangeNotifyPrivilege`,
bypass traverse checking). No `FILE_TRAVERSE` ancestor-grant code is
needed. Item 2 reduces to the positive-FS acceptance tests below plus
the ADR-0067 amendment correcting the premise. (The spike's H3 probe is
promoted to a permanent positive-FS regression test in PR2 so a future
Windows hardening change that strips the privilege is caught.)

## Error handling

| Failure | Behavior |
|---|---|
| pipe creation / SDDL / accept-task spawn fails | `CapabilityError::Proxy`, plan refused (fail-closed) |
| bridge exe missing / not grantable | `CapabilityError::WrapFailed`, plan refused |
| plugin dials a non-allowlisted host | proxy answers 403 (`HostAllow`), plugin sees a failed request |
| bridge crashes mid-run | child keeps running but has no egress (connections refused); exit propagates via launcher/job object as today |
| pipe proxy handle dropped | accept task aborted; pipe instances close; container can no longer egress |

## Testing

- **Acceptance (Windows, tier-2 `full-matrix`):** real `kind = "rust-cargo"`
  install succeeds under the graduated adapter WITHOUT
  `--allow-unsandboxed-build` (exercises egress + positive FS reads
  end-to-end; ADR-0067 names this the criterion for both items).
- **Positive-FS:** plugin reads/writes a nested granted path through the
  launcher (item 2; the cargo test doubles as this, plus a focused test).
- **Negative guards kept/added:** unlisted host → 403 (denied); sibling path
  → still denied; a second AppContainer cannot open the pipe (H2-control
  promoted to a permanent test).
- **Cross-platform (tier-0):** proxy genericization keeps all existing unix
  proxy tests green; bridge/launcher arg parsing unit tests run everywhere.
- **Unit (Windows):** pipe proxy lifecycle (drop closes), traverse-grant
  walk (if H3 says it's needed).

## PR sequencing

1. **PR1 (cross-platform):** `tau-sandbox-proxy` handler genericization. No
   Windows code; tier-0 proves no Unix regression.
2. **PR2 (Windows):** `pipe_proxy` + `tau-net-bridge-win` + `wrap_spawn`
   rewire + shapes/probe flip + all Windows tests (incl. the promoted
   positive-FS regression test). `full-matrix` label.
3. **PR3 (docs):** ADR-0067 amendment with spike measurements (drafted
   alongside this spec), escape-hatch/docs updates, close #622.

Crates touched: `tau-sandbox-proxy`, `tau-sandbox-windows`, possibly
`tau-cli` (install acceptance test) and `tau-runtime-tokio` (tier/registry
strings). `tau-domain`/`tau-ports` untouched — no new shapes, no port
changes, no semver bumps expected.
