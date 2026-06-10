//! Integration: `:quit` and Ctrl-D (EOF on stdin) both exit cleanly (exit 0).

use assert_fs::prelude::*;

/// Write a minimal tau.toml with one agent into `tmp`.
fn write_minimal_toml(tmp: &assert_fs::TempDir) {
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name    = "quit-test"
version = "0.0.1"

[agents.a]
display_name  = "A"
package       = "a@^0.1"
llm_backend   = "mock-llm"
model         = "mock-1"
prompt.system = "x"
"#,
        )
        .unwrap();
}

#[test]
fn ctrl_d_exits_zero() {
    let tmp = assert_fs::TempDir::new().unwrap();
    write_minimal_toml(&tmp);

    // write_stdin("") sends empty bytes then closes the pipe, simulating
    // an immediate EOF / Ctrl-D.  rustyline sees EOF on the first readline
    // call and the REPL loop breaks cleanly.
    assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["dev", "."])
        .write_stdin("")
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .success();
}

#[test]
fn quit_command_exits_zero() {
    let tmp = assert_fs::TempDir::new().unwrap();
    write_minimal_toml(&tmp);

    assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["dev", "."])
        .write_stdin(":quit\n")
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .success();
}
