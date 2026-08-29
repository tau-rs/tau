# Windows AppContainer Egress (#622) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Un-defer network egress + positive-FS on the Windows AppContainer adapter: a named-pipe broker + in-container loopback bridge makes the host-side `HostAllow` proxy the container's only egress path, so real `rust-cargo` installs run sandboxed on Windows without `--allow-unsandboxed-build`.

**Architecture:** `tau-sandbox-proxy`'s per-connection handler is genericized over `AsyncRead + AsyncWrite` (PR1). On Windows (PR2), `tau-sandbox-windows` gains a named-pipe front end for that handler (pipe DACL = current user + the per-spawn AppContainer package SID, plain `\\.\pipe\` namespace) and a `tau-net-bridge-win` first-process that binds an ephemeral loopback port inside the container, forwards conns over the pipe, and spawns the plugin with `HTTPS_PROXY` pointing at itself. No network capability SIDs are granted. PR3 ships the docs (ADR-0067 amendment already committed on `kyoto`).

**Tech Stack:** Rust; tokio (`net`, named pipes); windows 0.58 crate (Win32: SDDL→SD, token user SID); std-only bridge binary.

**Spec:** `docs/superpowers/specs/2026-08-22-windows-egress-design.md` (spike-confirmed; read it first — H1/H2/H3 measurements there justify every choice below).

## Global Constraints

- **CARGO RULES (repo `CLAUDE.md`) — every cargo command:** `timeout <300 test / 180 check / 240 clippy> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <cmd> -p <crate>`. Never bare cargo, never workspace-wide, always `-p`.
- **Windows code cannot run locally** (dev host is macOS). Compile-verify Windows-gated code with `--target x86_64-pc-windows-gnu` (installed). Runtime proof is the tier-2 `nextest / windows` job — PR2 MUST carry the `full-matrix` label.
- `tau-sandbox-proxy` stays `#![forbid(unsafe_code)]`. Unsafe Win32 lives only in `tau-sandbox-windows`, module-scoped `#![allow(unsafe_code)]` like `acl.rs`.
- **#617 invariant:** callers set piped stdio + `kill_on_drop` AFTER `wrap_spawn`; the rebuild inside `wrap_spawn_windows` must not touch stdio.
- **Fail-closed (ADR-0014):** any egress-setup failure (pipe create, SDDL, bridge resolve) returns a typed `CapabilityError`; never silently drop or grant.
- **Pipe names: plain `\\.\pipe\<name>` namespace only.** `LOCAL\` is invisible from inside a container (spike H2b). Pipe DACL must grant BOTH the current user SID (token user part) and the package SID (container part); never `WD`/Everyone — a non-AppContainer process has no container part, so an Everyone ACE would admit any local user.
- **Bridge port is ephemeral** (`127.0.0.1:0`): AppContainers share the host TCP port space; fixed 8443 collides across concurrent spawns.
- No changes to `tau-domain` / `tau-ports` (no new shapes, no semver bumps).
- Conventional commits; commit with `git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit ...`. PRs base `main`; enrol auto-merge with `gh pr merge <N> --squash --delete-branch --auto`; `gh pr update-branch <N>` when BEHIND.
- Branches: PR1 `feat/windows-egress-pr1-proxy-generic` off `origin/main`; PR2 `feat/windows-egress-pr2-pipe-bridge` off `origin/main` AFTER PR1 merges; PR3 is the existing `kyoto` branch (spec + ADR amendment already committed there).

---

### Task 1: Genericize `tau-sandbox-proxy::handle_connection` (PR1)

**Files:**
- Modify: `crates/tau-sandbox-proxy/src/lib.rs` (handler fns at lines ~188–411; the `#[cfg(unix)]` attributes on them)
- Test: same file, new `mod generic_handler_tests`

**Interfaces:**
- Consumes: existing `HostAllow`, `parse_connect_request`, `parse_http_request`, `peek_sni`, `rewrite_request_line` (all already platform-agnostic).
- Produces (PR2 relies on these exact signatures):
  ```rust
  pub async fn handle_connection<S>(conn: &mut S, hosts: &HostAllow) -> std::io::Result<()>
  where
      S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send;
  ```
  `spawn_proxy` (UnixListener) stays `#[cfg(unix)]`, unchanged behavior.

- [ ] **Step 1: Write the failing cross-platform tests**

Add at the bottom of `crates/tau-sandbox-proxy/src/lib.rs` (NOT inside a `#[cfg(unix)]` block):

```rust
#[cfg(test)]
mod generic_handler_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // The generic handler must enforce the host allowlist over ANY
    // duplex byte stream — this is what the Windows named-pipe front
    // end (tau-sandbox-windows) relies on. tokio::io::duplex proves it
    // without any OS socket, on every platform.
    #[tokio::test]
    async fn connect_to_forbidden_host_gets_403_over_generic_stream() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let hosts = HostAllow::Exact(vec!["allowed.example.com".to_string()]);
        let task = tokio::spawn(async move { handle_connection(&mut server, &hosts).await });
        client
            .write_all(b"CONNECT denied.example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.starts_with("HTTP/1.1 403"), "got: {s}");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn plaintext_http_to_forbidden_host_gets_403_over_generic_stream() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let hosts = HostAllow::Exact(vec!["allowed.example.com".to_string()]);
        let task = tokio::spawn(async move { handle_connection(&mut server, &hosts).await });
        client
            .write_all(b"GET http://denied.example.com/ HTTP/1.1\r\nHost: denied.example.com\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.starts_with("HTTP/1.1 403"), "got: {s}");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn malformed_request_gets_400_over_generic_stream() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let hosts = HostAllow::Any;
        let task = tokio::spawn(async move { handle_connection(&mut server, &hosts).await });
        client.write_all(b"NOTAMETHOD / HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("HTTP/1.1 400"));
        task.await.unwrap().unwrap();
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-sandbox-proxy generic_handler`
Expected: COMPILE ERROR — `handle_connection` is private, `#[cfg(unix)]`, and takes `&mut UnixStream`.

- [ ] **Step 3: Genericize the handler chain**

In `crates/tau-sandbox-proxy/src/lib.rs`:

