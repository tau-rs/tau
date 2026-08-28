//! In-container test probe for tau-sandbox-windows integration tests.
//!
//! Pure std. Modes (argv[1]):
//! - `http-get <url>` — proxy-form (absolute-URI) GET through the proxy
//!   named by the HTTP_PROXY env var, exactly as `tau-net-bridge-win`
//!   sets it for the wrapped plugin. Prints the response's status line;
//!   exit 0 iff the response is 200.
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
    // stderr canary. Result markers go to STDOUT, so a run where the
    // stdout markers arrive but this line does not proves the launcher
    // -> bridge -> probe chain loses stderr — which would also mean the
    // bridge's own `BRIDGE ` diagnostics never reach CI. Distinguishing
    // "the bridge printed nothing" from "nothing the bridge prints can
    // be seen" is otherwise a whole CI round.
    eprintln!("PROBE stderr-alive");
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

/// Open `\\.\pipe\<name>` read+write; exit 0 on success, 1 on error.
fn pipe_open(name: &str) -> ! {
    let path = format!(r"\\.\pipe\{name}");
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
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

/// GET `url` (plain http) through the HTTP_PROXY proxy, absolute-URI
/// form — exactly the request shape tau-sandbox-proxy's handle_http
/// expects (and what `tau-net-bridge-win` gives the wrapped plugin via
/// HTTP_PROXY).
fn http_get(url: &str) -> ! {
    let proxy = std::env::var("HTTP_PROXY").unwrap_or_default();
    let Some(hostport) = proxy.strip_prefix("http://") else {
        println!("PROBE result=err detail=no-http-proxy-env");
        std::process::exit(1);
    };
    eprintln!("PROBE proxy={proxy}");
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
    conn.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let req = format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if let Err(e) = conn.write_all(req.as_bytes()) {
        println!("PROBE result=err detail=write: {e}");
        std::process::exit(1);
    }
    // Read incrementally rather than `read_to_end` so the *reason* the
    // read ended is reportable. `read_to_end` returns `Err` on the
    // socket's read timeout and drops whatever the failing iteration had
    // buffered, which made "the proxy answered nothing" and "the proxy
    // answered but never closed" produce the identical, useless
    // `PROBE result=status detail=` (empty) seen in #622 CI round 2.
    let mut resp = Vec::new();
    let mut chunk = [0u8; 4096];
    let read_end;
    loop {
        match conn.read(&mut chunk) {
            Ok(0) => {
                read_end = "eof".to_string();
                break;
            }
            Ok(n) => resp.extend_from_slice(&chunk[..n]),
            Err(e) => {
                read_end = format!("err:{e}");
                break;
            }
        }
    }
    let head = String::from_utf8_lossy(&resp);
    let status = head.lines().next().unwrap_or("");
    // Diagnostics on stderr; the stdout marker below stays byte-stable
    // because the integration tests match it literally.
    eprintln!(
        "PROBE http bytes={} read-end={read_end} raw={:?}",
        resp.len(),
        head.chars().take(200).collect::<String>()
    );
    println!("PROBE result=status detail={status}");
    if status.contains(" 200 ") {
        std::process::exit(0);
    }
    std::process::exit(1);
}

/// Read `path` to a string; exit 0 on success, 1 on error.
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
