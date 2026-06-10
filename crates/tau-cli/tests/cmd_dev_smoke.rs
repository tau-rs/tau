//! Smoke test: `tau dev --help` is dispatchable and lists the 4 flags.

use assert_cmd::Command;

#[test]
fn dev_help_lists_four_flags() {
    let output = Command::cargo_bin("tau")
        .expect("binary")
        .args(["dev", "--help"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--prompt", "--agent", "--watch", "--no-color"] {
        assert!(stdout.contains(flag), "expected `{flag}` in: {stdout}");
    }
}
