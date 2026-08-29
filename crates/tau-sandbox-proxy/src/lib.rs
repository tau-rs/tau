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

/// Opt-in stderr tracing for the proxy's data path.
///
/// `tracing` events are the primary instrument, but the Windows
/// AppContainer egress chain (`tau-sandbox-windows`) runs with **no
/// subscriber installed** — the events are invisible exactly when the
/// data path breaks, which cost #622 a full 20-minute CI round. Setting
/// `TAU_SANDBOX_PROXY_TRACE` to a non-empty, non-`0` value additionally
/// prints each decision point to stderr with a `PROXY ` prefix, matching
/// the `BRIDGE ` / `PIPEPROXY ` markers on the other two hops so a stall
/// can be localised to one hop from one log.
///
/// Read once and cached: off by default, so nothing about the Unix /
/// container proxy path changes unless a caller opts in.
fn trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("TAU_SANDBOX_PROXY_TRACE")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

macro_rules! ptrace {
    ($($arg:tt)*) => {
        if $crate::trace_enabled() {
            eprintln!("PROXY {}", format_args!($($arg)*));
        }
    };
}

/// Escape a request-head slice for a single-line stderr marker.
fn head_preview(buf: &[u8]) -> String {
    let cut = buf.len().min(120);
    String::from_utf8_lossy(&buf[..cut])
        .escape_debug()
        .to_string()
}

/// Host egress policy for the proxy. `Any` = pass-all (reachable only from a
/// `HostSet::Any` capability); `Exact` = only these (pre-validated, lowercase)
/// hosts. Case-insensitive matching at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAllow {
    /// Allow every host (pass-all).
    Any,
    /// Allow exactly these hosts (case-insensitive).
    Exact(Vec<String>),
}

impl HostAllow {
    /// True iff `host` is permitted. Case-folds both sides.
    pub fn permits(&self, host: &str) -> bool {
        match self {
            HostAllow::Any => true,
            HostAllow::Exact(list) => list.iter().any(|h| h.eq_ignore_ascii_case(host)),
        }
    }
}

// Platform-agnostic unit coverage for the host policy (the `#[cfg(unix)]`
// integration tests below exercise the same logic through the proxy, but only
// on unix; this proves the property deterministically, with no network).
#[cfg(test)]
mod host_allow_tests {
    use super::HostAllow;

    #[test]
    fn any_permits_every_host() {
        assert!(HostAllow::Any.permits("anything.example.com"));
        assert!(HostAllow::Any.permits("EVEN.MIXED.Case"));
    }

    #[test]
    fn exact_membership_is_case_insensitive() {
        let policy = HostAllow::Exact(vec!["allowed.example.com".to_string()]);
        assert!(policy.permits("allowed.example.com"));
        assert!(policy.permits("ALLOWED.EXAMPLE.COM"));
        assert!(!policy.permits("denied.example.com"));
    }
}

// The async runtime code below is unix-only — it relies on Unix-domain
// sockets (`tokio::net::Unix*`). The strict-tier sandbox is also unix-only
// (landlock, seccomp, namespaces), so this module's runtime API is only
// reachable on unix-target builds. Pure-logic parts above (validate,
// connect parsing) compile on any platform.