1. Move these imports out of the `#[cfg(unix)]` group so they are unconditional: `use tokio::io::{AsyncReadExt, AsyncWriteExt};` and `use tokio::net::TcpStream;`. Keep `UnixListener`/`UnixStream`, `Path`/`PathBuf`, `JoinHandle` under `#[cfg(unix)]` (JoinHandle is only used by `ProxyHandle`).
2. Remove `#[cfg(unix)]` from: `handle_connection`, `handle_connect`, `handle_http`, `splice_bidirectional`, `is_loopback_host`, `http_port_allowed` (the last two's `#[cfg(unix)]` on `port_gate_tests` also goes away).
3. Change the three handler signatures from `&mut UnixStream` to generics, and make `handle_connection` `pub` with a doc comment:

```rust
/// Serve one proxied connection over any duplex byte stream.
///
/// This is the platform-agnostic core shared by the Unix socket
/// listener ([`spawn_proxy`]) and the Windows named-pipe front end in
/// `tau-sandbox-windows`. Reads one request head, validates the host
/// against `hosts`, then tunnels (CONNECT) or forwards (plain HTTP).
pub async fn handle_connection<S>(conn: &mut S, hosts: &HostAllow) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    // body unchanged, but `plugin_sock` renamed `conn`
}

async fn handle_connect<S>(conn: &mut S, initial: &[u8], hosts: &HostAllow) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{ /* body unchanged except the split — see 4. */ }

async fn handle_http<S>(conn: &mut S, initial: &[u8], hosts: &HostAllow) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{ /* same */ }
```

4. `UnixStream::split()` doesn't exist on a generic `S`. In both `handle_connect` and `handle_http` replace

```rust
let (pr, pw) = plugin_sock.split();
```

with

```rust
let (pr, pw) = tokio::io::split(&mut *conn);
```

(`&mut S` implements `AsyncRead`/`AsyncWrite` when `S: Unpin`, so `tokio::io::split` accepts it; `splice_bidirectional` is already generic over the halves.) The `remote.split()` on `TcpStream` stays as-is.

5. `accept_loop` (unix) now calls the generic fn — no change needed beyond it compiling.

- [ ] **Step 4: Run the full proxy suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-sandbox-proxy`
Expected: PASS — the 3 new generic tests plus every pre-existing unix test (lifecycle, 403/400, splice logging, port gate) unchanged.

- [ ] **Step 5: Cross-check for Windows + lint gates**

Run:
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-sandbox-proxy --target x86_64-pc-windows-gnu
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-sandbox-proxy --all-targets
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-sandbox-proxy -- --check
```
Expected: all clean. The windows-gnu check proves the handler chain (now un-gated) compiles where `UnixStream` doesn't exist.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sandbox-proxy/src/lib.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sandbox-proxy): genericize handle_connection over AsyncRead+AsyncWrite (#622)"
```

---

### Task 2: Open PR1 and merge

**Files:** none (git/gh mechanics)

- [ ] **Step 1: Push branch and open PR**

```bash
git push -u origin feat/windows-egress-pr1-proxy-generic
gh pr create --base main \
  --title "feat(sandbox-proxy): genericize handle_connection over AsyncRead+AsyncWrite (#622)" \
  --body "PR1/3 of #622 (Windows AppContainer egress). Platform-agnostic per-connection handler so the Windows named-pipe front end (PR2) can reuse the HostAllow/CONNECT/SNI/port validation verbatim. No behavior change on unix — all existing proxy tests unchanged. New cross-platform duplex-stream tests cover the generic path. Spec: docs/superpowers/specs/2026-08-22-windows-egress-design.md (lands with PR3).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
```

- [ ] **Step 2: Enrol auto-merge and confirm CI**

```bash
gh pr merge --squash --delete-branch --auto
gh pr checks --watch
```
Expected: Tier-0 green, GitHub auto-merges. If the PR shows `BEHIND` while waiting: `gh pr update-branch <N>`.

---

### Task 3: `pipe_proxy` — Windows named-pipe front end (PR2)

**Files:**
- Create: `crates/tau-sandbox-windows/src/pipe_proxy.rs`
- Modify: `crates/tau-sandbox-windows/src/lib.rs` (add `#[cfg(target_os = "windows")] mod pipe_proxy;`), `crates/tau-sandbox-windows/src/acl.rs` (add `sid_string` + `current_user_sid_string`), `crates/tau-sandbox-windows/Cargo.toml` (windows features + tokio features)
- Test: `crates/tau-sandbox-windows/src/pipe_proxy.rs` (`#[cfg(test)]` mod, Windows-only)

**Interfaces:**
- Consumes: `tau_sandbox_proxy::{handle_connection, HostAllow}` (Task 1), `acl::AppContainerSid` (existing).
- Produces (Task 5 relies on):
  ```rust
  pub(crate) fn spawn_pipe_proxy(
      hosts: tau_sandbox_proxy::HostAllow,
      profile: &acl::AppContainerSid,
  ) -> std::io::Result<PipeProxyHandle>;   // must be called inside a tokio runtime

  pub(crate) struct PipeProxyHandle { /* name, task */ }
  impl PipeProxyHandle {
      /// Bare pipe name, e.g. "tau-proxy-1234-0" (no \\.\pipe\ prefix).
      pub(crate) fn pipe_name(&self) -> &str;
  }
  // Drop aborts the accept task (mirrors unix ProxyHandle).
  ```
  And in `acl.rs`:
  ```rust
  pub(crate) fn sid_string(profile: &AppContainerSid) -> std::io::Result<String>;      // "S-1-15-2-..."
  pub(crate) fn current_user_sid_string() -> std::io::Result<String>;                  // token user SID
  ```

- [ ] **Step 1: Cargo.toml — features**

In `crates/tau-sandbox-windows/Cargo.toml`:
- windows dep features: add `"Win32_System_Pipes"`, `"Win32_System_IO"` (needed by `ConnectNamedPipe` transitively and SDDL conversion lives in the already-present `Win32_Security_Authorization`).
- Ensure the tokio dependency has the named-pipe API: `tokio = { workspace = true, features = ["net", "rt", "io-util"] }`. Check what the workspace default provides first (`grep -A2 '^tokio' Cargo.toml` at the workspace root); only add the features line if `net` isn't already there.

- [ ] **Step 2: acl.rs SID helpers (write + failing compile check)**

Append to `crates/tau-sandbox-windows/src/acl.rs`:

```rust
/// String form ("S-1-15-2-…") of an AppContainer profile's package SID.
pub(crate) fn sid_string(profile: &AppContainerSid) -> std::io::Result<String> {
    use windows::core::PWSTR;
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let psid = sid_for(&profile.profile_name)?;
    let mut s = PWSTR::null();
    let r = unsafe { ConvertSidToStringSidW(psid, &mut s) };
    unsafe { FreeSid(psid) };
    r.map_err(|e| std::io::Error::other(format!("ConvertSidToStringSidW: {e}")))?;
    let out = unsafe { s.to_string() }
        .map_err(|e| std::io::Error::other(format!("sid utf16: {e}")))?;
    unsafe {
        let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
    }
    Ok(out)
}

/// String SID of the current process token's user. Used as the
/// "user part" ACE of the egress pipe's DACL (an AppContainer access
/// check requires BOTH a user-part and a container-part grant; Everyone
/// must not be used — it would admit any non-AppContainer local process).
pub(crate) fn current_user_sid_string() -> std::io::Result<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| std::io::Error::other(format!("OpenProcessToken: {e}")))?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let r = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            len,
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(token);
        r.map_err(|e| std::io::Error::other(format!("GetTokenInformation: {e}")))?;
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(tu.User.Sid, &mut s)
            .map_err(|e| std::io::Error::other(format!("ConvertSidToStringSidW: {e}")))?;
        let out = s
            .to_string()
            .map_err(|e| std::io::Error::other(format!("sid utf16: {e}")))?;
        let _ = LocalFree(HLOCAL(s.as_ptr() as *mut _));
        Ok(out)
    }
}
```

(If `GetTokenInformation`/`OpenProcessToken`/`TOKEN_QUERY` don't resolve, they are under `Win32_Security` + `Win32_System_Threading`, both already in the feature list; fix imports per compiler guidance.)

- [ ] **Step 3: Write `pipe_proxy.rs`**

```rust
//! Named-pipe front end for `tau-sandbox-proxy` on Windows.
//!
//! The host-side accept loop creates `\\.\pipe\<name>` instances whose
//! DACL grants exactly two principals: the current user (token user
//! part of the access check) and one AppContainer package SID (the
//! container part). The in-container `tau-net-bridge-win` dials the
//! pipe and relays the plugin's proxied TCP conns; each pipe
//! connection is served by `tau_sandbox_proxy::handle_connection`, so
//! host allowlisting, CONNECT/SNI verification, and the port policy
//! are identical to Linux/macOS.
//!
//! Win32 FFI (SDDL → SECURITY_ATTRIBUTES) is inherently unsafe; scope
//! the workspace opt-out locally, like `acl.rs`.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

use tau_sandbox_proxy::HostAllow;

use crate::acl;

/// Handle to a running pipe-proxy accept loop. Drop aborts the task;
/// open pipe instances close with it (mirrors the unix `ProxyHandle`).
pub(crate) struct PipeProxyHandle {
    name: String,
    task: tokio::task::JoinHandle<()>,
}

impl PipeProxyHandle {
    /// Bare pipe name (no `\\.\pipe\` prefix) — what the bridge's
    /// `--pipe` argument expects.
    pub(crate) fn pipe_name(&self) -> &str {
        &self.name
    }
}

impl Drop for PipeProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Security descriptor built from SDDL, kept alive for the lifetime of
/// the accept loop (every pipe instance creation reads it).
struct OwnedSd(windows::Win32::Security::PSECURITY_DESCRIPTOR);
// SAFETY: the SD is an opaque LocalAlloc'd buffer only read by Win32
// calls; ownership moves into the accept-loop task.
unsafe impl Send for OwnedSd {}
impl Drop for OwnedSd {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(
                windows::Win32::Foundation::HLOCAL(self.0 .0),
            );
        }
    }
}

fn sddl_to_sd(sddl: &str) -> std::io::Result<OwnedSd> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    let w: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sd = windows::Win32::Security::PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(w.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    }
    .map_err(|e| std::io::Error::other(format!("SDDL '{sddl}': {e}")))?;
    Ok(OwnedSd(sd))
}

fn make_instance(path: &str, sd: &OwnedSd, first: bool) -> std::io::Result<NamedPipeServer> {
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0 .0,
        bInheritHandle: false.into(),
    };
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first);
    // SAFETY: `sa` points at a valid SECURITY_ATTRIBUTES whose SD
    // outlives this call (owned by the accept loop).
    unsafe { opts.create_with_security_attributes_raw(path, &sa as *const _ as *mut _) }
}

/// Spawn the pipe-proxy accept loop for one AppContainer spawn.
/// Must be called from within a tokio runtime (wrap_spawn is async).
pub(crate) fn spawn_pipe_proxy(
    hosts: HostAllow,
    profile: &acl::AppContainerSid,
) -> std::io::Result<PipeProxyHandle> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("tau-proxy-{}-{}", std::process::id(), n);
    let path = format!(r"\\.\pipe\{name}");

    let user = acl::current_user_sid_string()?;
    let pkg = acl::sid_string(profile)?;
    // Protected DACL, two ACEs: user part + container part. GA on a
    // pipe = read/write/etc. — fine, the pipe carries only proxy bytes.
    let sd = sddl_to_sd(&format!("D:P(A;;GA;;;{user})(A;;GA;;;{pkg})"))?;

    // Create the first instance before returning so a racing name-squat
    // fails HERE (fail-closed) rather than inside the task.
    let first = make_instance(&path, &sd, true)?;

    let task = tokio::spawn(accept_loop(path, sd, first, hosts));
    Ok(PipeProxyHandle { name, task })
}

async fn accept_loop(path: String, sd: OwnedSd, first: NamedPipeServer, hosts: HostAllow) {
    let mut server = first;
    loop {
        if let Err(e) = server.connect().await {
            tracing::warn!(error = %e, "pipe proxy accept failed");
            return;
        }
        // Next instance must exist before we serve this one, or a
        // second bridge conn would get ERROR_FILE_NOT_FOUND.
        let next = match make_instance(&path, &sd, false) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "pipe proxy re-listen failed");
                return;
            }
        };
        let mut conn = std::mem::replace(&mut server, next);
        let hosts = hosts.clone();
        tokio::spawn(async move {
            if let Err(e) = tau_sandbox_proxy::handle_connection(&mut conn, &hosts).await {
                tracing::warn!(error = %e, "pipe proxy connection failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Same-process client: the current-user ACE admits us, so the
    // proxy semantics (403 on forbidden host) are testable end-to-end
    // over a real pipe without an AppContainer.
    #[tokio::test]
    async fn forbidden_host_gets_403_over_real_pipe() {
        let profile = format!("tau-pipetest-{}", std::process::id());
        let sid = crate::acl::create_appcontainer_profile(&profile).expect("profile");
        let h = spawn_pipe_proxy(
            HostAllow::Exact(vec!["allowed.example.com".to_string()]),
            &sid,
        )
        .expect("spawn");
        let path = format!(r"\\.\pipe\{}", h.pipe_name());
        let mut client = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&path)
            .expect("client open");
        client
            .write_all(b"CONNECT denied.example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("HTTP/1.1 403"));
        drop(h);
        crate::acl::delete_appcontainer_profile(&profile).ok();
    }

    #[tokio::test]
    async fn drop_aborts_listener() {
        let profile = format!("tau-pipetest-drop-{}", std::process::id());
        let sid = crate::acl::create_appcontainer_profile(&profile).expect("profile");
        let h = spawn_pipe_proxy(HostAllow::Any, &sid).expect("spawn");
        let path = format!(r"\\.\pipe\{}", h.pipe_name());
        drop(h);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let denied = tokio::net::windows::named_pipe::ClientOptions::new().open(&path);
        assert!(denied.is_err(), "pipe should be gone after handle drop");
        crate::acl::delete_appcontainer_profile(&profile).ok();
    }
}
```

Register in `lib.rs` next to `mod acl`:

```rust
#[cfg(target_os = "windows")]
mod pipe_proxy;
```

(If `tokio::time` is missing add the `time` feature; if `ClientOptions` needs a feature it is under tokio `net` — same as the server.)

- [ ] **Step 4: Cross-compile check + clippy + fmt**

Run:
```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-sandbox-windows --target x86_64-pc-windows-gnu --all-targets
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-sandbox-windows --target x86_64-pc-windows-gnu --all-targets
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-sandbox-windows -- --check
```
Expected: clean. (The `#[cfg(test)]` pipe tests run on Windows CI only; here they just have to compile.)

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sandbox-windows
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sandbox-windows): named-pipe front end for tau-sandbox-proxy (#622)"
```

---

### Task 4: `tau-net-bridge-win` — in-container bridge binary

**Files:**
- Create: `crates/tau-sandbox-windows/src/bridge_args.rs` (pure, unit-tested anywhere)
- Create: `crates/tau-sandbox-windows/src/bin/tau-net-bridge-win.rs`
- Modify: `crates/tau-sandbox-windows/src/lib.rs` (add `pub mod bridge_args;`), `crates/tau-sandbox-windows/Cargo.toml` (`[[bin]]` entry)

**Interfaces:**
- Consumes: nothing from other tasks (pure std binary; pipe client side is `std::fs::OpenOptions` — spike H2a proved this works from inside a container).
- Produces (Task 5 relies on): CLI contract
  `tau-net-bridge-win --pipe <name> -- <program> <arg>...`
  Behavior: bind `127.0.0.1:0`; spawn `<program>` with `HTTPS_PROXY`/`HTTP_PROXY`/`https_proxy`/`http_proxy` set to `http://127.0.0.1:<port>`; relay each accepted TCP conn over a fresh open of `\\.\pipe\<name>`; exit with the child's code.

- [ ] **Step 1: Write the failing arg-parsing tests + module**

`crates/tau-sandbox-windows/src/bridge_args.rs`:

```rust
//! Pure arg parsing for `tau-net-bridge-win`. No Win32; unit-tested on
//! any host. CLI contract:
//!   tau-net-bridge-win --pipe <name> -- <prog> <arg>...
use std::ffi::OsString;

/// Parsed bridge invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct BridgeArgs {
    /// Bare pipe name (no `\\.\pipe\` prefix).
    pub pipe: String,
    /// The real program to run (the plugin / cargo).
    pub program: OsString,
    /// Arguments to the real program.
    pub args: Vec<OsString>,
}

/// Parse bridge argv (excluding argv[0]).
pub fn parse_bridge_args(argv: impl Iterator<Item = OsString>) -> Result<BridgeArgs, String> {
    let mut pipe: Option<String> = None;
    let mut it = argv;
    while let Some(a) = it.next() {
        if a == "--" {
            let program = it.next().ok_or_else(|| "missing program after --".to_string())?;
            let args: Vec<OsString> = it.collect();
            let pipe = pipe.ok_or_else(|| "missing --pipe".to_string())?;
            return Ok(BridgeArgs { pipe, program, args });
        } else if a == "--pipe" {
            pipe = Some(
                it.next()
                    .ok_or("--pipe needs a value")?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else {
            return Err(format!("unexpected arg: {}", a.to_string_lossy()));
        }
    }
    Err("missing -- separator / program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_pipe_and_program() {
        let a = parse_bridge_args(os(&["--pipe", "tau-proxy-1-0", "--", "cargo", "build"]).into_iter())
            .unwrap();
        assert_eq!(a.pipe, "tau-proxy-1-0");
        assert_eq!(a.program, OsString::from("cargo"));
        assert_eq!(a.args, os(&["build"]));
    }

    #[test]
    fn missing_pipe_is_error() {
        let e = parse_bridge_args(os(&["--", "prog"]).into_iter()).unwrap_err();
        assert!(e.contains("pipe"), "got {e}");
    }

    #[test]
    fn missing_program_is_error() {
        let e = parse_bridge_args(os(&["--pipe", "p", "--"]).into_iter()).unwrap_err();
        assert!(e.contains("program"), "got {e}");
    }
}
```

Register `pub mod bridge_args;` in `lib.rs` (next to `pub mod launcher_args;`).

- [ ] **Step 2: Run the module tests (they run on macOS)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-sandbox-windows bridge_args`
Expected: PASS (3 tests).

- [ ] **Step 3: Write the bin**

`crates/tau-sandbox-windows/src/bin/tau-net-bridge-win.rs`:

```rust
//! tau-net-bridge-win — first process inside a Windows AppContainer.
//!
//! Windows twin of Linux's `tau-net-bridge` (netns variant): binds an
//! EPHEMERAL loopback port (AppContainers share the host TCP port
//! space, so a fixed port would collide across concurrent spawns),
//! spawns the real plugin as its child with HTTPS_PROXY/HTTP_PROXY
//! pointing at that port, and relays every accepted TCP connection
//! over `\\.\pipe\<name>` to the host-side allowlist proxy.
//!
//! Same-package-SID loopback and SID-DACL'd pipe access were measured
//! on windows-latest (spike #626): the plugin can reach this listener,
//! the pipe reaches the host, and nothing else in or out.
//!
//! Pure std: the pipe client side is a plain file open.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};

use tau_sandbox_windows::bridge_args::parse_bridge_args;

fn main() {
    let parsed = match parse_bridge_args(std::env::args_os().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tau-net-bridge-win: {e}");
            std::process::exit(2);
        }
    };
    match run(parsed.pipe, parsed.program, parsed.args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("tau-net-bridge-win: {e}");
            std::process::exit(3);
        }
    }
}

