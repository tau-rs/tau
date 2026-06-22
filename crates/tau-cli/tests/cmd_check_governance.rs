//! Integration: `tau check governance` fails (exit 2) on an over-reaching tool.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn over_reaching_tool_fails_governance_with_exit_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-06-19T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.tools.fetch]
native = "Fetch"

[tools.fetch]
native = "Fetch"
capabilities = [{ kind = "fs.read", paths = ["/etc/**"] }]
"#,
    )
    .unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .arg("check")
        .arg("governance")
        .current_dir(root)
        .assert()
        .code(2);
}
