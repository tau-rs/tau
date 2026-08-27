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
//! ## Why tokio and not `std::fs::File` for the pipe (CI round 1)
//!
//! The first implementation opened the pipe with `std::fs::OpenOptions`
//! and spliced it with two threads (one per direction) over
//! `File::try_clone` handles. On Windows that **deadlocks**: a handle
//! opened without `FILE_FLAG_OVERLAPPED` is a *synchronous* file object,
//! and the I/O manager serialises every operation on a file object —
//! `try_clone` duplicates the handle but not the file object, so the
//! blocking `ReadFile` waiting for the host's response holds the file
//! object's lock and the sibling thread's `WriteFile` (the request!)
//! never issues. Nothing errors: both threads simply wait forever, the
//! plugin gets zero bytes, and the bridge prints nothing — exactly the
//! signature CI round 1 produced (`PROBE result=status detail=` after
//! the probe's 5s read timeout, empty bridge stderr).
//!
//! `tokio::net::windows::named_pipe::ClientOptions` opens the client
//! side with `FILE_FLAG_OVERLAPPED` and drives it through IOCP, so the
//! two directions are genuinely independent. The TCP side moves to
//! tokio for the same reason (one reactor, no blocking threads).
//!
//! ## Observability
//!
//! Every step of the data path prints a `BRIDGE ` marker to stderr.
//! This binary runs inside an AppContainer with no tracing subscriber
//! and no log sink; stderr (inherited through the launcher) is the only
//! channel that reaches CI. Errors are never swallowed: an unreadable
//! pipe, a failed relay direction, or a listener that cannot register
//! with the reactor all print, with the raw OS error code.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("tau-net-bridge-win is Windows-only");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
fn main() {
    use tau_sandbox_windows::bridge_args::parse_bridge_args;

    let parsed = match parse_bridge_args(std::env::args_os().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("tau-net-bridge-win: {e}");
            std::process::exit(2);
        }
    };
    match win::run(parsed.pipe, parsed.program, parsed.args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("tau-net-bridge-win: {e}");
            std::process::exit(3);
        }
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::net::TcpListener;

    /// `ERROR_PIPE_BUSY` — every instance of the pipe is currently
    /// serving a client. The host-side accept loop creates the next
    /// instance immediately after a connect completes, so this is a
    /// narrow race, not a failure: retry briefly.
    const ERROR_PIPE_BUSY: i32 = 231;

    /// How long to keep retrying a busy pipe before giving up.
    const PIPE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn run(pipe: String, program: OsString, args: Vec<OsString>) -> std::io::Result<i32> {
        // Bind before anything else: the child's proxy env needs the port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        eprintln!("BRIDGE listen port={port} pipe={pipe}");

        // Multi-thread ON PURPOSE: this thread blocks in `child.wait()`
        // for the whole life of the plugin, so the accept loop must run
        // on worker threads of its own. A current-thread runtime here
        // would never be polled and the plugin would see a silent
        // black hole (the same failure shape the host-side tests avoid
        // with `spawn_blocking`).
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("BRIDGE runtime-build FAILED err={e}");
                return Err(e);
            }
        };
        let pipe_path = format!(r"\\.\pipe\{pipe}");
        rt.spawn(accept_loop(listener, pipe_path));

        let proxy_url = format!("http://127.0.0.1:{port}");
        let mut child = match std::process::Command::new(&program)
            .args(&args)
            .env("HTTPS_PROXY", &proxy_url)
            .env("HTTP_PROXY", &proxy_url)
            .env("https_proxy", &proxy_url)
            .env("http_proxy", &proxy_url)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "BRIDGE child-spawn FAILED program={} err={e} os={:?}",
                    program.to_string_lossy(),
                    e.raw_os_error()
                );
                return Err(e);
            }
        };
        eprintln!("BRIDGE child pid={} proxy={proxy_url}", child.id());

        let status = child.wait()?;
        eprintln!("BRIDGE child exit code={:?}", status.code());
        Ok(status.code().unwrap_or(1))
    }

    /// Accept plugin connections on the loopback listener and relay each
    /// one over its own pipe connection.
    async fn accept_loop(listener: std::net::TcpListener, pipe_path: String) {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => {
                eprintln!("BRIDGE listener-ready");
                l
            }
            Err(e) => {
                // Registering a socket with the reactor is the one step
                // that could plausibly be denied inside an AppContainer
                // (mio opens \Device\Afd). Say so out loud.
                eprintln!(
                    "BRIDGE listener-register FAILED err={e} os={:?}",
                    e.raw_os_error()
                );
                return;
            }
        };
        let mut n: u64 = 0;
        loop {
            match listener.accept().await {
                Ok((conn, peer)) => {
                    n += 1;
                    eprintln!("BRIDGE conn={n} accepted peer={peer}");
                    tokio::spawn(relay(n, conn, pipe_path.clone()));
                }
                Err(e) => {
                    eprintln!("BRIDGE accept FAILED err={e} os={:?}", e.raw_os_error());
                    return;
                }
            }
        }
    }

    /// Splice one plugin TCP connection against one fresh pipe
    /// connection to the host proxy, both directions, to EOF.
    async fn relay(id: u64, tcp: tokio::net::TcpStream, pipe_path: String) {
        let pipe = match open_pipe(&pipe_path).await {
            Ok(p) => {
                eprintln!("BRIDGE conn={id} pipe-open ok");
                p
            }
            Err(e) => {
                eprintln!(
                    "BRIDGE conn={id} pipe-open FAILED path={pipe_path} err={e} os={:?}",
                    e.raw_os_error()
                );
                return;
            }
        };
        let (tcp_r, tcp_w) = tcp.into_split();
        let (pipe_r, pipe_w) = tokio::io::split(pipe);
        let up = tokio::spawn(pump(id, "up", tcp_r, pipe_w));
        let down = pump(id, "down", pipe_r, tcp_w).await;
        let up = up.await.unwrap_or(0);
        eprintln!("BRIDGE conn={id} closed up={up}B down={down}B");
    }

    /// Copy `r` into `w` until EOF or error, then half-close `w`.
    /// Returns the byte count; every failure prints its direction and
    /// raw OS error (the round-1 diagnosis gap: this used to `break` on
    /// `Err(_)` with no output at all).
    async fn pump<R, W>(id: u64, dir: &'static str, mut r: R, mut w: W) -> u64
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut buf = vec![0u8; 16 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) => {
                    eprintln!("BRIDGE conn={id} {dir} eof after={total}B");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    eprintln!(
                        "BRIDGE conn={id} {dir} read FAILED after={total}B err={e} os={:?}",
                        e.raw_os_error()
                    );
                    break;
                }
            };
            if let Err(e) = w.write_all(&buf[..n]).await {
                eprintln!(
                    "BRIDGE conn={id} {dir} write FAILED after={total}B err={e} os={:?}",
                    e.raw_os_error()
                );
                break;
            }
            total += n as u64;
        }
        // Half-close so the peer sees EOF instead of hanging on its own
        // read (no-op on the pipe half; a real FIN on the TCP half).
        if let Err(e) = w.shutdown().await {
            eprintln!(
                "BRIDGE conn={id} {dir} shutdown FAILED err={e} os={:?}",
                e.raw_os_error()
            );
        }
        total
    }

    /// Open the host proxy's pipe, retrying only `ERROR_PIPE_BUSY`.
    async fn open_pipe(path: &str) -> std::io::Result<NamedPipeClient> {
        let deadline = Instant::now() + PIPE_BUSY_TIMEOUT;
        loop {
            match ClientOptions::new().open(path) {
                Ok(c) => return Ok(c),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