fn run(pipe: String, program: OsString, args: Vec<OsString>) -> std::io::Result<i32> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let proxy_url = format!("http://127.0.0.1:{port}");

    let mut child = std::process::Command::new(&program)
        .args(&args)
        .env("HTTPS_PROXY", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env("http_proxy", &proxy_url)
        .spawn()?;

    let pipe_path = format!(r"\\.\pipe\{pipe}");
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { return };
            let pipe_path = pipe_path.clone();
            std::thread::spawn(move || {
                if let Err(e) = relay(conn, &pipe_path) {
                    eprintln!("tau-net-bridge-win: relay: {e}");
                }
            });
        }
    });

    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

/// Splice one TCP connection with one fresh pipe connection, both
/// directions, until EOF/error on both.
fn relay(tcp: TcpStream, pipe_path: &str) -> std::io::Result<()> {
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_path)?;

    let mut tcp_r = tcp.try_clone()?;
    let tcp_w = tcp;
    let mut pipe_r = pipe.try_clone()?;
    let mut pipe_w = pipe;

    // tcp -> pipe on this thread's sibling; pipe -> tcp here.
    let up = std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match tcp_r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if pipe_w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        // Best-effort: nothing more to send toward the host.
        let _ = pipe_w.flush();
    });

    let mut buf = [0u8; 16 * 1024];
    let mut tcp_w2 = tcp_w.try_clone()?;
    loop {
        match pipe_r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tcp_w2.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = tcp_w.shutdown(Shutdown::Write);
    let _ = up.join();
    Ok(())
}
```

Add to `crates/tau-sandbox-windows/Cargo.toml` after the launcher `[[bin]]`:

```toml
[[bin]]
name = "tau-net-bridge-win"
path = "src/bin/tau-net-bridge-win.rs"
```

(The bin is pure std and compiles on every platform; on non-Windows the pipe open just fails at runtime, which is fine — it never runs there.)

- [ ] **Step 4: Cross-compile + lint gates**

Same three commands as Task 3 Step 4. Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-sandbox-windows
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sandbox-windows): tau-net-bridge-win in-container bridge (#622)"
```

