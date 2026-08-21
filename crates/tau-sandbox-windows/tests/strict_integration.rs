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
/// NOTE: this does NOT assert positive readability of the *granted* path.
/// Making an arbitrary nested granted path reachable requires FILE_TRAVERSE
/// grants on every ancestor directory (AppContainers get no access via the
/// usual Everyone/Users ACEs). That "functional positive grant" work is
/// deferred to the Windows sandbox network-egress follow-on, where real
/// `cargo` builds first need to read granted paths. The security-relevant
/// property — deny-by-default isolation — is proven by
/// `empty_plan_denies_arbitrary_read` and by this test's sibling denial.
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

/// HTTP plans fail closed.
#[test]
fn http_plan_is_refused() {
    let sandbox = WindowsSandbox::new("native");
    let mut cmd = with_launcher(Command::new("cmd"));
    let p = plan(json!([{ "kind": "net.http", "hosts": ["example.com"], "methods": ["GET"] }]));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let err = rt
        .block_on(sandbox.wrap_spawn(&p, &mut cmd))
        .expect_err("must refuse http");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("egress") || msg.contains("ShapeUnsupported"),
        "got {msg}"
    );
}
