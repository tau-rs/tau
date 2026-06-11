#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! `tau-sandbox-proxy` — userspace HTTP-CONNECT proxy for tau sandboxed plugins.
//!
//! Shared by both the native (Linux landlock/seccomp) and container
//! (docker/podman) sandbox adapters. Extracted from `tau-sandbox-native`
//! because the proxy logic is purely tokio-based and cross-platform, while
//! `tau-sandbox-native` itself is Linux-specific.
//!
//! Architecture: a tokio task in tau's parent address space accepts
//! Unix-socket connections from the per-plugin `tau-net-bridge` binary.
//! Each connection arrives carrying either:
//!   - An HTTP `CONNECT host:port` request (HTTPS tunnel), or
//!   - A plain HTTP request (`GET http://host/path HTTP/1.1`).
//!
//! The proxy validates the host against the plan's allow-list, then
//! handles accordingly:
//!   - CONNECT: peeks the TLS ClientHello to verify SNI matches, splices.
//!   - Plain HTTP: rewrites the request line to origin-form, splices.
//!
//! Pass-through mode only for HTTPS — proxy does NOT terminate TLS.
//! Plugin's TLS handshake goes end-to-end with the real remote server.

mod connect;
mod http;
mod validate;

pub use connect::{parse_connect_request, peek_sni, ConnectRequest};
pub use http::{parse_http_request, rewrite_request_line, HttpParseError, HttpRequest};
pub use validate::{validate_hosts, ValidationError};

// The async runtime code below is unix-only — it relies on Unix-domain
// sockets (`tokio::net::Unix*`). The strict-tier sandbox is also unix-only
// (landlock, seccomp, namespaces), so this module's runtime API is only
// reachable on unix-target builds. Pure-logic parts above (validate,
// connect parsing) compile on any platform.

#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{TcpStream, UnixListener, UnixStream};
#[cfg(unix)]
use tokio::task::JoinHandle;

/// Handle to a running proxy task. Drop aborts the task and unlinks the
/// temp Unix socket file.
#[cfg(unix)]
#[non_exhaustive]
pub struct ProxyHandle {
    sock_path: PathBuf,
    task: JoinHandle<()>,
}

#[cfg(unix)]
impl ProxyHandle {
    /// Returns the path to the Unix socket the proxy is listening on.
    pub fn sock_path(&self) -> &Path {
        &self.sock_path
    }
}

#[cfg(unix)]
impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.sock_path);
    }
}

/// Spawn a tokio task that listens for HTTP CONNECT requests on a
/// temp Unix socket file. Returns a `ProxyHandle` whose Drop cleans up.
///
/// Caller is responsible for granting the child access to the returned
/// socket path (e.g. via landlock rules for native, bind-mount for container)
/// so the bridge inside the sandbox can dial it.
#[cfg(unix)]
pub fn spawn_proxy(allowed_hosts: Vec<String>) -> std::io::Result<ProxyHandle> {
    use std::os::unix::fs::PermissionsExt;
    let sock_path = make_temp_sock_path()?;
    let listener = UnixListener::bind(&sock_path)?;
    // The container's bridge runs as a non-root user (tau, uid 1000 in
    // tau-plugin-base) whose UID does not match the host user that bound
    // this socket. Make the socket world-writable so the bridge can dial
    // it from inside any sandboxed container regardless of its UID. This
    // is safe: the socket is in a per-pid temp file, and connections are
    // already validated against the plan's host allowlist before any
    // forwarding happens. Removing this chmod produces silent
    // "Permission denied" inside the container — see the Bridge's
    // `proxy connect failed` warning.
    std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o666))?;
    let task = tokio::spawn(accept_loop(listener, allowed_hosts));
    Ok(ProxyHandle { sock_path, task })
}

#[cfg(unix)]
fn make_temp_sock_path() -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut p = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let suffix = format!("tau-proxy-{}-{}.sock", std::process::id(), n);
    p.push(suffix);
    // Ensure the file does not exist (clean state from a prior aborted run)
    let _ = std::fs::remove_file(&p);
    Ok(p)
}

