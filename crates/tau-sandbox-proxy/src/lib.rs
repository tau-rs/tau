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

use tau_domain::{Capability, NetCapability, NetHosts};

/// The host egress policy the proxy enforces. Derived from a plan's
/// `net.http` capabilities: [`NetHosts::Any`] anywhere yields
/// [`HostPolicy::Any`] (unrestricted egress); otherwise the union of the
/// explicit host lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPolicy {
    /// Unrestricted egress — every host is allowed (`net.http hosts = "any"`).
    Any,
    /// Egress restricted to this exact host allow-list.
    List(Vec<String>),
}

impl HostPolicy {
    /// Fold a plan's capabilities into a single egress policy. Any
    /// `net.http` capability granting [`NetHosts::Any`] makes the whole
    /// policy [`HostPolicy::Any`]; otherwise the host lists are unioned.
    pub fn from_capabilities(caps: &[Capability]) -> Self {
        let mut list = Vec::new();
        for cap in caps {
            if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
                match hosts {
                    NetHosts::Any => return HostPolicy::Any,
                    NetHosts::List(h) => list.extend(h.iter().cloned()),
                }
            }
        }
        HostPolicy::List(list)
    }

    /// `true` if `host` is permitted to egress under this policy.
    pub fn allows(&self, host: &str) -> bool {
        match self {
            HostPolicy::Any => true,
            HostPolicy::List(hosts) => hosts.iter().any(|h| h == host),
        }
    }

    /// `true` if this policy permits no egress at all (empty explicit list).
    pub fn is_empty(&self) -> bool {
        matches!(self, HostPolicy::List(h) if h.is_empty())
    }

    /// Validate the host forms the proxy can enforce (rejects wildcards /
    /// non-loopback IP literals). [`HostPolicy::Any`] has nothing to
    /// validate.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            HostPolicy::Any => Ok(()),
            HostPolicy::List(hosts) => validate_hosts(hosts),
        }
    }
}

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
    sock_dir: PathBuf,
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
        // Remove the per-run directory too (best-effort; only succeeds once
        // the socket file inside it is gone).
        let _ = std::fs::remove_dir(&self.sock_dir);
    }
}

/// Spawn a tokio task that listens for HTTP CONNECT requests on a
/// temp Unix socket file. Returns a `ProxyHandle` whose Drop cleans up.
///
/// Caller is responsible for granting the child access to the returned
/// socket path (e.g. via landlock rules for native, bind-mount for container)
/// so the bridge inside the sandbox can dial it.
#[cfg(unix)]
pub fn spawn_proxy(policy: HostPolicy) -> std::io::Result<ProxyHandle> {
    let (sock_dir, sock_path) = make_run_dir_and_sock_path()?;
    let listener = UnixListener::bind(&sock_path)?;
    let task = tokio::spawn(accept_loop(listener, policy));
    Ok(ProxyHandle {
        sock_path,
        sock_dir,
        task,
    })
}

/// Create a private per-run directory (mode `0o700`) in the system temp dir
/// and return `(dir, socket_path)` for the proxy's Unix socket.
///
/// The `0o700` directory — not the socket file mode — is the access boundary,
/// mirroring the ssh-agent / gpg-agent socket-in-a-private-dir pattern. This
/// replaces the former world-writable (`0o666`) socket sitting directly in
/// shared `/tmp`, which let any local user dial the proxy and relay egress to
/// allowlisted hosts for the lifetime of a run (audit S6).
///
/// The socket file is left at the OS default mode: no other local user can
/// traverse the `0o700` directory to reach it, while the two legitimate
/// callers are unaffected — the container bridge reaches the socket through a
/// bind-mount of the inode (independent of host-side directory perms) and the
/// native bridge runs as the same host user that owns the directory (so DAC
/// traversal is permitted; landlock grants the socket path itself).
#[cfg(unix)]
fn make_run_dir_and_sock_path() -> std::io::Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("tau-proxy-{}-{}", std::process::id(), n));
    // Clear any stale directory from a prior aborted run, then create fresh
    // at 0o700 so only the owning user can traverse into it.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let sock_path = dir.join("proxy.sock");
    Ok((dir, sock_path))
}

