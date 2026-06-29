//! Integration: governance lattice L1/L2 with an installed package manifest.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

/// Write project scope + lockfile + an installed package manifest with the
/// given capabilities, plus the project tau.toml.
fn setup(root: &std::path::Path, pkg_caps: &str, project_toml: &str) {
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-06-19T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tau-lock.toml"),
        format!(
            "schema_version = 4\ngenerated_by_tau_version = \"0.0.0\"\ngenerated_at = \"2026-06-19T00:00:00Z\"\n\n[[package]]\nname = \"demo\"\nactive_version = \"0.1.0\"\nsource = \"https://example.com/demo.git\"\n\n[[package.versions]]\nversion = \"0.1.0\"\nresolved_commit = \"{zero}\"\nsha256 = \"\"\ninstalled_at = \"2026-06-19T00:00:00Z\"\n",
            zero = "0".repeat(40)
        ),
    )
    .unwrap();
    let inst = root
        .join(".tau")
        .join("packages")
        .join("demo")
        .join("0.1.0");
    std::fs::create_dir_all(&inst).unwrap();
    std::fs::write(
        inst.join("tau.toml"),
        format!(
            "name = \"demo\"\nversion = \"0.1.0\"\ndescription = \"d\"\nauthors = []\nsource = \"https://example.com/demo.git\"\nkind = \"tool\"\ndependencies = []\ncapabilities = {pkg_caps}\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join("tau.toml"), project_toml).unwrap();
}

#[test]
fn package_exceeding_root_fails_with_exit_2() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    setup(
        root,
        r#"[{ kind = "fs.read", paths = ["/etc/**"] }]"#, // package requests /etc/**
        r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
"#,
    );
    Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance"])
        .current_dir(root)
        .assert()
        .code(2);
}

#[test]
fn uninstalled_package_is_needs_setup_exit_3() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // project scope + tau.toml, but NO lockfile / installed manifest
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        "schema_version = 3\nkind = \"project\"\ncreated_at = \"2026-06-19T00:00:00Z\"\ncreated_by_tau_version = \"0.0.0\"\n\n[sandbox]\nrequired_tier = \"none\"\n",
    ).unwrap();
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "demo"

[allow]
"fs.read" = { paths = ["/proj/**"] }

[allow.models.fast]
backend = "demo"
model = "m-1"

[agents.solo]
display_name = "Solo"
package = "demo@^0.1"
model = "fast"
"#,
    )
    .unwrap();
    Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "governance"])
        .current_dir(root)
        .assert()
        .code(3); // NeedsSetup, not a false Error
}