#[cfg(unix)]
async fn accept_loop(listener: UnixListener, allowed_hosts: Vec<String>) {
    loop {
        match listener.accept().await {
            Ok((mut conn, _)) => {
                let hosts = allowed_hosts.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(&mut conn, &hosts).await {
                        tracing::warn!(error = %e, "proxy connection failed");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "proxy accept failed");
                return;
            }
        }
    }
}

#[cfg(unix)]
async fn handle_connection(
    plugin_sock: &mut UnixStream,
    allowed_hosts: &[String],
) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = plugin_sock.read(&mut buf).await?;
    let first_line: &[u8] = match buf[..n].iter().position(|&b| b == b'\n') {
        Some(idx) => &buf[..idx],
        None => &buf[..n],
    };
    if first_line.starts_with(b"CONNECT ") {
        handle_connect(plugin_sock, &buf[..n], allowed_hosts).await
    } else {
        handle_http(plugin_sock, &buf[..n], allowed_hosts).await
    }
}

#[cfg(unix)]
async fn handle_connect(
    plugin_sock: &mut UnixStream,
    initial: &[u8],
    allowed_hosts: &[String],
) -> std::io::Result<()> {
    let req = match parse_connect_request(initial) {
        Ok(r) => r,
        Err(_) => {
            plugin_sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
            return Ok(());
        }
    };
    if !allowed_hosts.iter().any(|h| h == &req.host) {
        plugin_sock
            .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
            .await?;
        return Ok(());
    }
    if req.port != 443 {
        plugin_sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }
    let mut remote = TcpStream::connect((req.host.as_str(), req.port)).await?;
    plugin_sock
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    // Peek the first chunk — should be TLS ClientHello with SNI matching CONNECT host
    let mut peek_buf = [0u8; 1024];
    let n = plugin_sock.read(&mut peek_buf).await?;
    if let Some(sni) = peek_sni(&peek_buf[..n]) {
        if sni != req.host {
            return Err(std::io::Error::other(format!(
                "SNI mismatch: CONNECT={} SNI={}",
                req.host, sni
            )));
        }
    } else {
        return Err(std::io::Error::other("missing SNI in TLS ClientHello"));
    }
    // Forward the peeked bytes onward, then splice
    remote.write_all(&peek_buf[..n]).await?;
    let (mut pr, mut pw) = plugin_sock.split();
    let (mut rr, mut rw) = remote.split();
    let _ = tokio::try_join!(
        tokio::io::copy(&mut pr, &mut rw),
        tokio::io::copy(&mut rr, &mut pw),
    );
    Ok(())
}

/// True if `host` is an IP literal that is a loopback address
/// (`127.0.0.0/8` or `::1`). Non-IP hostnames are never loopback.
/// Matches the loopback semantics of [`validate::validate_hosts`].
#[cfg(unix)]
fn is_loopback_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Plaintext HTTP egress port policy. Mirrors [`handle_connect`]'s 443-only
/// rule: a remote (non-loopback) host may only be reached on the well-known
/// HTTP port 80; loopback hosts may use any port so local servers (e.g. a
/// local model server on `http://127.0.0.1:11434`) keep working.
#[cfg(unix)]
fn http_port_allowed(host: &str, port: u16) -> bool {
    is_loopback_host(host) || port == 80
}

#[cfg(unix)]
async fn handle_http(
    plugin_sock: &mut UnixStream,
    initial: &[u8],
    allowed_hosts: &[String],
) -> std::io::Result<()> {
    let req = match parse_http_request(initial) {
        Ok(r) => r,
        Err(_) => {
            plugin_sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
            return Ok(());
        }
    };
    if !allowed_hosts.iter().any(|h| h == &req.host) {
        plugin_sock
            .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
            .await?;
        return Ok(());
    }
    // Mirror handle_connect's port restriction on the plaintext path: a
    // remote allowlisted host is reachable over plaintext only on port 80;
    // loopback hosts may use any port (local servers).
    if !http_port_allowed(&req.host, req.port) {
        tracing::warn!(
            host = %req.host,
            port = req.port,
            "proxy rejected plaintext HTTP to non-80 port on non-loopback host"
        );
        plugin_sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }
    // Open TCP to the destination host:port.
    let mut remote = match TcpStream::connect((req.host.as_str(), req.port)).await {
        Ok(s) => s,
        Err(e) => {
            plugin_sock
                .write_all(
                    format!("HTTP/1.1 502 Bad Gateway\r\n\r\nupstream connect: {e}\r\n").as_bytes(),
                )
                .await?;
            return Ok(());
        }
    };
    // Send the rewritten request line, then the rest of the original buffer
    // (headers + maybe partial body) verbatim.
    let rewritten = rewrite_request_line(&req);
    remote.write_all(rewritten.as_bytes()).await?;
    remote.write_all(&initial[req.line_end..]).await?;
    // Splice both directions for the rest of the conversation.
    let (mut pr, mut pw) = plugin_sock.split();
    let (mut rr, mut rw) = remote.split();
    let _ = tokio::try_join!(
        tokio::io::copy(&mut pr, &mut rw),
        tokio::io::copy(&mut rr, &mut pw),
    );
    Ok(())
}

