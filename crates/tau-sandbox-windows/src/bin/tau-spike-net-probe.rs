//! THROWAWAY SPIKE (issue #622) — do not ship, delete after the spike
//! PR reports its CI results.
//!
//! In-container probe half of the Windows egress spike. Runs inside an
//! AppContainer (via `tau-appcontainer-launcher`) and reports, through
//! exit codes + `SPIKE ...` stdout markers, whether the transport the
//! egress design depends on actually works there:
//!
//! - `loopback-serve` — bind `127.0.0.1:0`, spawn self as a child in
//!   `connect <port>` mode (the child inherits the AppContainer token),
//!   accept + read its "ping". Proves/refutes same-package-SID loopback
//!   (the `tau-net-bridge-win` → plugin topology).
//! - `connect <port>` — dial `127.0.0.1:<port>`, send "ping". Used both
//!   as the child of `loopback-serve` and, alone, as the negative
//!   control against a HOST-side listener (must stay blocked).
//! - `pipe-open <name>` — open `\\.\pipe\<name>`, send "ping", expect
//!   "pong". Proves/refutes named-pipe access via a package-SID DACL.
//! - `read-file <path>` — read a file. Used to test whether a leaf-only
//!   ACL grant is reachable at a nested path (ADR-0067's FILE_TRAVERSE
//!   premise, deferred item 2).
//!
//! Every mode arms a watchdog that hard-exits with code 9 after 8s so a
//! refuted hypothesis can never hang the CI job.

use std::io::{Read, Write};

const WATCHDOG_SECS: u64 = 8;
const WATCHDOG_EXIT: i32 = 9;

fn arm_watchdog(mode: &str) {
    let mode = mode.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(WATCHDOG_SECS));
        println!("SPIKE probe={mode} result=watchdog-timeout");
        std::process::exit(WATCHDOG_EXIT);
    });
}

fn fail(mode: &str, detail: impl std::fmt::Display) -> ! {
    println!("SPIKE probe={mode} result=err detail={detail}");
    std::process::exit(1);
}

fn ok(mode: &str, detail: impl std::fmt::Display) -> ! {
    println!("SPIKE probe={mode} result=ok detail={detail}");
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    arm_watchdog(mode);
    match (mode, args.get(1)) {
        ("loopback-serve", None) => loopback_serve(),
        ("connect", Some(port)) => connect(port),
        ("pipe-open", Some(name)) => pipe_open(name),
        ("read-file", Some(path)) => read_file(path),
        _ => {
            eprintln!("usage: tau-spike-net-probe loopback-serve | connect <port> | pipe-open <name> | read-file <path>");
            std::process::exit(2);
        }
    }
}

/// Bind loopback, spawn self as `connect <port>` (same AppContainer, as
/// a child), accept and read "ping".
fn loopback_serve() -> ! {
    const MODE: &str = "loopback-serve";
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => fail(MODE, format_args!("bind: {e}")),
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => fail(MODE, format_args!("local_addr: {e}")),
    };
    println!("SPIKE probe={MODE} step=bound port={port}");

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => fail(MODE, format_args!("current_exe: {e}")),
    };
    let mut child = match std::process::Command::new(&exe)
        .args(["connect", &port.to_string()])
        .spawn()
    {
        Ok(c) => c,
        Err(e) => fail(MODE, format_args!("spawn-child: {e}")),
    };
    println!("SPIKE probe={MODE} step=child-spawned");

    // Non-blocking accept loop with a deadline shorter than the watchdog,
    // so "connection never arrives" is reported as a distinct failure.
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|e| fail(MODE, format_args!("set_nonblocking: {e}")));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut conn = loop {
        match listener.accept() {
            Ok((c, _)) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    let child_state = child
                        .try_wait()
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|e| e.to_string());
                    fail(MODE, format_args!("accept-timeout child={child_state}"));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => fail(MODE, format_args!("accept: {e}")),
        }
    };
    conn.set_nonblocking(false)
        .unwrap_or_else(|e| fail(MODE, format_args!("conn set_blocking: {e}")));
    conn.set_read_timeout(Some(std::time::Duration::from_secs(4)))
        .unwrap_or_else(|e| fail(MODE, format_args!("set_read_timeout: {e}")));
    let mut buf = [0u8; 4];
    if let Err(e) = conn.read_exact(&mut buf) {
        fail(MODE, format_args!("read: {e}"));
    }
    if &buf != b"ping" {
        fail(MODE, format_args!("bad payload: {buf:?}"));
    }
    // Ack so the child can exit only after its payload was delivered
    // (round 1 raced: child process::exit right after write_all aborted
    // the socket with RST 10054 before the server's read).
    if let Err(e) = conn.write_all(b"pong") {
        fail(MODE, format_args!("write ack: {e}"));
    }
    let _ = child.wait();
    ok(MODE, "same-container loopback round-trip");
}

/// Dial `127.0.0.1:<port>`, send "ping", wait for the "pong" ack.
fn connect(port: &str) -> ! {
    const MODE: &str = "connect";
    let port: u16 = match port.parse() {
        Ok(p) => p,
        Err(e) => fail(MODE, format_args!("bad port: {e}")),
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut conn =
        match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(4)) {
            Ok(c) => c,
            Err(e) => fail(MODE, format_args!("connect: {e}")),
        };
    if let Err(e) = conn.write_all(b"ping") {
        fail(MODE, format_args!("write: {e}"));
    }
    // Block for the server's ack instead of exiting straight after the
    // write — process::exit would abort the socket before delivery.
    conn.set_read_timeout(Some(std::time::Duration::from_secs(4)))
        .unwrap_or_else(|e| fail(MODE, format_args!("set_read_timeout: {e}")));
    let mut ack = [0u8; 4];
    if let Err(e) = conn.read_exact(&mut ack) {
        fail(MODE, format_args!("read ack: {e}"));
    }
    ok(MODE, format_args!("round-trip via {addr}"));
}

/// Open a named pipe, send "ping", expect "pong" back.
fn pipe_open(name: &str) -> ! {
    const MODE: &str = "pipe-open";
    let path = format!(r"\\.\pipe\{name}");
    let mut pipe = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => fail(
            MODE,
            format_args!("open {path}: {e} (os error {:?})", e.raw_os_error()),
        ),
    };
    println!("SPIKE probe={MODE} step=opened path={path}");
    if let Err(e) = pipe.write_all(b"ping") {
        fail(MODE, format_args!("write: {e}"));
    }
    let mut buf = [0u8; 4];
    if let Err(e) = pipe.read_exact(&mut buf) {
        fail(MODE, format_args!("read: {e}"));
    }
    if &buf != b"pong" {
        fail(MODE, format_args!("bad payload: {buf:?}"));
    }
    ok(MODE, "pipe round-trip");
}

/// Read a file and report success/failure (leaf-only ACL reachability).
fn read_file(path: &str) -> ! {
    const MODE: &str = "read-file";
    match std::fs::read_to_string(path) {
        Ok(s) => ok(MODE, format_args!("read {} bytes", s.len())),
        Err(e) => fail(
            MODE,
            format_args!("read {path}: {e} (os error {:?})", e.raw_os_error()),
        ),
    }
}