#[cfg(unix)]
async fn accept_loop(listener: UnixListener, policy: HostPolicy) {
    loop {
        match listener.accept().await {
            Ok((mut conn, _)) => {
                let policy = policy.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(&mut conn, &policy).await {
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
    policy: &HostPolicy,
) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = plugin_sock.read(&mut buf).await?;
    let first_line: &[u8] = match buf[..n].iter().position(|&b| b == b'\n') {
        Some(idx) => &buf[..idx],
        None => &buf[..n],
    };
    if first_line.starts_with(b"CONNECT ") {
        handle_connect(plugin_sock, &buf[..n], policy).await
    } else {
        handle_http(plugin_sock, &buf[..n], policy).await
    }
}

#[cfg(unix)]
async fn handle_connect(
    plugin_sock: &mut UnixStream,
    initial: &[u8],
    policy: &HostPolicy,
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
    if !policy.allows(&req.host) {
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
    let (pr, pw) = plugin_sock.split();
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    Ok(())
}

/// Splice both directions of a proxied connection and log each direction's
/// outcome under target `tau::proxy`.
///
/// Uses [`tokio::join!`] rather than `try_join!` so a failure in one direction
/// does not discard the other direction's byte count — both data-path outcomes
/// are always recorded. On success the transferred byte count is logged at
/// debug; on error a `warn!` carries the destination `host` and the io error,
/// so a mid-stream truncation (reset, upstream drop, partial transfer) is no
/// longer silent (audit O3).
#[cfg(unix)]
async fn splice_bidirectional<CR, CW, RR, RW>(
    mut client_r: CR,
    mut client_w: CW,
    mut remote_r: RR,
    mut remote_w: RW,
    host: &str,
) where
    CR: tokio::io::AsyncRead + Unpin,
    CW: tokio::io::AsyncWrite + Unpin,
    RR: tokio::io::AsyncRead + Unpin,
    RW: tokio::io::AsyncWrite + Unpin,
{
    let (up, down) = tokio::join!(
        tokio::io::copy(&mut client_r, &mut remote_w),
        tokio::io::copy(&mut remote_r, &mut client_w),
    );
    match up {
        Ok(bytes) => tracing::debug!(
            target: "tau::proxy",
            host = %host,
            bytes,
            direction = "client->remote",
            "proxy splice direction complete"
        ),
        Err(e) => tracing::warn!(
            target: "tau::proxy",
            host = %host,
            error = %e,
            direction = "client->remote",
            "proxy splice direction failed mid-stream"
        ),
    }
    match down {
        Ok(bytes) => tracing::debug!(
            target: "tau::proxy",
            host = %host,
            bytes,
            direction = "remote->client",
            "proxy splice direction complete"
        ),
        Err(e) => tracing::warn!(
            target: "tau::proxy",
            host = %host,
            error = %e,
            direction = "remote->client",
            "proxy splice direction failed mid-stream"
        ),
    }
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
    policy: &HostPolicy,
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
    if !policy.allows(&req.host) {
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
    let (pr, pw) = plugin_sock.split();
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    Ok(())
}

#[cfg(unix)]
#[cfg(test)]
mod proxy_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn socket_lives_in_private_0700_dir() {
        use std::os::unix::fs::PermissionsExt;
        let h = spawn_proxy(HostPolicy::List(vec!["example.com".to_string()])).expect("spawn");
        let sock = h.sock_path().to_path_buf();
        let dir = sock
            .parent()
            .expect("socket has a parent dir")
            .to_path_buf();
        // The socket must NOT sit directly in the shared temp dir — it must
        // live in a dedicated per-run subdirectory whose perms gate access.
        assert_ne!(
            dir,
            std::env::temp_dir(),
            "socket must live in a private per-run dir, not shared temp"
        );
        // The per-run dir must be 0o700: no group/other access, so no other
        // local user can traverse into it to reach the socket (audit S6).
        let mode = std::fs::metadata(&dir)
            .expect("dir metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o700,
            "per-run dir must be 0o700, got {:o}",
            mode & 0o777
        );
    }

    #[tokio::test]
    async fn proxy_handle_drop_unlinks_socket_file() {
        let h = spawn_proxy(HostPolicy::List(vec!["example.com".to_string()])).expect("spawn");
        let path = h.sock_path().to_path_buf();
        assert!(path.exists(), "socket file should exist after spawn");
        drop(h);
        // Give the OS a beat to unlink
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!path.exists(), "socket file should be unlinked on drop");
    }

    #[tokio::test]
    async fn forbidden_host_returns_403() {
        let h =
            spawn_proxy(HostPolicy::List(vec!["allowed.example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostPolicy::List(vec!["example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostPolicy::List(vec!["example.com".to_string()])).expect("spawn");
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
        let h =
            spawn_proxy(HostPolicy::List(vec!["allowed.example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostPolicy::List(vec!["example.com".to_string()])).expect("spawn");
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
        let h =
            spawn_proxy(HostPolicy::List(vec!["allowed.example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostPolicy::List(vec!["127.0.0.1".to_string()])).expect("spawn");
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
mod splice_logging_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, Layer};
    use tracing_subscriber::prelude::*;

    #[derive(Clone, Default, Debug)]
    struct CapturedEvent {
        target: String,
        host: Option<String>,
        bytes: Option<u64>,
    }

    #[derive(Default)]
    struct CaptureVisitor {
        ev: CapturedEvent,
    }
    impl Visit for CaptureVisitor {
        fn record_u64(&mut self, field: &Field, value: u64) {
            if field.name() == "bytes" {
                self.ev.bytes = Some(value);
            }
        }
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "host" {
                self.ev.host = Some(format!("{value:?}").trim_matches('"').to_string());
            }
        }
    }

    #[derive(Clone, Default)]
    struct CaptureLayer {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }
    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut v = CaptureVisitor::default();
            v.ev.target = event.metadata().target().to_string();
            event.record(&mut v);
            self.events.lock().unwrap().push(v.ev);
        }
    }

    // A clean plaintext-HTTP exchange through the proxy must emit a
    // `tau::proxy` splice event carrying the destination `host` and a `bytes`
    // count — proving the byte-splice result is no longer swallowed (audit O3).
    #[tokio::test]
    async fn splice_emits_event_with_host_and_bytes() {
        let cap = CaptureLayer::default();
        let events = cap.events.clone();
        let subscriber = tracing_subscriber::registry()
            .with(cap.with_filter(tracing_subscriber::filter::LevelFilter::DEBUG));
        let _guard = tracing::subscriber::set_default(subscriber);

        // Loopback upstream: read the forwarded request, reply, then close.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let port = upstream.local_addr().expect("addr").port();
        let up = tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.expect("accept");
            let mut b = [0u8; 1024];
            let _ = s.read(&mut b).await.expect("read req");
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi")
                .await
                .expect("write resp");
            // drop -> clean close -> EOF on the remote->client copy direction
        });

        let h = spawn_proxy(HostPolicy::List(vec!["127.0.0.1".to_string()])).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        let req =
            format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        conn.write_all(req.as_bytes()).await.expect("write req");
        // Half-close our write side so the client->remote copy reaches EOF and
        // the splice can complete cleanly.
        conn.shutdown().await.expect("shutdown write");
        let mut resp = Vec::new();
        let _ = conn.read_to_end(&mut resp).await;
        up.await.expect("upstream task");

        // Let the proxy's spawned connection task finish logging.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let evs = events.lock().unwrap();
        let found = evs.iter().any(|e| {
            e.target == "tau::proxy" && e.host.as_deref() == Some("127.0.0.1") && e.bytes.is_some()
        });
        assert!(
            found,
            "expected a tau::proxy splice event with host+bytes, got: {evs:?}"
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
