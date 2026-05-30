//! macOS integration tests for `DarwinSandbox::wrap_spawn`.
//!
//! Skip on non-macOS at compile time so the same source compiles on Linux
//! CI without producing failing tests. Requires the `integration-tests`
//! cargo feature.

#![cfg(all(target_os = "macos", feature = "integration-tests"))]

use std::process::{Command, Stdio};

use serde_json::json;

use tau_ports::{CapabilityGate, ProcessCapabilityGate, CapabilityPlan, CapabilityProbe};
use tau_sandbox_darwin::DarwinSandbox;

fn make_plan(value: serde_json::Value) -> CapabilityPlan {
    let plan_json = json!({
        "capabilities": value,
        "context": null,
        "limits": null,
    });
    serde_json::from_value(plan_json).expect("decode plan")
}

#[tokio::test]
async fn probe_returns_available_on_macos() {
    let s = DarwinSandbox::new("darwin");
    let probe = s.probe().await;
    assert!(
        matches!(probe, CapabilityProbe::Available { .. }),
        "expected Available, got {probe:?}"
    );
}

#[tokio::test]
async fn echo_runs_under_strict_profile() {
    let s = DarwinSandbox::new("darwin");
    // Empty plan = baseline-only (no plan-derived FS reads, no network).
    // /bin/echo only needs the baseline's libc/dyld bootstrap.
    let plan = make_plan(json!([]));

    let mut cmd = Command::new("/bin/echo");
    cmd.arg("hello-from-strict-darwin")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let _handle = s.wrap_spawn(&plan, &mut cmd).await.expect("wrap_spawn");

    // Now spawn the wrapped command and read stdout.
    let output = cmd.output().expect("spawn");
    assert!(
        output.status.success(),
        "exit status: {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "hello-from-strict-darwin");
}

#[tokio::test]
async fn echo_with_fs_read_capability() {
    let s = DarwinSandbox::new("darwin");
    let plan = make_plan(json!([
        { "kind": "fs.read", "paths": ["/etc/hosts"] }
    ]));

    let mut cmd = Command::new("/bin/echo");
    cmd.arg("ok").stdout(Stdio::piped()).stderr(Stdio::piped());
    let _handle = s.wrap_spawn(&plan, &mut cmd).await.expect("wrap_spawn");
    let output = cmd.output().expect("spawn");
    assert!(output.status.success());
}

#[tokio::test]
async fn http_plan_spawns_proxy_task() {
    // Plan with Network(Http) — spawn_proxy is invoked inside wrap_spawn,
    // and the resulting handle nests the proxy guard. Drop the handle and
    // verify the proxy socket file is unlinked.
    let s = DarwinSandbox::new("darwin");
    let plan = make_plan(json!([
        { "kind": "net.http", "hosts": ["api.example.com"], "methods": ["GET"] }
    ]));
    let mut cmd = Command::new("/bin/echo");
    cmd.arg("ok").stdout(Stdio::piped()).stderr(Stdio::piped());
    let handle = s.wrap_spawn(&plan, &mut cmd).await.expect("wrap_spawn");

    // Verify HTTPS_PROXY ended up in the wrapped command's env.
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    let has_proxy = envs
        .iter()
        .any(|(k, v)| k == "HTTPS_PROXY" && v.as_deref() == Some("http://127.0.0.1:8443"));
    assert!(has_proxy, "expected HTTPS_PROXY in env: {envs:?}");

    drop(handle);
    // After drop, the proxy socket file should be unlinked. We can't check
    // it without re-exposing the path; rely on the proxy crate's own test
    // (`proxy_handle_drop_unlinks_socket_file`) for that invariant.
}