---

### Task 5: Rewire `wrap_spawn_windows`, flip shapes + probe

**Files:**
- Modify: `crates/tau-sandbox-windows/src/lib.rs` (`supported_shapes` ~97–103, `run_probe` ~162–173, `wrap_spawn_windows` ~175–270, existing unit test `supported_shapes_is_fs_and_exec` ~283–294)
- Modify: `crates/tau-sandbox-windows/src/profile.rs` (stale `has_http` doc comment ~28–35; delete `PROXY_PORT` const ~9–15 and any unit test asserting it)

**Interfaces:**
- Consumes: `pipe_proxy::spawn_pipe_proxy` / `PipeProxyHandle::pipe_name()` (Task 3); bridge CLI contract (Task 4); `CapabilityHandle::nest_handle` (same API darwin uses at `tau-sandbox-darwin/src/lib.rs:233-235`).
- Produces: adapter behavior for Tasks 6–7. Bridge exe resolution order (documented for tests): `TAU_NET_BRIDGE_WIN_PATH` env var, else a `tau-net-bridge-win.exe` sibling of `std::env::current_exe()`, else bare `tau-net-bridge-win` from PATH.

- [ ] **Step 1: Update the shape/probe unit test first (failing)**

Replace `supported_shapes_is_fs_and_exec` in `lib.rs` tests:

```rust
#[test]
fn supported_shapes_includes_network() {
    let s = WindowsSandbox::new("windows");
    let supported = s.supported_shapes();
    assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemRead));
    assert!(supported.contains(&tau_domain::CapabilityShape::FilesystemWrite));
    assert!(supported.contains(&tau_domain::CapabilityShape::ProcessExec));
    assert!(
        supported.contains(&tau_domain::CapabilityShape::NetworkHttp),
        "egress landed with #622 — NetworkHttp must be supported"
    );
}
```

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-sandbox-windows supported_shapes`
Expected: FAIL (NetworkHttp not in the set yet).

- [ ] **Step 2: Flip `supported_shapes` + probe details**

In `supported_shapes()` add `set.insert(tau_domain::CapabilityShape::NetworkHttp);`.
In `run_probe()` change the details string to
`"AppContainer (FS + process isolation + proxied egress via named pipe)"` and rewrite the stale doc comment above it (network is no longer deferred). Search the repo for the old string in case anything asserts on it:
`grep -rn "network egress deferred" crates/` — update every hit (registry/tier strings in `tau-runtime-tokio` if present).

- [ ] **Step 3: Rewire `wrap_spawn_windows`**

Replace the fail-closed block (lines ~213–223) and extend the rebuild:

```rust
    // Egress (#622): spawn the per-container pipe proxy. The container
    // gets NO network capability SIDs — the SID-ACL'd pipe into this
    // HostAllow proxy is its only route out. Fail closed on any setup
    // error (ADR-0014).
    let proxy_handle = if caps.has_http {
        let mut any = false;
        let mut exact: Vec<String> = Vec::new();
        for cap in &plan.capabilities {
            if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
                if hosts.is_any() {
                    any = true;
                } else {
                    exact.extend(hosts.exact_hosts());
                }
            }
        }
        let policy = if any {
            tau_sandbox_proxy::HostAllow::Any
        } else {
            tau_sandbox_proxy::HostAllow::Exact(exact)
        };
        let handle = pipe_proxy::spawn_pipe_proxy(policy, &app_sid).map_err(|e| {
            CapabilityError::Proxy {
                message: format!("spawn_pipe_proxy: {e}"),
            }
        })?;
        Some(handle)
    } else {
        None
    };

    // Resolve the bridge exe (env override -> sibling of tau.exe ->
    // PATH) and grant the container read+execute on it so the image
    // load inside the AppContainer succeeds.
    let bridge_exe: Option<std::path::PathBuf> = if proxy_handle.is_some() {
        let p = std::env::var_os("TAU_NET_BRIDGE_WIN_PATH")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(|d| d.join("tau-net-bridge-win.exe")))
                    .filter(|p| p.exists())
            })
            .unwrap_or_else(|| std::path::PathBuf::from("tau-net-bridge-win"));
        if p.exists() {
            let ps = p.to_string_lossy().into_owned();
            acl::grant_access(&app_sid, &ps, acl::AccessKind::Read).map_err(|e| {
                CapabilityError::WrapFailed {
                    message: format!("grant read on bridge exe {ps}: {e}"),
                }
            })?;
            granted_paths.push((ps, acl::AccessKind::Read));
        }
        Some(p)
    } else {
        None
    };
```

Then, in the rebuild (after `cmd.arg("--profile").arg(&profile_name);` and before `cmd.arg("--")`), route through the bridge for HTTP plans:

```rust
    cmd.arg("--");
    if let (Some(proxy), Some(bridge)) = (&proxy_handle, &bridge_exe) {
        // launcher -- <bridge> --pipe <name> -- <orig> <args...>
        cmd.arg(bridge)
            .arg("--pipe")
            .arg(proxy.pipe_name())
            .arg("--");
    }
    cmd.arg(orig_program).args(orig_args);
```

(Delete the old `cmd.arg("--").arg(orig_program).args(orig_args);` line; env/cwd re-attachment below it is unchanged. Do NOT touch stdio — #617.)

Finally nest the proxy guard in the handle (after the existing `CapabilityHandle::new(...)`):

```rust
    let mut handle = CapabilityHandle::new(move || { /* existing cleanup closure unchanged */ });
    if let Some(p) = proxy_handle {
        handle.nest_handle(Box::new(p));
    }
    Ok(handle)
```

(`nest_handle` requires the nested value to satisfy the same bound darwin's proxy guard does — see `tau-sandbox-darwin/src/lib.rs:233-235`; `PipeProxyHandle` is `Send` because `OwnedSd` is `Send` and `JoinHandle` is `Send`.)

- [ ] **Step 4: Clean up `profile.rs`**

- Delete `PROXY_PORT` (the port is ephemeral now) and any unit test referencing it.
- Rewrite the `has_http` field doc: when true, the spawn layer spawns the per-container pipe proxy and routes the command through `tau-net-bridge-win`; **no capability SIDs are added** (spike #626: same-package loopback and SID-ACL'd pipes need none, and `internetClient` would allow bypassing the allowlist).

- [ ] **Step 5: Run unit tests + gates**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-sandbox-windows
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo check -p tau-sandbox-windows --target x86_64-pc-windows-gnu --all-targets --features integration-tests
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-sandbox-windows --target x86_64-pc-windows-gnu --all-targets --features integration-tests
timeout 30 env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-sandbox-windows -- --check
```
Expected: all pass/clean (`supported_shapes_includes_network` now passes).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-sandbox-windows crates/tau-runtime-tokio 2>/dev/null || git add crates/tau-sandbox-windows
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "feat(sandbox-windows): route HTTP plans through pipe proxy + bridge; NetworkHttp supported (#622)"
```

---

### Task 6: Windows integration tests (egress e2e, negatives, positive-FS)

**Files:**
- Create: `crates/tau-sandbox-windows/src/bin/tau-sandbox-test-probe.rs` (permanent, pure-std test probe)
- Create: `crates/tau-sandbox-windows/tests/egress_integration.rs`
- Modify: `crates/tau-sandbox-windows/Cargo.toml` (`[[bin]]` for the probe), `crates/tau-sandbox-windows/src/lib.rs` (`test_support` additions)

**Interfaces:**
- Consumes: `WindowsSandbox` public API (`ProcessCapabilityGate::wrap_spawn`), launcher + bridge via `TAU_APPCONTAINER_LAUNCHER_PATH` / `TAU_NET_BRIDGE_WIN_PATH` env, and NEW `test_support` helpers (add to the existing `test_support` mod in `lib.rs`):
  ```rust
  /// Grant read+execute on `path` to profile `profile` (spike #626 helper, re-added).
  pub fn grant_read(profile: &str, path: &str) -> std::io::Result<()> {
      let sid = crate::acl::AppContainerSid { profile_name: profile.to_string() };
      crate::acl::grant_access(&sid, path, crate::acl::AccessKind::Read)
  }
  /// Spawn a pipe proxy DACL'd to `profile` and return (bare pipe name, opaque
  /// keep-alive guard). For the foreign-container pipe-access control test.
  pub fn spawn_pipe_proxy(
      profile: &str,
      hosts: tau_sandbox_proxy::HostAllow,
  ) -> std::io::Result<(String, Box<dyn std::any::Any + Send>)> {
      let sid = crate::acl::AppContainerSid { profile_name: profile.to_string() };
      let h = crate::pipe_proxy::spawn_pipe_proxy(hosts, &sid)?;
      Ok((h.pipe_name().to_string(), Box::new(h)))
  }
  ```
- Produces: nothing downstream; these are the EPIC's negative + positive guards.

- [ ] **Step 1: Write the probe bin**

`crates/tau-sandbox-windows/src/bin/tau-sandbox-test-probe.rs` — permanent replacement for the deleted spike probe (same marker style):

```rust
//! In-container test probe for tau-sandbox-windows integration tests.
//!
//! Pure std. Modes (argv[1]):
//! - `http-get <url>` — origin-form GET through the proxy named by the
//!   HTTP_PROXY env var (as a real plugin's HTTP stack would). Prints
//!   the status line; exit 0 iff the response is 200.
//! - `read-file <path>` — read a file; exit 0 on success. Regression
//!   guard for leaf-only ACL reachability (spike #626 H3:
//!   AppContainers retain bypass-traverse-checking).
//! - `pipe-open <name>` — open `\\.\pipe\<name>` read+write; exit 0 on
//!   success. Used by the foreign-container pipe-access control test.
//!
//! An 8s watchdog hard-exits with code 9 so a broken chain never hangs
//! the CI job.

