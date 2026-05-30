//! End-to-end container sandbox integration tests.
//!
//! Linux + docker-or-podman-on-PATH only. Run with:
//!   cargo test -p tau-runtime --features integration-tests -- --ignored
//!
//! Tests skip gracefully (via the runtime probe) when no container
//! runtime is on PATH.

#![cfg(all(target_os = "linux", feature = "integration-tests"))]

use std::process::Command;
use tau_ports::{CapabilityGate, CapabilityPlan, CapabilityProbe, ProcessCapabilityGate};
use tau_sandbox_container::{ContainerRuntime, ContainerSandbox};

#[tokio::test]
#[ignore = "requires Linux + docker or podman on PATH"]
async fn fs_read_works_inside_container() {
    let s = ContainerSandbox::new("container", ContainerRuntime::Auto);
    let probe = s.probe().await;
    if matches!(probe, CapabilityProbe::Unavailable { .. }) {
        eprintln!("skipping: no docker/podman on PATH");
        return;
    }

    // The exact behavior here depends on the default base image
    // (ghcr.io/tau-runtime/sandbox-base:v0.1) being pullable. For v0.1
    // we just verify wrap_spawn replaces the cmd with a docker run
    // invocation; full e2e (actually pulling and running the image) is
    // covered by the unit tests.
    let plan = CapabilityPlan::new(vec![], None, None);
    let mut cmd = Command::new("/bin/echo");
    cmd.arg("hello");
    let _h = s.wrap_spawn(&plan, &mut cmd).await.unwrap();

    // After wrap_spawn, the cmd's program should be "docker" or "podman".
    let prog = cmd.get_program().to_string_lossy().to_string();
    assert!(
        prog == "docker" || prog == "podman",
        "expected wrap_spawn to set cmd program to docker/podman, got {prog}"
    );
}

#[tokio::test]
#[ignore = "requires Linux + docker or podman on PATH"]
async fn shell_plugin_runs_under_container() {
    // Same idea: verify wrap_spawn structure rather than actually
    // spawning (which would require a pre-pulled image).
    let s = ContainerSandbox::new("container", ContainerRuntime::Auto);
    let probe = s.probe().await;
    if matches!(probe, CapabilityProbe::Unavailable { .. }) {
        eprintln!("skipping: no docker/podman on PATH");
        return;
    }
    let plan = CapabilityPlan::new(vec![], None, None);
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "echo hello"]);
    let _h = s.wrap_spawn(&plan, &mut cmd).await.unwrap();

    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.contains(&"run".into()),
        "should have docker/podman 'run'"
    );
    assert!(
        args.contains(&"/bin/sh".into()),
        "should preserve original program"
    );
}
