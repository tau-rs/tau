//! Build the `docker run` / `podman run` argv from a [`CapabilityPlan`].
//!
//! The public-facing entry point is [`wrap_command`], which rewrites a
//! [`std::process::Command`] in-place. [`build_run_args`] is extracted as a
//! pure function so it can be unit-tested without spawning anything.

use std::path::PathBuf;
use std::process::Command;

use tau_domain::{Capability, FsCapability, NetCapability};
use tau_ports::{CapabilityError, CapabilityHandle, CapabilityPlan};

use crate::probe::ResolvedRuntime;

/// Proxy configuration for Network(Http) plans.
///
/// Holds the Unix socket path the proxy is listening on. The bridge
/// binary lives in the per-plugin image (baked via `tau-plugin-base`),
/// so no host-side bridge path is plumbed through any more.
#[derive(Debug)]
pub(crate) struct ProxyConfig {
    /// Absolute path to the proxy Unix socket in the parent's temp dir.
    pub(crate) sock_path: PathBuf,
}

/// Replace `cmd` with a `docker run` (or `podman run`) invocation that
/// runs the plugin's per-plugin image. The image is resolved by
/// convention from the original `cmd`'s program path: the basename
/// (e.g. `shell-plugin`) becomes `tau-plugin-shell-plugin:dev`.
///
/// For plans with [`tau_domain::NetCapability::Http`], spawns a userspace
/// proxy task and bind-mounts its Unix socket into the container. The
/// `tau-net-bridge` binary is **not** bind-mounted — it lives inside
/// `tau-plugin-base` (the runtime stage of every plugin image). For HTTP
/// plans, the adapter overrides the image's `ENTRYPOINT` to
/// `/usr/local/bin/tau-net-bridge` and passes the bridge args + plugin
/// path positionally.
///
/// ## Caller config
///
/// - **stdio**: set to `Stdio::piped()` for stdin, stdout, and stderr after
///   `wrap_command` returns. The original caller's stdio configuration is
///   dropped (stable `std::process::Command` does not expose stdio inspection).
///   If different stdio is needed, reconfigure via the returned `Command`.
///
/// - **env vars**: env vars whose names match `TAU_*` or `RUST_LOG` are
///   captured from the original `cmd` and forwarded into the container via
///   `-e KEY=VALUE` flags. All other env vars are dropped — the container
///   image provides its own environment.
///
/// - **cwd**: dropped — has no meaning inside a container with `--read-only`
///   root and `--tmpfs /tmp`.
pub(crate) fn wrap_command(
    plan: &CapabilityPlan,
    cmd: &mut Command,
    runtime: ResolvedRuntime,
) -> Result<CapabilityHandle, CapabilityError> {
    let original_program = cmd.get_program().to_string_lossy().into_owned();
    let original_args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    // Resolve the per-plugin image name from the program's basename.
    let bin_name = std::path::Path::new(&original_program)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CapabilityError::WrapFailed {
            message: format!("cannot derive plugin bin name from program path: {original_program}"),
        })?;
    let image = format!("tau-plugin-{bin_name}:dev");

    // Capture env vars matching TAU_* or RUST_LOG before we replace cmd.
    let forwarded_envs: Vec<(String, String)> = cmd
        .get_envs()
        .filter_map(|(k, v)| {
            let key = k.to_string_lossy().to_string();
            let val = v?.to_string_lossy().to_string();
            if key.starts_with("TAU_") || key == "RUST_LOG" {
                Some((key, val))
            } else {
                None
            }
        })
        .collect();

    // Determine if the plan requests outbound HTTP.
    let has_network_http = plan
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Network(NetCapability::Http { .. })));

    // For Network(Http): spawn the userspace proxy, collect proxy config.
    // Proxy support is unix-only (relies on Unix-domain sockets + the
    // tau-net-bridge Linux binary). On non-unix targets, Network(Http) plans
    // are hard-refused — Windows/etc. container support is a future iteration.
    #[cfg(unix)]
    let (proxy_handle, proxy_config) = if has_network_http {
        let mut any = false;
        let mut exact: Vec<String> = Vec::new();
        for cap in &plan.capabilities {
            if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
                if hosts.is_any() {
                    any = true;
                } else {
                    exact.extend(hosts.exact_hosts());
                }
            }
        }
        let policy = if any {
            tau_sandbox_proxy::HostAllow::Any
        } else {
            tau_sandbox_proxy::HostAllow::Exact(exact.clone())
        };
        tau_sandbox_proxy::validate_hosts(&exact).map_err(|e| CapabilityError::Proxy {
            message: format!("host validation: {e}"),
        })?;
        let handle =
            tau_sandbox_proxy::spawn_proxy(policy).map_err(|e| CapabilityError::Proxy {
                message: format!("spawn_proxy: {e}"),
            })?;
        let sock_path = handle.sock_path().to_path_buf();
        let config = ProxyConfig { sock_path };
        (Some(handle), Some(config))
    } else {
        (None, None)
    };

    #[cfg(not(unix))]
    let (proxy_handle, proxy_config): (Option<()>, Option<ProxyConfig>) = if has_network_http {
        return Err(CapabilityError::Proxy {
            message: "Network(Http) capability is unix-only in this iteration; \
                      Windows container proxy is a future sub-project"
                .to_string(),
        });
    } else {
        (None, None)
    };

    let argv = build_run_args(
        plan,
        runtime,
        &image,
        &original_program,
        &original_args,
        &forwarded_envs,
        proxy_config.as_ref(),
    );

    // Replace cmd in-place: clear via re-assignment, then build new argv.
    *cmd = Command::new(runtime.binary());
    for arg in &argv {
        cmd.arg(arg);
    }
    // Set stdio to piped so the container's stdin/stdout/stderr are accessible
    // to the caller (required for plugin host JSON-RPC IPC in Task 9).
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Nest the proxy guard inside the CapabilityHandle so it is dropped (LIFO)
    // when the handle is dropped.
    let mut handle = CapabilityHandle::noop();
    if let Some(p) = proxy_handle {
        handle.nest_handle(Box::new(p));
    }

    Ok(handle)
}

