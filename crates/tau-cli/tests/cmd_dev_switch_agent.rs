//! Integration: `:agent <name>` switches the active agent.
//!
//! The test sends `:agents` (lists both agents), then `:agent second`
//! (switch), then `:agents` again (confirm marker moved), then `:quit`.
//! The assertion checks that the switch-confirmation message was printed.

use assert_fs::prelude::*;

#[test]
fn switch_agent_via_repl_command() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("tau.toml")
        .write_str(
            r#"
[project]
name    = "switch-test"
version = "0.0.1"

[agents.first]
display_name  = "First"
package       = "first@^0.1"
llm_backend   = "mock-llm"
model         = "mock-1"
prompt.system = "first"

[agents.second]
display_name  = "Second"
package       = "second@^0.1"
llm_backend   = "mock-llm"
model         = "mock-1"
prompt.system = "second"
"#,
        )
        .unwrap();

    let output = assert_cmd::Command::cargo_bin("tau")
        .unwrap()
        .current_dir(tmp.path())
        .args(["dev", "."])
        .write_stdin(":agents\n:agent second\n:agents\n:quit\n")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    // `repl::switch_agent` prints exactly: `switched to `{name}``
    assert!(
        stdout.contains("switched to") && stdout.contains("second"),
        "expected agent-switch confirmation in output, got:\n{stdout}"
    );
}
