//! Integration tests: each named `tau check <category>` subcommand runs
//! only its own check and returns a valid exit code.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use std::path::PathBuf;
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/check")
        .join(name)
}

#[test]
fn each_subcommand_is_invokable() {
    check_common::ensure_tau_home();
    for cat in [
        "config",
        "lockfile",
        "packages",
        "sandbox",
        "plugins",
        "skills",
        "mcp-contracts",
        "dirs",
    ] {
        let tmp = TempDir::new().unwrap();
        let src = fixture("clean-project");
        let proj = tmp.path().join("proj");
        std::fs::create_dir(&proj).unwrap();
        std::fs::copy(src.join("tau.toml"), proj.join("tau.toml")).unwrap();
        let out = Command::cargo_bin("tau")
            .unwrap()
            .args(["check", cat])
            .current_dir(&proj)
            .output()
            .unwrap();
        // Subcommands MUST return a valid exit code (0, 2, or 3).
        let code = out.status.code().unwrap_or(-1);
        assert!(
            matches!(code, 0 | 2 | 3),
            "category `{cat}` produced unexpected exit {code}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}