/// Build the argv slice (without the `docker`/`podman` binary itself) that
/// runs `program` inside the per-plugin `image`.
///
/// `forwarded_envs` is a list of `(KEY, VALUE)` pairs that will be injected
/// into the container via `-e KEY=VALUE` flags. Typically these are env vars
/// matching `TAU_*` or `RUST_LOG` captured from the original `Command`.
///
/// `proxy` is `Some` when the plan has `Network(Http)` capabilities. When
/// present, the proxy Unix socket is bind-mounted into the container at
/// `/run/tau-proxy.sock`, `HTTPS_PROXY=http://127.0.0.1:8443` is injected,
/// and the image's `ENTRYPOINT` is overridden to
/// `/usr/local/bin/tau-net-bridge` (baked into `tau-plugin-base`). The
/// bridge then exec's the plugin (whose path is `/usr/local/bin/<bin>`,
/// derived from the basename of the original `Command`'s program).
///
/// Exposed for unit tests so argv shape can be verified without spawning.
pub(crate) fn build_run_args(
    plan: &CapabilityPlan,
    runtime: ResolvedRuntime,
    image: &str,
    program: &str,
    program_args: &[String],
    forwarded_envs: &[(String, String)],
    proxy: Option<&ProxyConfig>,
) -> Vec<String> {
    // Both docker and podman share the same `run` argument shape.
    let _ = runtime;

    // For HTTP plans the bridge needs to dial a Unix socket bind-mounted
    // from the host. Docker Desktop on macOS ignores the host-side mode
    // bits and presents bind-mounts as root:root 660 inside the container,
    // so a non-root user inside the container cannot open the socket.
    // Linux Docker preserves host UIDs but the host UID won't generally
    // match `nobody` (65534) either. Run the container as root for HTTP
    // plans; cap-drop=ALL + no-new-privileges + read-only + seccomp
    // default keeps the sandbox properties roughly equivalent to nobody.
    let has_http = plan
        .capabilities
        .iter()
        .any(|c| matches!(c, Capability::Network(NetCapability::Http { .. })));
    let user = if has_http { "0" } else { "nobody" };

    let mut argv: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "--user".into(),
        user.into(),
        "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(),
        "--read-only".into(),
        "--pids-limit".into(),
        "256".into(),
        "--ipc=none".into(),
        "--tmpfs".into(),
        "/tmp:size=64m".into(),
    ];

    // Network: bridge only when the plan requests HTTP; otherwise none.
    argv.push("--network".into());
    if has_http {
        argv.push("bridge".into());
    } else {
        argv.push("none".into());
    }

    // Volume mounts derived from filesystem capabilities.
    for cap in &plan.capabilities {
        match cap {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                for p in paths {
                    let cleaned = clean_mount_path(p);
                    argv.push("-v".into());
                    argv.push(format!("{cleaned}:{cleaned}:ro"));
                }
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                for p in paths {
                    let cleaned = clean_mount_path(p);
                    argv.push("-v".into());
                    argv.push(format!("{cleaned}:{cleaned}:rw"));
                }
            }
            // ProcessExec and Network are handled via --cap-drop / --network;
            // no extra mount args needed.
            _ => {}
        }
    }

    // Proxy bind-mount + entrypoint override for Network(Http) plans.
    if let Some(proxy_cfg) = proxy {
        let sock = proxy_cfg.sock_path.display().to_string();
        argv.push("-v".into());
        argv.push(format!("{sock}:/run/tau-proxy.sock:rw"));
        argv.push("-e".into());
        argv.push("HTTPS_PROXY=http://127.0.0.1:8443".into());
        argv.push("-e".into());
        argv.push("HTTP_PROXY=http://127.0.0.1:8443".into());
        argv.push("-e".into());
        argv.push("https_proxy=http://127.0.0.1:8443".into());
        argv.push("-e".into());
        argv.push("http_proxy=http://127.0.0.1:8443".into());
        argv.push("--entrypoint=/usr/local/bin/tau-net-bridge".into());
    }

    // Forward whitelisted env vars into the container.
    for (k, v) in forwarded_envs {
        argv.push("-e".into());
        argv.push(format!("{k}={v}"));
    }

    // Image comes after all docker-run flags.
    argv.push(image.into());

    // After the image, what gets passed depends on whether we wrapped with
    // the bridge:
    //
    // - HTTP plans (proxy.is_some()): we overrode ENTRYPOINT to
    //   tau-net-bridge, so the post-image argv is bridge args + the in-
    //   image plugin path. The plugin path is derived from `program`'s
    //   basename — see `wrap_command` for the same derivation used to
    //   choose the image tag.
    //
    // - non-HTTP plans: the image's own ENTRYPOINT is the plugin binary;
    //   we just pass through `program_args` (caller-supplied flags).
    if proxy.is_some() {
        let bin_name = std::path::Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(program);
        argv.push("--proxy-sock=/run/tau-proxy.sock".into());
        argv.push("--listen=127.0.0.1:8443".into());
        argv.push("--".into());
        argv.push(format!("/usr/local/bin/{bin_name}"));
        for a in program_args {
            argv.push(a.clone());
        }
    } else {
        for a in program_args {
            argv.push(a.clone());
        }
    }

    argv
}

