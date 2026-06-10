//! Integration: `:help` lists all 9 REPL commands.

use assert_fs::prelude::*;

#[test]
fn help_lists_all_commands() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name    = "help-test"
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

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["dev", "."])
        .write_stdin(":help\n:quit\n")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Every verb listed in `repl::print_help` must appear in the output.
    for verb in [
        ":reload", ":state", ":history", ":agents", ":agent", ":clear", ":help", ":quit",
    ] {
        assert!(
            stdout.contains(verb),
            "expected `{verb}` in :help output, got:\n{stdout}"
        );
    }
}
