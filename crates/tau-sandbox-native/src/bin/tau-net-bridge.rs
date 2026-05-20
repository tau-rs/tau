//! tau-net-bridge — proxy bridge running inside the strict-tier child's
//! empty network namespace (sub-project H, ADR-0020).
//!
//! Workflow:
//!   1. Bring `lo` up (rtnetlink); empty netns starts with lo DOWN
//!   2. Bind a TCP listener on 127.0.0.1:8443
//!   3. fork(2): parent of fork = bridge runtime; child of fork = plugin
//!   4. Bridge: for each accepted TCP conn, dial the proxy Unix socket;
//!      splice bytes both ways
//!   5. When the plugin exits, the bridge exits with the same status

#![cfg_attr(not(target_os = "linux"), allow(unused))]

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let args: Vec<String> = std::env::args().collect();
    let parsed = parse_args(&args)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("args: {e}")))?;

    bring_lo_up()?;

    let bind_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, parsed.listen_port);
    let listener = TcpListener::bind(bind_addr)?;
    listener.set_nonblocking(false)?;

    // SAFETY: fork is the canonical Unix way to split the bridge from the
    // plugin process; both inherit fds and the netns/userns from us.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid == 0 {
        // Child of fork: replace this process with the plugin
        let mut cmd = Command::new(&parsed.plugin_argv[0]);
        cmd.args(&parsed.plugin_argv[1..]);
        // env was set by the parent (tau) before spawn — we inherit it.
        return Err(cmd.exec());
    }

    // Parent of fork: run the bridge loop until the plugin exits
    let proxy_sock = parsed.proxy_sock_path.clone();
    std::thread::spawn(move || run_bridge_loop(listener, &proxy_sock));

    // Wait for the plugin to exit; propagate its exit code
    let mut status: libc::c_int = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    if waited < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if libc::WIFEXITED(status) {
        std::process::exit(libc::WEXITSTATUS(status));
    } else if libc::WIFSIGNALED(status) {
        std::process::exit(128 + libc::WTERMSIG(status));
    } else {
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("tau-net-bridge runs only on Linux");
    std::process::exit(1);
}

// Args and parse_args are pure logic — no Linux-specific types — so they
// compile on all platforms, enabling the unit tests to run on macOS too.
// Debug is required so `parse_args(...).unwrap_err()` can format the Ok
// arm in test panics.
#[derive(Debug)]
struct Args {
    proxy_sock_path: std::path::PathBuf,
    listen_port: u16,
    plugin_argv: Vec<String>,
}

fn parse_args(argv: &[String]) -> Result<Args, &'static str> {
    let mut proxy_sock = None;
    let mut listen_addr = None;
    let mut plugin_argv = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        if let Some(v) = arg.strip_prefix("--proxy-sock=") {
            proxy_sock = Some(std::path::PathBuf::from(v));
        } else if let Some(v) = arg.strip_prefix("--listen=") {
            listen_addr = Some(v.to_string());
        } else if arg == "--" {
            plugin_argv.extend(argv[i + 1..].iter().cloned());
            break;
        } else {
            return Err("unexpected arg");
        }
        i += 1;
    }
    let proxy_sock_path = proxy_sock.ok_or("missing --proxy-sock=")?;
    let listen = listen_addr.ok_or("missing --listen=")?;
    let port: u16 = listen
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .ok_or("invalid --listen= addr")?;
    if plugin_argv.is_empty() {
        return Err("missing -- <plugin> <args>");
    }
    Ok(Args {
        proxy_sock_path,
        listen_port: port,
        plugin_argv,
    })
}