#[cfg(unix)]
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixListener;
// `UnixStream` is only named explicitly in the unix integration tests below
// (`accept_loop` gets it via `UnixListener::accept()` type inference, so the
// non-test unix build never names the type itself).
#[cfg(all(unix, test))]
use tokio::net::UnixStream;
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
pub fn spawn_proxy(hosts: HostAllow) -> std::io::Result<ProxyHandle> {
    let (sock_dir, sock_path) = make_run_dir_and_sock_path()?;
    let listener = UnixListener::bind(&sock_path)?;
    // Make the socket inode other-writable (`0o666`). `connect(2)` to a Unix
    // socket requires *write* permission on the inode, and the two legitimate
    // callers reach it as neither owner nor a CAP_DAC_OVERRIDE-capable process:
    // the container bridge bind-mounts the inode and, under rootful Docker,
    // runs as uid 0 with `--cap-drop=ALL` (no DAC_OVERRIDE) while the socket is
    // owned by the host user that spawned the proxy — so it is "other" and only
    // the other-write bit lets it connect. (Rootless Podman maps container-root
    // to the socket's owner, so owner-write sufficed there — which is why the
    // OS-default `0o755` mode silently regressed only Docker CI.) The `0o700`
    // per-run dir (see `make_run_dir_and_sock_path`) — not the socket mode —
    // remains the access boundary against other *local* users, who cannot
    // traverse into the dir to reach the socket regardless of its mode. This
    // preserves the S6 hardening while restoring the container path.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o666))?;
    }
    let task = tokio::spawn(accept_loop(listener, hosts));
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
/// The socket file itself is set other-writable (`0o666`) by [`spawn_proxy`]
/// after bind — see the rationale there: a container-root bridge without
/// CAP_DAC_OVERRIDE must be able to `connect(2)` to the bind-mounted inode. No
/// other local user can traverse the `0o700` directory to reach the socket, so
/// the socket mode is not the boundary; the native bridge runs as the same host
/// user that owns the directory (so DAC traversal is permitted; landlock grants
/// the socket path itself).
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
async fn accept_loop(listener: UnixListener, hosts: HostAllow) {
    loop {
        match listener.accept().await {
            Ok((mut conn, _)) => {
                let hosts = hosts.clone();
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
    let mut buf = [0u8; 4096];
    let n = conn.read(&mut buf).await?;
    ptrace!("head read n={n} bytes head={:?}", head_preview(&buf[..n]));
    let first_line: &[u8] = match buf[..n].iter().position(|&b| b == b'\n') {
        Some(idx) => &buf[..idx],
        None => &buf[..n],
    };
    if first_line.starts_with(b"CONNECT ") {
        handle_connect(conn, &buf[..n], hosts).await
    } else {
        handle_http(conn, &buf[..n], hosts).await
    }
}

async fn handle_connect<S>(
    plugin_sock: &mut S,
    initial: &[u8],
    hosts: &HostAllow,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let req = match parse_connect_request(initial) {
        Ok(r) => r,
        Err(_) => {
            ptrace!("connect parse FAILED -> 400");
            plugin_sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
            return Ok(());
        }
    };
    ptrace!("connect parsed host={} port={}", req.host, req.port);
    if !hosts.permits(&req.host) {
        ptrace!("connect host DENIED host={} -> 403", req.host);
        plugin_sock
            .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
            .await?;
        return Ok(());
    }
    if req.port != 443 {
        ptrace!("connect port-gate REJECT port={} -> 400", req.port);
        plugin_sock
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }
    let mut remote = TcpStream::connect((req.host.as_str(), req.port)).await?;
    ptrace!("connect upstream connect OK {}:{}", req.host, req.port);
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
    ptrace!("connect tunnel established host={} peek={n}B", req.host);
    let (pr, pw) = tokio::io::split(&mut *plugin_sock);
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    ptrace!("connect splice returned host={}", req.host);
    Ok(())
}

/// Splice both directions of a proxied connection and log each direction's
/// outcome under target `tau::proxy`.
///
/// # Termination: response-direction-driven, NOT `join!` (#622 CI round 2)
///
/// This used to be a [`tokio::join!`] of the two copies, which only
/// completes when **both** directions reach EOF. That is unreachable
/// whenever the client stream cannot express a half-close, and one of
/// this function's two callers is exactly that case: on Windows the
/// client is a `NamedPipeServer`, and tokio's
/// `NamedPipeServer::poll_shutdown` / `NamedPipeClient::poll_shutdown`
/// are **no-ops** — they just `poll_flush` and always return `Ready`
/// (verified in `tokio-1.53.1/src/net/windows/named_pipe.rs:922` and
/// `:1711`). So `AsyncWriteExt::shutdown()` never signals half-close over
/// a pipe: the in-container bridge keeps its pipe handle open while it
/// waits for the response, the `client -> remote` copy therefore never
/// sees EOF, `join!` never returns, the handler never returns, the pipe
/// server instance is never closed — and the bridge, in turn, never sees
/// EOF on its own read. Circular non-termination: one stuck task and one
/// leaked pipe instance per request, and the plugin gets its response
/// only if it happens to be flushed before its own read timeout.
///
/// The rule now is: **the exchange is over when the response direction
/// (`remote -> client`) completes.** At that point the upstream has
/// closed (HTTP/1.1 `Connection: close`, or the far end of a CONNECT
/// tunnel went away), so nothing more can arrive for the client and the
/// connection is torn down — which is what closes the pipe handle and
/// releases the peer. The request direction completing is *not* a
/// termination signal: a client that half-closes after sending its
/// request is still waiting for the response, so an `Ok` there is
/// recorded and we keep waiting. An *error* there does end the splice —
/// the client is gone, there is nobody left to deliver a response to.
///
/// CONNECT tunnels keep working: they are long-lived precisely because
/// neither side EOFs, and this only ends them when the remote closes (or
/// the client connection breaks), which is exactly when a tunnel is over.
///
/// Both directions' outcomes are still recorded (the audit-O3 property):
/// the abandoned direction is logged as `incomplete` with no byte count
/// rather than being silently dropped.
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
    let up = tokio::io::copy(&mut client_r, &mut remote_w);
    let down = tokio::io::copy(&mut remote_r, &mut client_w);
    tokio::pin!(up, down);

    let mut up_res: Option<std::io::Result<u64>> = None;
    let mut down_res: Option<std::io::Result<u64>> = None;
    loop {
        tokio::select! {
            // Disabled once it has produced a value: a completed future
            // must never be polled again.
            r = &mut up, if up_res.is_none() => {
                let client_gone = r.is_err();
                up_res = Some(r);
                if client_gone {
                    break;
                }
            }
            r = &mut down => {
                down_res = Some(r);
                break;
            }
        }
    }

    log_direction(host, "client->remote", up_res);
    log_direction(host, "remote->client", down_res);
}

/// Emit the `tau::proxy` event for one splice direction.
///
/// `None` means the direction was still in flight when the connection was
/// torn down (see [`splice_bidirectional`]) — recorded rather than dropped.
fn log_direction(host: &str, direction: &'static str, res: Option<std::io::Result<u64>>) {
    match res {
        Some(Ok(bytes)) => {
            ptrace!("splice {direction} host={host} bytes={bytes}");
            tracing::debug!(
                target: "tau::proxy",
                host = %host,
                bytes,
                direction,
                "proxy splice direction complete"
            )
        }
        Some(Err(e)) => {
            ptrace!("splice {direction} host={host} FAILED err={e}");
            tracing::warn!(
                target: "tau::proxy",
                host = %host,
                error = %e,
                direction,
                "proxy splice direction failed mid-stream"
            )
        }
        None => {
            ptrace!("splice {direction} host={host} incomplete (torn down)");
            tracing::debug!(
                target: "tau::proxy",
                host = %host,
                direction,
                "proxy splice direction abandoned when the peer direction finished"
            )
        }
    }
}

/// True if `host` is an IP literal that is a loopback address
/// (`127.0.0.0/8` or `::1`). Non-IP hostnames are never loopback.
/// Matches the loopback semantics of [`validate::validate_hosts`].
fn is_loopback_host(host: &str) -> bool {
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Plaintext HTTP egress port policy. Mirrors [`handle_connect`]'s 443-only
/// rule: a remote (non-loopback) host may only be reached on the well-known
/// HTTP port 80; loopback hosts may use any port so local servers (e.g. a
/// local model server on `http://127.0.0.1:11434`) keep working.
fn http_port_allowed(host: &str, port: u16) -> bool {
    is_loopback_host(host) || port == 80
}

async fn handle_http<S>(
    plugin_sock: &mut S,
    initial: &[u8],
    hosts: &HostAllow,
) -> std::io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let req = match parse_http_request(initial) {
        Ok(r) => r,
        Err(e) => {
            ptrace!("http parse FAILED err={e} -> 400");
            plugin_sock
                .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
                .await?;
            return Ok(());
        }
    };
    ptrace!(
        "http parsed method={} host={} port={} path={} line_end={}",
        req.method,
        req.host,
        req.port,
        req.path_and_query,
        req.line_end
    );
    if !hosts.permits(&req.host) {
        ptrace!("http host DENIED host={} -> 403", req.host);
        plugin_sock
            .write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n")
            .await?;
        return Ok(());
    }
    // Mirror handle_connect's port restriction on the plaintext path: a
    // remote allowlisted host is reachable over plaintext only on port 80;
    // loopback hosts may use any port (local servers).
    if !http_port_allowed(&req.host, req.port) {
        ptrace!(
            "http port-gate REJECT host={} port={} -> 400",
            req.host,
            req.port
        );
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
        Ok(s) => {
            ptrace!("http upstream connect OK {}:{}", req.host, req.port);
            s
        }
        Err(e) => {
            ptrace!(
                "http upstream connect FAILED {}:{} err={e} -> 502",
                req.host,
                req.port
            );
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
    ptrace!(
        "http forwarded head line={}B rest={}B",
        rewritten.len(),
        initial.len().saturating_sub(req.line_end)
    );
    // Splice both directions for the rest of the conversation.
    let (pr, pw) = tokio::io::split(&mut *plugin_sock);
    let (rr, rw) = remote.split();
    splice_bidirectional(pr, pw, rr, rw, &req.host).await;
    ptrace!("http splice returned host={}", req.host);
    Ok(())
}

/// One-shot loopback upstream shared by the tests below.
#[cfg(test)]
mod test_upstream {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// Drain the WHOLE forwarded request head, then write `resp`, then
    /// half-close so the peer observes a clean FIN.
    ///
    /// # Why draining matters (#622 CI round 3)
    ///
    /// [`handle_http`](super::handle_http) forwards a request as **two**
    /// writes — the rewritten request line, then the rest of the head it
    /// had already buffered. A fixture that issues a single `read()` and
    /// then drops the socket leaves the second write unread in the
    /// receive buffer; on Windows, closing a socket with unread received
    /// data aborts the connection with an **RST** instead of a FIN,
    /// which destroys the response still in flight the other way. That
    /// is exactly how `tau-sandbox-windows`'s
    /// `egress_allowlisted_host_succeeds_through_full_chain` failed
    /// (`splice remote->client FAILED ... os error 10054`).
    ///
    /// These two fixtures had the identical shape and only survived
    /// because loopback usually coalesces both writes into one read —
    /// i.e. they were latently flaky on Windows CI, not correct.
    ///
    /// Bounded by a byte cap and a per-read timeout: a regression must
    /// fail the assertion, never hang the job.
    pub(super) async fn answer_after_draining_head(s: &mut TcpStream, resp: &[u8]) {
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 64 * 1024 {
            match tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut chunk)).await
            {
                Ok(Ok(n)) if n > 0 => head.extend_from_slice(&chunk[..n]),
                // EOF, read error, or timeout: answer with what we have
                // and let the test's own assertion report the problem.
                _ => break,
            }
        }
        s.write_all(resp).await.expect("write resp");
        s.shutdown().await.expect("shutdown upstream write side");
    }
}

#[cfg(unix)]
#[cfg(test)]
mod proxy_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn socket_lives_in_private_0700_dir() {
        use std::os::unix::fs::PermissionsExt;
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
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
    async fn socket_file_is_other_writable_for_container_root_connect() {
        use std::os::unix::fs::PermissionsExt;
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
        let mode = std::fs::metadata(h.sock_path())
            .expect("sock metadata")
            .permissions()
            .mode();
        // `connect(2)` to a Unix socket requires *write* permission on the
        // socket inode. The container bridge reaches this socket through a
        // bind-mount of the inode and, under rootful Docker, runs as uid 0
        // with `--cap-drop=ALL` (no CAP_DAC_OVERRIDE) while the socket is
        // owned by the host user that spawned the proxy — so container-root is
        // "other" and can only connect if the other-write bit is set. The
        // `0o700` parent dir (asserted in `socket_lives_in_private_0700_dir`)
        // remains the real access boundary against other local users.
        assert_eq!(
            mode & 0o002,
            0o002,
            "proxy socket must be other-writable (got {:o}) so a container-root \
             bridge without CAP_DAC_OVERRIDE can connect(2) via bind-mount",
            mode & 0o777
        );
    }

    #[tokio::test]
    async fn proxy_handle_drop_unlinks_socket_file() {
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
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
            spawn_proxy(HostAllow::Exact(vec!["allowed.example.com".to_string()])).expect("spawn");
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
    async fn pass_all_permits_any_host() {
        let h = spawn_proxy(HostAllow::Any).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        // Non-443 port still 400s, but the host is NOT 403'd under Any:
        conn.write_all(b"CONNECT anything.example.com:443 HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(!s.starts_with("HTTP/1.1 403"), "Any must not 403, got: {s}");
    }

    #[tokio::test]
    async fn exact_match_is_case_insensitive() {
        let h =
            spawn_proxy(HostAllow::Exact(vec!["allowed.example.com".to_string()])).expect("spawn");
        let mut conn = UnixStream::connect(h.sock_path()).await.expect("connect");
        conn.write_all(b"CONNECT ALLOWED.EXAMPLE.COM:443 HTTP/1.1\r\n\r\n")
            .await
            .expect("write");
        let mut resp = [0u8; 256];
        let n = conn.read(&mut resp).await.expect("read");
        let s = std::str::from_utf8(&resp[..n]).expect("utf8");
        assert!(
            !s.starts_with("HTTP/1.1 403"),
            "case-folded host must not 403, got: {s}"
        );
    }

    #[tokio::test]
    async fn malformed_request_returns_400() {
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
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
            spawn_proxy(HostAllow::Exact(vec!["allowed.example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostAllow::Exact(vec!["example.com".to_string()])).expect("spawn");
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
            spawn_proxy(HostAllow::Exact(vec!["allowed.example.com".to_string()])).expect("spawn");
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
        let h = spawn_proxy(HostAllow::Exact(vec!["127.0.0.1".to_string()])).expect("spawn");
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
            // Drain the whole head before answering, then FIN: see
            // `test_upstream::answer_after_draining_head`.
            super::test_upstream::answer_after_draining_head(
                &mut s,
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi",
            )
            .await;
        });

        let h = spawn_proxy(HostAllow::Exact(vec!["127.0.0.1".to_string()])).expect("spawn");
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
            .write_all(
                b"GET http://denied.example.com/ HTTP/1.1\r\nHost: denied.example.com\r\n\r\n",
            )
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.starts_with("HTTP/1.1 403"), "got: {s}");
        task.await.unwrap().unwrap();
    }

    // #622 CI round 2 regression: the handler MUST return once the
    // upstream has answered and closed, even though the client never
    // half-closes its write side.
    //
    // This is the shape the Windows named-pipe front end always has —
    // tokio's `NamedPipe{Server,Client}::poll_shutdown` are no-ops
    // (tokio-1.53.1 named_pipe.rs:922 / :1711), so the in-container
    // bridge cannot signal EOF on the request direction while it waits
    // for the response. Under the old `join!` splice that made
    // `handle_connection` unreturnable, which left the pipe instance
    // open and starved the bridge of its own EOF.
    //
    // `tokio::io::duplex` reproduces it on every platform: the client
    // half is held open for the whole test, so the `client -> remote`
    // copy can never reach EOF. A timeout, not a hang, is the failure
    // mode we want if this ever regresses.
    #[tokio::test]
    async fn handler_returns_when_upstream_closes_without_client_half_close() {
        // One-shot upstream: read the forwarded request, answer, close.
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream");
        let port = upstream.local_addr().expect("addr").port();
        let up = tokio::spawn(async move {
            let (mut s, _) = upstream.accept().await.expect("accept");
            // Drain the whole head before answering, then FIN: see
            // `test_upstream::answer_after_draining_head`.
            super::test_upstream::answer_after_draining_head(
                &mut s,
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi",
            )
            .await;
        });

        let (mut client, mut server) = tokio::io::duplex(4096);
        let hosts = HostAllow::Exact(vec!["127.0.0.1".to_string()]);
        let handler = tokio::spawn(async move { handle_connection(&mut server, &hosts).await });

        let req =
            format!("GET http://127.0.0.1:{port}/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
        client.write_all(req.as_bytes()).await.expect("write req");
        // Deliberately NO `client.shutdown()`: the client keeps its write
        // side open while it waits, exactly like the bridge over a pipe.

        // The response must arrive...
        let mut resp = [0u8; 256];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut resp))
            .await
            .expect("client must receive the response, not time out")
            .expect("read response");
        assert!(
            std::str::from_utf8(&resp[..n])
                .unwrap()
                .starts_with("HTTP/1.1 200"),
            "got: {:?}",
            String::from_utf8_lossy(&resp[..n])
        );

        // ...and the handler must then RETURN (this is what closes the
        // pipe instance on Windows and releases the bridge).
        tokio::time::timeout(std::time::Duration::from_secs(5), handler)
            .await
            .expect("handle_connection must return once the upstream closed")
            .expect("handler task")
            .expect("handler ok");
        up.await.expect("upstream task");
    }

    #[tokio::test]
    async fn malformed_request_gets_400_over_generic_stream() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let hosts = HostAllow::Any;
        let task = tokio::spawn(async move { handle_connection(&mut server, &hosts).await });
        client
            .write_all(b"NOTAMETHOD / HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 128];
        let n = client.read(&mut buf).await.unwrap();
        assert!(std::str::from_utf8(&buf[..n])
            .unwrap()
            .starts_with("HTTP/1.1 400"));
        task.await.unwrap().unwrap();
    }
}