#[cfg(unix)]
#[cfg(test)]
mod proxy_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn proxy_handle_drop_unlinks_socket_file() {
        let h = spawn_proxy(vec!["example.com".to_string()]).expect("spawn");
        let path = h.sock_path().to_path_buf();
        assert!(path.exists(), "socket file should exist after spawn");
        drop(h);
        // Give the OS a beat to unlink
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!path.exists(), "socket file should be unlinked on drop");
    }

    #[tokio::test]
    async fn forbidden_host_returns_403() {
        let h = spawn_proxy(vec!["allowed.example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(b"CONNECT denied.example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 403"), "got: {s}");
    }

    #[tokio::test]
    async fn malformed_request_returns_400() {
        let h = spawn_proxy(vec!["example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Use a truly malformed request (no newline at all) so it lands in the
        // HTTP parse error path and returns 400.
        conn.write_all(b"NOTAMETHOD / HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 400"), "got: {s}");
    }

    #[tokio::test]
    async fn non_443_port_returns_400() {
        let h = spawn_proxy(vec!["example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(b"CONNECT example.com:80 HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 400"), "got: {s}");
    }

    #[tokio::test]
    async fn http_forbidden_host_returns_403() {
        let h = spawn_proxy(vec!["allowed.example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(
            b"GET http://denied.example.com/ HTTP/1.1\r\nHost: denied.example.com\r\n\r\n",
        )
        .await
        .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 403"), "got: {s}");
    }

    #[tokio::test]
    async fn http_malformed_returns_400() {
        let h = spawn_proxy(vec!["example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(b"NOTAMETHOD / HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 400"), "got: {s}");
    }

    #[tokio::test]
    async fn http_non_loopback_non_80_port_returns_400() {
        let h = spawn_proxy(vec!["allowed.example.com".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Allowlisted host, but plaintext on a non-80 port: must be rejected,
        // mirroring CONNECT's non-443 -> 400.
        conn.write_all(
            b"GET http://allowed.example.com:8080/ HTTP/1.1\r\nHost: allowed.example.com:8080\r\n\r\n",
        )
        .await
        .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(s.starts_with("HTTP/1.1 400"), "got: {s}");
    }

    #[tokio::test]
    async fn http_loopback_arbitrary_port_not_rejected_by_port_gate() {
        let h = spawn_proxy(vec!["127.0.0.1".to_string()]).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Loopback host on an arbitrary (closed) port: the port gate must NOT
        // reject it. It reaches the upstream-connect path, which fails to dial
        // the closed port and returns 502 — proving the gate let it through.
        conn.write_all(b"GET http://127.0.0.1:9/ HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(
            s.starts_with("HTTP/1.1 502"),
            "expected 502 not 400, got: {s}"
        );
    }
}

#[cfg(unix)]
#[cfg(test)]
mod port_gate_tests {
    use super::{http_port_allowed, is_loopback_host};

    #[test]
    fn remote_host_port_80_allowed() {
        assert!(http_port_allowed("allowed.example.com", 80));
    }

    #[test]
    fn remote_host_non_80_rejected() {
        assert!(!http_port_allowed("allowed.example.com", 8080));
        assert!(!http_port_allowed("allowed.example.com", 443));
    }

    #[test]
    fn loopback_any_port_allowed() {
        assert!(http_port_allowed("127.0.0.1", 11434));
        assert!(http_port_allowed("::1", 9000));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("8.8.8.8"));
        assert!(!is_loopback_host("example.com"));
    }
}
