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
            let _ = windows::Win32::Foundation::LocalFree(windows::Win32::Foundation::HLOCAL(
                self.0 .0,
            ));
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
        assert!(std::str::from_utf8(&buf[..n])
            .unwrap()
            .starts_with("HTTP/1.1 403"));
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
