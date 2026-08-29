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
//! ## Why the relay ends on the response direction (CI round 2)
//!
//! `AsyncWriteExt::shutdown()` cannot half-close a named pipe: tokio's
//! `NamedPipeClient::poll_shutdown` / `NamedPipeServer::poll_shutdown`
//! are **no-ops** that just `poll_flush` and always return `Ready`
//! (`tokio-1.53.1/src/net/windows/named_pipe.rs:922` and `:1711`). So
//! neither end of the pipe can signal "I am done sending" — only closing
//! the handle does that. The relay therefore ends when the **response**
//! direction (`pipe -> tcp`) finishes: the host proxy closes its pipe
//! instance once the upstream has answered, `down` sees EOF, and the
//! plugin's socket is FIN'd so its own read can return. The `up`
//! direction is then aborted rather than awaited — it is parked on a
//! client read that will never complete, and awaiting it would deadlock
//! the relay task (and leak the pipe instance) for the plugin's whole
//! remaining lifetime.
//!
//! ## Observability
//!
//! Every step of the data path prints a `BRIDGE ` marker to stderr,
//! prefixed with `t=<ms>` since process start. This binary runs inside
//! an AppContainer with no tracing subscriber and no log sink; stderr
//! (inherited through the launcher) is the only channel that reaches CI.
//! Errors are never swallowed: an unreadable pipe, a failed relay
//! direction, or a listener that cannot register with the reactor all
//! print, with the raw OS error code. Both directions report their
//! first bytes (`first-bytes`) and running totals, so "nothing was ever
//! written" is distinguishable from "written but never delivered", and
//! the wall-clock stamps distinguish "returned immediately" from
//! "returned after the peer timed out" — the exact ambiguity that made
//! CI round 2 undiagnosable.

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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::sync::OnceLock;
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

    /// Milliseconds since the bridge started. Every `BRIDGE ` marker
    /// carries this: without it, markers from three different streams
    /// (this process's stderr, the plugin's stdout, the host test's own
    /// stderr) get interleaved by the harness in an order that says
    /// nothing about when anything actually happened.
    fn t() -> u128 {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis()
    }

    pub fn run(pipe: String, program: OsString, args: Vec<OsString>) -> std::io::Result<i32> {
        // Bind before anything else: the child's proxy env needs the port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        eprintln!("BRIDGE t={} listen port={port} pipe={pipe}", t());

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
                eprintln!("BRIDGE t={} runtime-build FAILED err={e}", t());
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
                    "BRIDGE t={} child-spawn FAILED program={} err={e} os={:?}",
                    t(),
                    program.to_string_lossy(),
                    e.raw_os_error()
                );
                return Err(e);
            }
        };
        eprintln!(
            "BRIDGE t={} child pid={} proxy={proxy_url}",
            t(),
            child.id()
        );

        let status = child.wait()?;
        eprintln!("BRIDGE t={} child exit code={:?}", t(), status.code());
        Ok(status.code().unwrap_or(1))
    }

    /// Accept plugin connections on the loopback listener and relay each
    /// one over its own pipe connection.
    async fn accept_loop(listener: std::net::TcpListener, pipe_path: String) {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => {
                eprintln!("BRIDGE t={} listener-ready", t());
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
                    eprintln!("BRIDGE t={} conn={n} accepted peer={peer}", t());
                    tokio::spawn(relay(n, conn, pipe_path.clone()));
                }
                Err(e) => {
                    eprintln!(
                        "BRIDGE t={} accept FAILED err={e} os={:?}",
                        t(),
                        e.raw_os_error()
                    );
                    return;
                }
            }
        }
    }

    /// Splice one plugin TCP connection against one fresh pipe
    /// connection to the host proxy.
    ///
    /// Terminates on the **response** direction (`down`, pipe -> tcp) —
    /// see the module docs. `up` is aborted, not awaited: over a pipe it
    /// can never reach EOF (no half-close), so awaiting it would park
    /// this task forever and hold the pipe instance open. Its byte count
    /// is still reported, via a shared counter that survives the abort.
    async fn relay(id: u64, tcp: tokio::net::TcpStream, pipe_path: String) {
        let pipe = match open_pipe(&pipe_path).await {
            Ok(p) => {
                eprintln!("BRIDGE t={} conn={id} pipe-open ok", t());
                p
            }
            Err(e) => {
                eprintln!(
                    "BRIDGE t={} conn={id} pipe-open FAILED path={pipe_path} err={e} os={:?}",
                    t(),
                    e.raw_os_error()
                );
                return;
            }
        };
        let (tcp_r, tcp_w) = tcp.into_split();
        let (pipe_r, pipe_w) = tokio::io::split(pipe);
        let up_count = Arc::new(AtomicU64::new(0));
        let down_count = Arc::new(AtomicU64::new(0));
        let up = tokio::spawn(pump(id, "up", tcp_r, pipe_w, Arc::clone(&up_count)));
        pump(id, "down", pipe_r, tcp_w, Arc::clone(&down_count)).await;
        // The response is delivered and the plugin's socket is FIN'd.
        // Anything still parked on the request direction is dead weight.
        let up_done = up.is_finished();
        up.abort();
        eprintln!(
            "BRIDGE t={} conn={id} closed up={}B down={}B up_finished={up_done}",
            t(),
            up_count.load(Ordering::Relaxed),
            down_count.load(Ordering::Relaxed),
        );
    }

    /// Copy `r` into `w` until EOF or error, then half-close `w`.
    ///
    /// `total` is mirrored into `counter` as it goes so the caller can
    /// report the byte count even for a direction it had to abort.
    /// Every outcome prints its direction, running total, and raw OS
    /// error; the first bytes to move in each direction print too, so a
    /// direction that was never fed is distinguishable from one whose
    /// bytes were dropped downstream.
    async fn pump<R, W>(id: u64, dir: &'static str, mut r: R, mut w: W, counter: Arc<AtomicU64>)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut buf = vec![0u8; 16 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) => {
                    eprintln!("BRIDGE t={} conn={id} {dir} eof after={total}B", t());
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    eprintln!(
                        "BRIDGE t={} conn={id} {dir} read FAILED after={total}B err={e} os={:?}",
                        t(),
                        e.raw_os_error()
                    );
                    break;
                }
            };
            if total == 0 {
                eprintln!("BRIDGE t={} conn={id} {dir} first-bytes n={n}", t());
            }
            if let Err(e) = w.write_all(&buf[..n]).await {
                eprintln!(
                    "BRIDGE t={} conn={id} {dir} write FAILED after={total}B err={e} os={:?}",
                    t(),
                    e.raw_os_error()
                );
                break;
            }
            total += n as u64;
            counter.store(total, Ordering::Relaxed);
            if total == n as u64 {
                eprintln!("BRIDGE t={} conn={id} {dir} first-write ok n={n}", t());
            }
        }
        // Half-close so the peer sees EOF instead of hanging on its own
        // read. This is a REAL FIN on the TCP half and a documented
        // NO-OP on the pipe half (tokio named_pipe.rs:922 / :1711) —
        // which is precisely why `relay` must not wait for the pipe
        // direction to EOF.
        if let Err(e) = w.shutdown().await {
            eprintln!(
                "BRIDGE t={} conn={id} {dir} shutdown FAILED err={e} os={:?}",
                t(),
                e.raw_os_error()
            );
        }
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