/// Strip trailing glob suffixes from a path so it can be used as a bind-mount
/// source/target. For example `/srv/data/**` becomes `/srv/data`.
fn clean_mount_path(p: &str) -> String {
    p.trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan_from(capabilities: serde_json::Value) -> CapabilityPlan {
        let plan_json = json!({
            "capabilities": capabilities,
            "context": null,
            "limits": null,
        });
        serde_json::from_value(plan_json).expect("decode plan")
    }

    #[test]
    fn read_path_yields_ro_mount() {
        let plan = plan_from(json!([{
            "kind": "fs.read",
            "paths": ["/etc/foo"]
        }]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "/etc/foo:/etc/foo:ro"),
            "expected ro mount, got {argv:?}"
        );
    }

    #[test]
    fn write_path_yields_rw_mount() {
        let plan = plan_from(json!([{
            "kind": "fs.write",
            "paths": ["/data/cache"]
        }]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "/data/cache:/data/cache:rw"),
            "expected rw mount, got {argv:?}"
        );
    }

    #[test]
    fn http_capability_enables_bridge_network() {
        let plan = plan_from(json!([{
            "kind": "net.http",
            "hosts": ["api.example.com"],
            "methods": ["GET"]
        }]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        let pos = argv
            .iter()
            .position(|a| a == "--network")
            .expect("--network present");
        assert_eq!(argv[pos + 1], "bridge");
    }

    #[test]
    fn no_network_capability_uses_none() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        let pos = argv
            .iter()
            .position(|a| a == "--network")
            .expect("--network present");
        assert_eq!(argv[pos + 1], "none");
    }

    #[test]
    fn read_only_root_and_tmpfs_present() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "--read-only"),
            "missing --read-only"
        );
        assert!(argv.iter().any(|a| a == "--tmpfs"), "missing --tmpfs");
        assert!(
            argv.iter().any(|a| a == "/tmp:size=64m"),
            "missing /tmp:size=64m"
        );
    }

    #[test]
    fn cap_drop_and_no_new_privs_present() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "--cap-drop=ALL"),
            "missing --cap-drop=ALL"
        );
        assert!(
            argv.iter().any(|a| a == "--security-opt=no-new-privileges"),
            "missing --security-opt=no-new-privileges"
        );
    }

    #[test]
    fn hardening_flags_present() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        let user_pos = argv
            .iter()
            .position(|a| a == "--user")
            .expect("--user present");
        assert_eq!(argv[user_pos + 1], "nobody", "--user must be nobody");
        let pids_pos = argv
            .iter()
            .position(|a| a == "--pids-limit")
            .expect("--pids-limit present");
        assert_eq!(argv[pids_pos + 1], "256", "--pids-limit must be 256");
        assert!(argv.iter().any(|a| a == "--ipc=none"), "missing --ipc=none");
    }

    #[test]
    fn original_program_and_args_at_end() {
        // In the new design, non-HTTP plans use the image's baked ENTRYPOINT
        // (the plugin binary); `program` is not appended to argv. Only
        // `program_args` appear after the image.
        let plan = plan_from(json!([]));
        let args: Vec<String> = vec!["hello".into(), "world".into()];
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &args,
            &[],
            None,
        );
        // Image is the third-to-last element, followed by the two program args.
        assert_eq!(argv[argv.len() - 3], "tau-plugin-test:dev");
        assert_eq!(argv[argv.len() - 2], "hello");
        assert_eq!(argv[argv.len() - 1], "world");
    }

    #[test]
    fn glob_suffix_trimmed_in_mounts() {
        let plan = plan_from(json!([{
            "kind": "fs.read",
            "paths": ["/srv/data/**"]
        }]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "/srv/data:/srv/data:ro"),
            "expected globless mount, got {argv:?}"
        );
    }

    #[test]
    fn podman_runtime_produces_same_argv_shape() {
        // Both runtimes should produce identical argv (the binary is set
        // outside build_run_args).
        let plan = plan_from(json!([]));
        let docker_argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/true",
            &[],
            &[],
            None,
        );
        let podman_argv = build_run_args(
            &plan,
            ResolvedRuntime::Podman,
            "tau-plugin-test:dev",
            "/bin/true",
            &[],
            &[],
            None,
        );
        assert_eq!(docker_argv, podman_argv);
    }

    #[test]
    fn multiple_read_paths_yield_multiple_mounts() {
        let plan = plan_from(json!([{
            "kind": "fs.read",
            "paths": ["/a", "/b", "/c"]
        }]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(argv.iter().any(|a| a == "/a:/a:ro"));
        assert!(argv.iter().any(|a| a == "/b:/b:ro"));
        assert!(argv.iter().any(|a| a == "/c:/c:ro"));
    }

    #[test]
    fn image_is_present_before_program() {
        // In the new design, `program` is not appended to argv for non-HTTP
        // plans (baked ENTRYPOINT handles it). Verify the image is present and
        // is the last element when no program_args are passed.
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            argv.iter().any(|a| a == "tau-plugin-test:dev"),
            "image must be present in argv: {argv:?}"
        );
        assert_eq!(
            argv.last().unwrap(),
            "tau-plugin-test:dev",
            "image must be last when no program_args: {argv:?}"
        );
    }

    #[test]
    fn rm_and_stdin_flags_present() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(argv.iter().any(|a| a == "--rm"), "missing --rm");
        assert!(argv.iter().any(|a| a == "-i"), "missing -i");
    }

    #[test]
    fn tau_env_vars_forwarded() {
        let plan = plan_from(json!([]));
        let envs = vec![("TAU_FOO".to_string(), "bar".to_string())];
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &envs,
            None,
        );
        let pos = argv
            .iter()
            .position(|a| a == "-e")
            .expect("-e flag present for TAU_FOO");
        assert_eq!(argv[pos + 1], "TAU_FOO=bar", "env must be TAU_FOO=bar");
    }

    #[test]
    fn unrelated_env_vars_dropped() {
        let plan = plan_from(json!([]));
        // LANG is not TAU_* and not RUST_LOG — it must not be forwarded.
        // build_run_args only injects what is passed in forwarded_envs.
        // wrap_command filters; here we verify the contract: if we pass no
        // forwarded envs, no -e flags appear.
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            !argv.iter().any(|a| a.starts_with("LANG=")),
            "LANG must not appear in argv: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "-e"),
            "no -e flag expected when forwarded_envs is empty: {argv:?}"
        );
    }

    #[test]
    fn rust_log_env_var_forwarded() {
        let plan = plan_from(json!([]));
        let envs = vec![("RUST_LOG".to_string(), "trace".to_string())];
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &envs,
            None,
        );
        let pos = argv
            .iter()
            .position(|a| a == "-e")
            .expect("-e flag present for RUST_LOG");
        assert_eq!(argv[pos + 1], "RUST_LOG=trace");
    }

    #[test]
    fn proxy_args_added_when_network_http_present() {
        let plan = plan_from(json!([{
            "kind": "net.http",
            "hosts": ["api.example.com"],
            "methods": ["GET"]
        }]));
        let proxy_cfg = ProxyConfig {
            sock_path: std::path::PathBuf::from("/tmp/tau-proxy-12345-0.sock"),
        };
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            Some(&proxy_cfg),
        );
        // HTTPS_PROXY env must be present.
        assert!(
            argv.iter()
                .any(|a| a == "HTTPS_PROXY=http://127.0.0.1:8443"),
            "expected HTTPS_PROXY in argv: {argv:?}"
        );
        // HTTP_PROXY (uppercase) must also be present.
        assert!(
            argv.iter().any(|a| a == "HTTP_PROXY=http://127.0.0.1:8443"),
            "expected HTTP_PROXY in argv: {argv:?}"
        );
        // http_proxy (lowercase) must be present for reqwest on UNIX.
        assert!(
            argv.iter().any(|a| a == "http_proxy=http://127.0.0.1:8443"),
            "expected http_proxy in argv: {argv:?}"
        );
        // Proxy socket bind-mount must be present.
        assert!(
            argv.iter().any(|a| a.contains("/run/tau-proxy.sock")),
            "expected proxy socket mount in argv: {argv:?}"
        );
        // Entrypoint override must be present (no bridge bind-mount anymore).
        assert!(
            argv.iter()
                .any(|a| a == "--entrypoint=/usr/local/bin/tau-net-bridge"),
            "expected --entrypoint override in argv: {argv:?}"
        );
        // bridge args must appear after the image.
        let proxy_sock_arg_pos = argv
            .iter()
            .position(|a| a == "--proxy-sock=/run/tau-proxy.sock")
            .expect("--proxy-sock arg in argv");
        let image_pos = argv
            .iter()
            .position(|a| a == "tau-plugin-test:dev")
            .expect("image in argv");
        assert!(
            proxy_sock_arg_pos > image_pos,
            "bridge args must follow image: {argv:?}"
        );
    }

    #[test]
    fn no_proxy_args_when_no_network_http() {
        let plan = plan_from(json!([]));
        let argv = build_run_args(
            &plan,
            ResolvedRuntime::Docker,
            "tau-plugin-test:dev",
            "/bin/echo",
            &[],
            &[],
            None,
        );
        assert!(
            !argv.iter().any(|a| a.contains("HTTPS_PROXY=")),
            "HTTPS_PROXY must not appear without network http: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("HTTP_PROXY=")),
            "HTTP_PROXY must not appear without network http: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("http_proxy=")),
            "http_proxy must not appear without network http: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("tau-proxy.sock")),
            "proxy socket must not appear without network http: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("tau-net-bridge")),
            "tau-net-bridge must not appear without network http: {argv:?}"
        );
    }
}
