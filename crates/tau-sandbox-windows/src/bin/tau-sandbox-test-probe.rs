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