use std::io::{Read, Write};

fn main() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(8));
        println!("PROBE result=watchdog-timeout");
        std::process::exit(9);
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("http-get"), Some(url)) => http_get(url),
        (Some("read-file"), Some(path)) => read_file(path),
        (Some("pipe-open"), Some(name)) => pipe_open(name),
        _ => {
            eprintln!(
                "usage: tau-sandbox-test-probe http-get <url> | read-file <path> | pipe-open <name>"
            );
            std::process::exit(2);
        }
    }
}

fn pipe_open(name: &str) -> ! {
    let path = format!(r"\\.\pipe\{name}");
    match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
        Ok(_) => {
            println!("PROBE result=ok detail=opened {path}");
            std::process::exit(0);
        }
        Err(e) => {
            println!("PROBE result=err detail=open {path}: {e}");
            std::process::exit(1);
        }
    }
}

/// GET `url` (plain http) through the HTTP_PROXY proxy, origin-form —
/// exactly the request shape tau-sandbox-proxy's handle_http expects.
fn http_get(url: &str) -> ! {
    let proxy = std::env::var("HTTP_PROXY").unwrap_or_default();
    let Some(hostport) = proxy.strip_prefix("http://") else {
        println!("PROBE result=err detail=no-http-proxy-env");
        std::process::exit(1);
    };
    let host = url
        .strip_prefix("http://")
        .and_then(|r| r.split('/').next())
        .unwrap_or_default();
    let mut conn = match std::net::TcpStream::connect(hostport) {
        Ok(c) => c,
        Err(e) => {
            println!("PROBE result=err detail=proxy-connect: {e}");
            std::process::exit(1);
        }
    };
    conn.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
    let req = format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if let Err(e) = conn.write_all(req.as_bytes()) {
        println!("PROBE result=err detail=write: {e}");
        std::process::exit(1);
    }
    let mut resp = Vec::new();
    let _ = conn.read_to_end(&mut resp);
    let head = String::from_utf8_lossy(&resp);
    let status = head.lines().next().unwrap_or("");
    println!("PROBE result=status detail={status}");
    if status.contains(" 200 ") {
        std::process::exit(0);
    }
    std::process::exit(1);
}

fn read_file(path: &str) -> ! {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            println!("PROBE result=ok detail=read {} bytes", s.len());
            std::process::exit(0);
        }
        Err(e) => {
            println!("PROBE result=err detail=read {path}: {e}");
            std::process::exit(1);
        }
    }
}
```

Cargo.toml:

```toml
# Pure-std in-container probe used by tests/egress_integration.rs.
[[bin]]
name = "tau-sandbox-test-probe"
path = "src/bin/tau-sandbox-test-probe.rs"
```

- [ ] **Step 2: Write the integration tests**

`crates/tau-sandbox-windows/tests/egress_integration.rs`:

```rust
//! Windows-only: end-to-end egress + positive-FS enforcement for #622.
//! Chain under test: WindowsSandbox::wrap_spawn -> launcher ->
//! tau-net-bridge-win (in-container, ephemeral loopback port) ->
//! SID-DACL'd named pipe -> host-side HostAllow proxy -> upstream.
//!
//! Hermetic: the upstream is a host-loopback HTTP server; the proxy's
//! port policy allows loopback on any port, so no external network is
//! touched.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::io::{Read, Write};
use std::process::Command;

use tau_ports::{CapabilityPlan, ProcessCapabilityGate};
use tau_sandbox_windows::{test_support, WindowsSandbox};

/// One-shot upstream: accepts a single conn, returns 200 "hello".
/// Returns (port, join-handle).
fn spawn_upstream() -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("upstream bind");
    let port = listener.local_addr().expect("addr").port();
    let h = std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            );
        }
    });
    (port, h)
}

fn plan_with_hosts(hosts: &[&str]) -> CapabilityPlan {
    serde_json::from_value(serde_json::json!({
        "capabilities": [
            { "kind": "net.http", "hosts": hosts, "methods": ["GET"] }
        ],
        "context": null,
        "limits": null,
    }))
    .expect("plan decode")
}

/// wrap_spawn the probe under the adapter and run it. Sets the
/// launcher/bridge env overrides to the cargo-built bins.
async fn run_probe_wrapped(plan: &CapabilityPlan, probe_args: &[&str]) -> std::process::Output {
    std::env::set_var(
        "TAU_APPCONTAINER_LAUNCHER_PATH",
        env!("CARGO_BIN_EXE_tau-appcontainer-launcher"),
    );
    std::env::set_var(
        "TAU_NET_BRIDGE_WIN_PATH",
        env!("CARGO_BIN_EXE_tau-net-bridge-win"),
    );
    let gate = WindowsSandbox::new("windows");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tau-sandbox-test-probe"));
    cmd.args(probe_args);
    let _guard = gate.wrap_spawn(plan, &mut cmd).await.expect("wrap_spawn");
    // NB: the probe exe itself needs a read grant for the container.
    // wrap_spawn granted the BRIDGE exe; the probe is the wrapped
    // program — grant it via the plan instead: fs.read on the exe path
    // (see plan_with_hosts_and_read below). Tests that reach here
    // already included it.
    cmd.output().expect("spawn wrapped probe")
}

fn plan_with_hosts_and_read(hosts: &[&str], read_paths: &[&str]) -> CapabilityPlan {
    serde_json::from_value(serde_json::json!({
        "capabilities": [
            { "kind": "net.http", "hosts": hosts, "methods": ["GET"] },
            { "kind": "fs.read", "paths": read_paths }
        ],
        "context": null,
        "limits": null,
    }))
    .expect("plan decode")
}