#[cfg(target_os = "linux")]
fn bring_lo_up() -> std::io::Result<()> {
    use futures::TryStreamExt;
    use rtnetlink::{new_connection, LinkUnspec};
    use tokio::runtime::Builder;

    // Best-effort: empty netns from Native adapter starts with lo down and we
    // have CAP_NET_ADMIN-in-userns so set-up succeeds. Inside Docker containers
    // (Container adapter), lo is already up but we don't have CAP_NET_ADMIN
    // and the set-up call fails with EPERM. Either way, the listener bind on
    // 127.0.0.1:8443 below is the load-bearing check: if lo isn't actually
    // usable, the bind fails loudly. Don't fail the bridge if we can't bring
    // lo up — log a warning and let the bind try.
    let rt = Builder::new_current_thread().enable_all().build()?;
    let attempt: std::io::Result<()> = rt.block_on(async {
        let (connection, handle, _) = new_connection().map_err(std::io::Error::other)?;
        tokio::spawn(connection);
        let mut links = handle.link().get().match_name("lo".to_string()).execute();
        let link = links
            .try_next()
            .await
            .map_err(std::io::Error::other)?
            .ok_or_else(|| std::io::Error::other("lo not found"))?;
        handle
            .link()
            .set(LinkUnspec::new_with_index(link.header.index).up().build())
            .execute()
            .await
            .map_err(std::io::Error::other)?;
        Ok(())
    });
    if let Err(e) = attempt {
        eprintln!("bridge: bring lo up failed (continuing — lo may already be up): {e}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_bridge_loop(listener: std::net::TcpListener, proxy_sock: &std::path::Path) {
    use std::os::unix::net::UnixStream;
    for tcp_conn in listener.incoming() {
        let Ok(tcp_conn) = tcp_conn else { continue };
        let proxy_conn = match UnixStream::connect(proxy_sock) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("bridge: proxy connect failed: {e}");
                continue;
            }
        };
        std::thread::spawn(move || splice_bidirectional(tcp_conn, proxy_conn));
    }
}

#[cfg(target_os = "linux")]
fn splice_bidirectional(tcp: std::net::TcpStream, unix: std::os::unix::net::UnixStream) {
    let tcp_clone = match tcp.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let unix_clone = match unix.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let h1 = std::thread::spawn(move || {
        let mut a = &tcp;
        let mut b = &unix;
        let _ = std::io::copy(&mut a, &mut b);
    });
    let h2 = std::thread::spawn(move || {
        let mut a = &unix_clone;
        let mut b = &tcp_clone;
        let _ = std::io::copy(&mut a, &mut b);
    });
    let _ = h1.join();
    let _ = h2.join();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_well_formed() {
        let argv: Vec<String> = vec![
            "tau-net-bridge".into(),
            "--proxy-sock=/tmp/x.sock".into(),
            "--listen=127.0.0.1:8443".into(),
            "--".into(),
            "/usr/bin/plugin".into(),
            "arg1".into(),
        ];
        let parsed = parse_args(&argv).expect("parse");
        assert_eq!(
            parsed.proxy_sock_path,
            std::path::PathBuf::from("/tmp/x.sock")
        );
        assert_eq!(parsed.listen_port, 8443);
        assert_eq!(parsed.plugin_argv, vec!["/usr/bin/plugin", "arg1"]);
    }

    #[test]
    fn parse_args_missing_proxy_sock() {
        let argv: Vec<String> = vec![
            "tau-net-bridge".into(),
            "--listen=127.0.0.1:8443".into(),
            "--".into(),
            "/usr/bin/plugin".into(),
        ];
        let err = parse_args(&argv).unwrap_err();
        assert_eq!(
            err, "missing --proxy-sock=",
            "expected the missing-proxy-sock error; got: {err:?}"
        );
    }

    #[test]
    fn parse_args_missing_separator() {
        let argv: Vec<String> = vec![
            "tau-net-bridge".into(),
            "--proxy-sock=/tmp/x.sock".into(),
            "--listen=127.0.0.1:8443".into(),
        ];
        let err = parse_args(&argv).unwrap_err();
        assert_eq!(
            err, "missing -- <plugin> <args>",
            "expected the missing-separator error; got: {err:?}"
        );
    }

    #[test]
    fn parse_args_invalid_listen() {
        let argv: Vec<String> = vec![
            "tau-net-bridge".into(),
            "--proxy-sock=/tmp/x.sock".into(),
            "--listen=not-an-addr".into(),
            "--".into(),
            "/usr/bin/plugin".into(),
        ];
        let err = parse_args(&argv).unwrap_err();
        assert_eq!(
            err, "invalid --listen= addr",
            "expected the invalid-listen-addr error; got: {err:?}"
        );
    }

    #[test]
    fn parse_args_unexpected_arg() {
        let argv: Vec<String> = vec![
            "tau-net-bridge".into(),
            "--proxy-sock=/tmp/x.sock".into(),
            "--gobbledygook".into(),
            "--listen=127.0.0.1:8443".into(),
            "--".into(),
            "/usr/bin/plugin".into(),
        ];
        let err = parse_args(&argv).unwrap_err();
        assert_eq!(
            err, "unexpected arg",
            "expected the unexpected-arg error; got: {err:?}"
        );
    }
}
