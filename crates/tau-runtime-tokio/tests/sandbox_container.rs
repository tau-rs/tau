//! End-to-end container sandbox integration tests.
//!
//! Linux + docker-or-podman-on-PATH only. Run with:
//!   cargo nextest run -p tau-runtime-tokio --features integration-tests \
//!     --run-ignored only
//!
//! What these add over `tau_sandbox_container`'s own unit tests: those call
//! `build_run_args` directly with a hard-coded `ResolvedRuntime`, so they
//! never touch the probe. These drive the real path — probe a runtime
//! binary that is actually on PATH, then `wrap_spawn` through the resolved
//! runtime — and assert the rewritten `Command`.
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

    // Actually pulling and running a per-plugin image is out of scope here
    // (that is what tau-plugin-compat's layer4_container suite does, with
    // `xtask build-plugin-images` as a prerequisite). This asserts the step
    // before it: probe resolves a runtime, and wrap_spawn rewrites the
    // Command to invoke that runtime.
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
    // The original program path is deliberately NOT passed through for a
    // non-HTTP plan: the per-plugin image's own ENTRYPOINT *is* the plugin
    // binary, so the program only survives as the image tag (resolved from
    // its basename) and the caller's args are appended verbatim after it.
    // See `tau_sandbox_container::runner::wrap_command`.
    assert!(
        args.contains(&"tau-plugin-sh:dev".into()),
        "image tag should be derived from the original program's basename, got {args:?}"
    );
    let image_pos = args
        .iter()
        .position(|a| a == "tau-plugin-sh:dev")
        .expect("image present");
    assert_eq!(
        &args[image_pos + 1..],
        ["-c", "echo hello"],
        "caller args should be appended after the image, got {args:?}"
    );
}
