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
