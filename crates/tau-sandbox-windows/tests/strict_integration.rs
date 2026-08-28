//! Windows-only enforcement proof for the AppContainer adapter.
#![cfg(all(target_os = "windows", feature = "integration-tests"))]

use serde_json::json;
use std::process::Command;
use tau_ports::{CapabilityPlan, ProcessCapabilityGate};
use tau_sandbox_windows::WindowsSandbox;

fn plan(caps: serde_json::Value) -> CapabilityPlan {
    serde_json::from_value(json!({ "capabilities": caps, "context": null, "limits": null }))
        .unwrap()
}

fn with_launcher(mut c: Command) -> Command {
    c.env(
        "TAU_APPCONTAINER_LAUNCHER_PATH",
        env!("CARGO_BIN_EXE_tau-appcontainer-launcher"),
    );
    c
}

/// A denied path is unreadable from inside the AppContainer (empty plan).
#[test]
fn empty_plan_denies_arbitrary_read() {
    // Write a secret to a temp file NOT granted to the container.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, b"topsecret").unwrap();

    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    cmd.args(["/C", &format!("type \"{}\"", secret.display())]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _handle = rt
        .block_on(sandbox.wrap_spawn(&plan(json!([])), &mut cmd))
        .expect("wrap");
    let out = cmd.output().expect("spawn");
    // AppContainer has no ACL grant on the temp dir → read denied.
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("topsecret"),
        "secret leaked: {:?}",
        out.stdout
    );
}

/// Granting read on one path must NOT leak access to an un-granted sibling.
///
/// Asserts the *scoping* of the sandbox: with a read grant present on one
/// directory, a file in a different, un-granted directory is still denied.
/// Positive control: the `echo` markers prove the child actually ran, so a
/// denied/empty read cannot pass vacuously.
///
/// NOTE: this does NOT assert positive readability of the *granted* path;
/// `egress_integration::leaf_only_grant_readable_at_nested_path` does
/// (spike #626 H3 refuted ADR-0067's FILE_TRAVERSE premise — AppContainer
/// tokens keep `SeChangeNotifyPrivilege`, so a leaf-only grant is already
/// reachable at a nested path and no ancestor grants are needed). The
/// security-relevant property here — deny-by-default isolation — is proven
/// by `empty_plan_denies_arbitrary_read` and by this test's sibling denial.
#[test]
fn grant_does_not_leak_ungranted_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let granted = dir.path().join("granted");
    std::fs::create_dir_all(&granted).unwrap();
    let sibling = dir.path().join("secret.txt");
    std::fs::write(&sibling, b"topsecret").unwrap();

    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    cmd.args([
        "/C",
        &format!("echo START & type \"{}\" & echo END", sibling.display()),
    ]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let plan = plan(json!([{ "kind": "fs.read", "paths": [granted.to_string_lossy()] }]));
    let _handle = rt
        .block_on(sandbox.wrap_spawn(&plan, &mut cmd))
        .expect("wrap");
    let out = cmd.output().expect("spawn");
    let s = String::from_utf8_lossy(&out.stdout);

    assert!(
        s.contains("START") && s.contains("END"),
        "child did not run to completion: {s:?}"
    );
    assert!(!s.contains("topsecret"), "un-granted sibling leaked: {s:?}");
}

/// HTTP plans are ACCEPTED since #622 and routed through the pipe bridge.
///
/// This test used to assert the opposite (`must refuse http`) — that was
/// ADR-0067's network-fail-closed phase, which #622 supersedes:
/// `supported_shapes()` now carries `NetworkHttp` and the adapter
/// enforces egress with a per-container SID-ACL'd named pipe instead of
/// refusing the plan. Kept (not deleted) and re-pointed at the new
/// contract, so the plan-shape decision stays covered: an HTTP plan must
/// wrap successfully AND the rebuilt command must actually go through
/// `launcher -- <bridge> --pipe <name> -- <orig program>`. A regression
/// that dropped the bridge from the rebuild would leave the plugin with
/// `HTTP_PROXY` unset and no egress at all, which no other unit test
/// would catch.
#[test]
fn http_plan_is_accepted_and_routed_through_bridge() {
    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    let p = plan(json!([{ "kind": "net.http", "hosts": ["example.com"], "methods": ["GET"] }]));
    let rt = tokio::runtime::Runtime::new().unwrap();
    // `_handle` owns the pipe proxy + the AppContainer profile; holding
    // it to the end of the test keeps cleanup (ACL revoke, profile
    // delete, accept-loop abort) on the normal drop path.
    let _handle = rt
        .block_on(sandbox.wrap_spawn(&p, &mut cmd))
        .expect("http plans are supported since #622");

    let program = cmd.get_program().to_string_lossy().into_owned();
    assert!(
        program.contains("tau-appcontainer-launcher"),
        "rebuilt program must be the launcher, got {program}"
    );
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args.first().map(String::as_str),
        Some("--profile"),
        "{args:?}"
    );
    let sep = args
        .iter()
        .position(|a| a == "--")
        .expect("launcher payload separator");
    assert!(
        args[sep + 1].contains("tau-net-bridge-win"),
        "payload must start with the in-container bridge: {args:?}"
    );
    assert_eq!(
        args.get(sep + 2).map(String::as_str),
        Some("--pipe"),
        "{args:?}"
    );
    assert!(
        args[sep + 3].starts_with("tau-proxy-"),
        "bridge must be handed this spawn's pipe name: {args:?}"
    );
    assert_eq!(
        args.get(sep + 4).map(String::as_str),
        Some("--"),
        "{args:?}"
    );
    assert_eq!(
        args.get(sep + 5).map(String::as_str),
        Some("cmd"),
        "{args:?}"
    );
}