fn render(out: &std::process::Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

/// The whole chain: allowlisted loopback host fetch returns 200.
#[tokio::test]
async fn egress_allowlisted_host_succeeds_through_full_chain() {
    let (port, upstream) = spawn_upstream();
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let url = format!("http://127.0.0.1:{port}/");
    let out = run_probe_wrapped(&plan, &["http-get", &url]).await;
    assert_eq!(out.status.code(), Some(0), "egress chain failed:\n{}", render(&out));
    upstream.join().ok();
}

/// Negative guard: a host NOT in the allowlist gets the proxy's 403.
#[tokio::test]
async fn egress_unlisted_host_denied() {
    let plan = plan_with_hosts_and_read(
        &["allowed.example.com"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let out = run_probe_wrapped(&plan, &["http-get", "http://denied.example.com/"]).await;
    assert_ne!(out.status.code(), Some(0), "unlisted host must be denied:\n{}", render(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("403"), "expected proxy 403, got:\n{}", render(&out));
}

/// Positive-FS (spike #626 H3, promoted): a leaf-only grant on a
/// nested path is readable — AppContainers retain bypass-traverse.
/// If a future Windows hardening strips SeChangeNotifyPrivilege from
/// AppContainer tokens, this fails and item 2 of ADR-0067's amendment
/// needs revisiting (FILE_TRAVERSE ancestor grants).
#[tokio::test]
async fn leaf_only_grant_readable_at_nested_path() {
    let dir = std::env::temp_dir()
        .join(format!("tau-egress-h3-{}", std::process::id()))
        .join("a/b/c");
    std::fs::create_dir_all(&dir).expect("mkdirs");
    let leaf = dir.join("leaf.txt");
    std::fs::write(&leaf, "hello").expect("write");
    let leaf_str = leaf.to_str().expect("utf8");
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe"), leaf_str],
    );
    let out = run_probe_wrapped(&plan, &["read-file", leaf_str]).await;
    assert_eq!(
        out.status.code(),
        Some(0),
        "leaf-only grant no longer readable (bypass-traverse gone?):\n{}",
        render(&out)
    );
}

/// Sibling isolation stays: a path with NO grant is denied even while
/// its cousin is granted.
#[tokio::test]
async fn ungranted_sibling_path_still_denied() {
    let base = std::env::temp_dir().join(format!("tau-egress-sib-{}", std::process::id()));
    let granted = base.join("granted");
    let sibling = base.join("sibling");
    std::fs::create_dir_all(&granted).expect("mkdirs");
    std::fs::create_dir_all(&sibling).expect("mkdirs");
    let secret = sibling.join("secret.txt");
    std::fs::write(&secret, "secret").expect("write");
    let plan = plan_with_hosts_and_read(
        &["127.0.0.1"],
        &[env!("CARGO_BIN_EXE_tau-sandbox-test-probe")],
    );
    let out = run_probe_wrapped(&plan, &["read-file", secret.to_str().unwrap()]).await;
    assert_ne!(
        out.status.code(),
        Some(0),
        "ungranted sibling must stay denied:\n{}",
        render(&out)
    );
}
```

Also add the promoted spike-H2 control (spec §Testing: "a second AppContainer cannot open the pipe"). This one bypasses `wrap_spawn` — it targets the pipe DACL directly, so it uses `test_support` + the launcher:

```rust
/// Security control (spike #626 H2-control, promoted): a pipe DACL'd
/// to container A must NOT be openable from container B. Guards the
/// per-spawn SID ACE — if someone ever "simplifies" the SDDL to
/// Everyone or ALL APPLICATION PACKAGES, this goes red.
#[tokio::test]
async fn foreign_container_cannot_open_pipe() {
    let owner = format!("tau-egress-own-{}", std::process::id());
    let foreign = format!("tau-egress-for-{}", std::process::id());
    test_support::create_profile(&owner).expect("owner profile");
    test_support::create_profile(&foreign).expect("foreign profile");
    let (pipe_name, _guard) = test_support::spawn_pipe_proxy(
        &owner,
        tau_sandbox_proxy::HostAllow::Any,
    )
    .expect("pipe proxy");
    let probe = env!("CARGO_BIN_EXE_tau-sandbox-test-probe");
    test_support::grant_read(&foreign, probe).expect("grant probe to foreign");
    let out = Command::new(env!("CARGO_BIN_EXE_tau-appcontainer-launcher"))
        .args(["--profile", &foreign, "--", probe, "pipe-open", &pipe_name])
        .output()
        .expect("launcher");
    test_support::delete_profile(&owner).ok();
    test_support::delete_profile(&foreign).ok();
    assert_ne!(
        out.status.code(),
        Some(0),
        "foreign container opened another container's proxy pipe:\n{}",
        render(&out)
    );
}
```

(`tau-sandbox-proxy` must be reachable from the integration test — it already is, as a normal dependency of `tau-sandbox-windows`.)

Note for the implementer: in the other four tests, `wrap_spawn` grants ACLs for every `fs.read` path in the plan (existing behavior), which is how the probe exe and the leaf file become readable — no direct `grant_read` calls needed there.

- [ ] **Step 3: Compile-check + lint (cannot run locally)**

Same three cross-target commands as Task 3 Step 4 (with `--features integration-tests`). Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-sandbox-windows
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "test(sandbox-windows): e2e egress chain + positive-FS + negative guards (#622)"
```

---

### Task 7: Acceptance — sandboxed `rust-cargo` install without `--allow-unsandboxed-build`

**Files:**
- Create: `crates/tau-sandbox-windows/tests/install_rust_cargo_acceptance.rs`
- Modify: `crates/tau-sandbox-windows/Cargo.toml` (dev-dependency `tau-pkg = { workspace = true }`; `tempfile` already present)

**Interfaces:**
- Consumes: `tau_pkg::{install_with_options, InstallOptions, Scope, install_sandbox::{InstallSandbox, InstallSandboxError, InstallSandboxGuard}}` (dev-dep; acyclic — tau-pkg does not depend on tau-sandbox-windows), `tau_domain::PackageSource`, `WindowsSandbox` (Task 5). `InstallOptions` fields: `sandbox: Option<Arc<dyn InstallSandbox>>` (`install.rs:190`), `allow_unsandboxed_build`, `skip_cross_check`.
- Produces: the EPIC's acceptance criterion (ADR-0067 §140–147 un-deferred).

- [ ] **Step 1: Write the test**

```rust
//! Windows-only ACCEPTANCE test for #622: a real `kind = "rust-cargo"`
//! install builds under the graduated AppContainer adapter with
//! `allow_unsandboxed_build = false` — the exact scenario ADR-0067
//! documented as failing closed before the egress follow-on.
//!
//! The fixture has zero registry dependencies so `cargo build` needs no
//! real crates.io traffic, but `build_envelope` still carries the
//! registry hosts, so the plan REQUIRES NetworkHttp — before #622 the
//! adapter refused it at wrap_spawn. Slow (~30s: real cargo build).
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::str::FromStr;
use std::sync::Arc;

use tau_domain::PackageSource;
use tau_pkg::install_sandbox::{InstallSandbox, InstallSandboxError, InstallSandboxGuard};
use tau_pkg::{install_with_options, InstallOptions, Scope};
use tau_sandbox_windows::WindowsSandbox;
use tempfile::TempDir;

/// Sync→async shim: the same bridging `tau-cli`'s RuntimeInstallSandbox
/// does, minimal for the test.
struct WinGate {
    rt: tokio::runtime::Runtime,
    gate: WindowsSandbox,
}

impl InstallSandbox for WinGate {
    fn is_enforced(&self) -> bool {
        true
    }
    fn wrap(
        &self,
        plan: &tau_ports::capability_gate::CapabilityPlan,
        cmd: &mut std::process::Command,
    ) -> Result<InstallSandboxGuard, InstallSandboxError> {
        use tau_ports::ProcessCapabilityGate;
        let handle = self
            .rt
            .block_on(self.gate.wrap_spawn(plan, cmd))
            .map_err(|e| InstallSandboxError::WrapFailed(e.to_string()))?;
        Ok(InstallSandboxGuard::new(handle))
    }
}

// ── fixture helpers (mirrored from tau-pkg/tests/install_builds_rust_cargo_plugin.rs) ──

fn run_git(cwd: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

fn file_url(p: &Path) -> String {
    format!("file://{}", p.display().to_string().replace('\\', "/"))
}

fn make_plugin_fixture_repo(parent: &Path, name: &str) -> PathBuf {
    let bare = parent.join(format!("{name}.git"));
    std::fs::create_dir_all(&bare).unwrap();
    run_git(&bare, &["init", "-q", "--bare", "-b", "main", "."]);
    let working = parent.join(format!("{name}-working"));
    std::fs::create_dir_all(&working).unwrap();
    run_git(&working, &["init", "-q", "-b", "main"]);
    run_git(&working, &["config", "user.email", "test@example.com"]);
    run_git(&working, &["config", "user.name", "Test User"]);
    std::fs::write(
        working.join("tau.toml"),
        format!(
            r#"name = "{name}"
version = "0.1.0"
description = "acceptance fixture for #622"
authors = ["Test <test@example.com>"]
source = "{src}"
kind = "tool"
dependencies = []
capabilities = []

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "{name}"
"#,
            src = file_url(&bare)
        ),
    )
    .unwrap();
    std::fs::write(
        working.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{name}\"\npath = \"src/main.rs\"\n\n[dependencies]\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(working.join("src")).unwrap();
    std::fs::write(working.join("src/main.rs"), "fn main() {}\n").unwrap();
    run_git(&working, &["add", "."]);
    run_git(&working, &["commit", "-q", "-m", "fixture"]);
    run_git(&working, &["remote", "add", "origin", &bare.to_string_lossy()]);
    run_git(&working, &["push", "-q", "origin", "main"]);
    bare
}

#[test]
fn rust_cargo_install_succeeds_sandboxed_without_unsandboxed_escape() {
    // Point the adapter at the cargo-built launcher + bridge.
    std::env::set_var(
        "TAU_APPCONTAINER_LAUNCHER_PATH",
        env!("CARGO_BIN_EXE_tau-appcontainer-launcher"),
    );
    std::env::set_var(
        "TAU_NET_BRIDGE_WIN_PATH",
        env!("CARGO_BIN_EXE_tau-net-bridge-win"),
    );

    let tmp = TempDir::new().unwrap();
    let project_root = tmp.path().join("tau-home");
    std::fs::create_dir_all(&project_root).unwrap();
    let scope = Scope::new_project(&project_root).unwrap();
    let bare = make_plugin_fixture_repo(tmp.path(), "acceptance-plugin");
    let source = PackageSource::from_str(&file_url(&bare)).unwrap();

    let mut opts = InstallOptions::default();
    opts.skip_cross_check = true; // stub bin has no handshake protocol
    opts.allow_unsandboxed_build = false; // THE acceptance condition
    opts.sandbox = Some(Arc::new(WinGate {
        rt: tokio::runtime::Runtime::new().unwrap(),
        gate: WindowsSandbox::new("windows"),
    }));

    let installed = install_with_options(&source, &scope, opts)
        .expect("sandboxed rust-cargo install must succeed without --allow-unsandboxed-build");
    assert_eq!(installed.name.as_str(), "acceptance-plugin");
}
```

- [ ] **Step 2: Add the dev-dependency + compile-check**

`crates/tau-sandbox-windows/Cargo.toml` `[dev-dependencies]`: add `tau-pkg = { workspace = true }`.
Run the cross-target check with `--features integration-tests` (as in Task 3 Step 4). Expected: compiles. Fix field/path names against the real `tau-pkg` API if the compiler disagrees (`InstallOptions` field docs at `crates/tau-pkg/src/install.rs:164-217`).

**Known risk (documented in spec §Testing):** the in-container `cargo build` must read the toolchain (`~/.cargo`, `~/.rustup`) and system DLLs. `build_envelope` (`crates/tau-pkg/src/install_sandbox.rs:104`) grants read on cargo/rustup homes; system dirs carry `ALL APPLICATION PACKAGES` ACEs by default. If CI shows access-denied on a path the envelope misses, extend `build_envelope`'s read set (cross-platform file — keep the addition OS-neutral) in a dedicated commit with a unit test in `install_sandbox.rs`'s existing test mod.

- [ ] **Step 3: Commit**

```bash
git add crates/tau-sandbox-windows
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" commit -m "test(sandbox-windows): rust-cargo install acceptance under AppContainer egress (#622)"
```

---

### Task 8: Open PR2 with `full-matrix`, iterate on Windows CI, merge

**Files:** none (git/gh mechanics)

- [ ] **Step 1: Push + PR + label**

```bash
git push -u origin feat/windows-egress-pr2-pipe-bridge
gh pr create --base main \
  --title "feat(sandbox-windows): AppContainer network egress via named-pipe broker + bridge (#622)" \
  --body "PR2/3 of #622. Pipe-proxy front end (per-spawn DACL: current user + package SID, plain namespace), tau-net-bridge-win in-container bridge (ephemeral loopback port), wrap_spawn rewire, NetworkHttp restored to supported_shapes. E2E egress + negative guards + positive-FS regression + rust-cargo install acceptance, all Windows-gated behind --features integration-tests. Design premises were measured on windows-latest in spike #626 (closed). Spec: docs/superpowers/specs/2026-08-22-windows-egress-design.md.

Preserves the #617 invariant (stdio set after wrap_spawn; rebuild untouched by stdio).

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr edit --add-label full-matrix
```

- [ ] **Step 2: Watch the tier-2 Windows job specifically**

```bash
gh run list --branch feat/windows-egress-pr2-pipe-bridge --workflow "Tier 2 — Heavy validation" --limit 1
gh run watch <run-id>
```
Tier 0 alone is NOT sufficient for this PR — the Windows integration tests only run in tier-2's `nextest / windows` job. Do not merge until that job is green. Iterate on failures (each fix: commit, push — the `synchronize` event re-runs tier-2 while the label is present).

- [ ] **Step 3: Enrol auto-merge once tier-2 windows is green**

```bash
gh pr merge --squash --delete-branch --auto
```
(`gh pr update-branch <N>` if BEHIND; re-enrol with `gh pr merge <N> --auto` bare if auto-merge drops after a re-run.)

---

### Task 9: PR3 — docs (from the `kyoto` branch)

**Files:**
- Already committed on `kyoto`: `docs/superpowers/specs/2026-08-22-windows-egress-design.md`, `docs/decisions/0067-sandbox-windows-appcontainer-phase2.md` (amendment), `docs/superpowers/plans/2026-08-22-windows-egress.md` (this plan)
- Modify (verify only): `docs/reference/escape-hatches.md` — no new escape hatch is introduced by this EPIC (`--allow-unsandboxed-build` already registered); confirm its wording doesn't claim Windows requires it and update the Windows-specific caveat if present.

- [ ] **Step 1: Sync docs with the merged reality**

After PR2 merges: re-read the amendment + spec for statements that changed during CI iteration (e.g. envelope read-set additions from Task 7's risk note). Update if needed. Grep the book for stale claims:
`grep -rn "egress deferred\|fail-closed on network\|allow-unsandboxed-build" docs/ --include="*.md"` — fix hits that describe pre-#622 Windows behavior (likely: ADR-0067 body above the amendment is historical record — leave it; how-to/reference pages are live docs — update them).

- [ ] **Step 2: Book build gate**

```bash
cd docs && PATH="$HOME/.cargo/bin:$PATH" mdbook build && cd .. && rm -rf docs/book
```
Expected: `[INFO]` lines only.

- [ ] **Step 3: Commit (if changes), push kyoto, open PR3**

```bash
git push -u origin kyoto
gh pr create --base main \
  --title "docs: Windows egress design + ADR-0067 amendment (#622)" \
  --body "PR3/3 of #622: spec, implementation plan, and the ADR-0067 amendment recording the spike #626 measurements (same-package-SID loopback, plain-namespace SID-DACL'd pipes, FILE_TRAVERSE premise corrected). Closes #622.

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
gh pr merge --squash --auto
```
(No `--delete-branch`: `kyoto` is the Conductor workspace branch.)

- [ ] **Step 4: Verify #622 closes and update memory**

`gh issue view 622` → CLOSED after merge (the "Closes #622" keyword). Update the auto-memory entry `project_windows_egress_622_spike_2026_08_22.md` to SHIPPED state with PR numbers.
