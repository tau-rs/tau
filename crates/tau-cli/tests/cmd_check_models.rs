//! Integration: `tau check plugins` emits `BackendNotLlmCapable` when a
//! `[models]` backend resolves to an installed package that does NOT expose
//! LLM completion (D7 stage 2).

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

/// A project whose model alias points at an installed *tool* package (not an
/// llm_backend) must be flagged. The package validates at build time (it IS a
/// declared package), but `tau check` catches the capability gap.
#[test]
fn models_backend_that_is_not_llm_capable_is_flagged() {
    check_common::ensure_tau_home();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Mark `root` as a project scope so `Scope::resolve` binds the lockfile
    // there (a bare cwd would fall back to the global scope).
    std::fs::create_dir_all(root.join(".tau")).unwrap();
    std::fs::write(
        root.join(".tau").join("config.toml"),
        r#"schema_version = 3
kind = "project"
created_at = "2026-06-19T00:00:00Z"
created_by_tau_version = "0.0.0"

[sandbox]
required_tier = "none"
"#,
    )
    .unwrap();

    // A real (empty) binary file so `--fast` existence check passes cleanly.
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let bin_path = bin_dir.join("datatool");
    std::fs::write(&bin_path, b"").unwrap();

    // Project: alias `default` → backend `datatool` (a tool-only package).
    std::fs::write(
        root.join("tau.toml"),
        r#"packages = ["datatool"]

[project]
name = "demo"

[models]
default = { backend = "datatool", model = "whatever-1" }

[agents.solo]
display_name = "Solo"
package      = "demo@^0.1"
model        = "default"
"#,
    )
    .unwrap();

    // Lockfile: `datatool` is installed and its plugin `provides = "tool"`.
    // Normalize path separators to `/` so a Windows path like
    // `C:\Users\RUNNER~1\...` doesn't produce invalid TOML: `\U` in a basic
    // string is a unicode escape, which would make the lockfile unreadable
    // and surface `tau.plugins.lockfile_unreadable` instead of the finding
    // this test asserts. Windows APIs accept forward slashes. (Same idiom as
    // the sibling pipeline tests.)
    let bin_str = bin_path
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    std::fs::write(
        root.join("tau-lock.toml"),
        format!(
            r#"schema_version = 4
generated_by_tau_version = "0.0.0"
generated_at = "2026-06-19T00:00:00Z"

[[package]]
name = "datatool"
active_version = "0.1.0"
source = "https://example.com/datatool.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "{zero}"
sha256 = ""
installed_at = "2026-06-19T00:00:00Z"

[package.plugin]
binary_path = "{bin_str}"
built_at = "2026-06-19T00:00:00Z"

[package.plugin.manifest]
provides = "tool"
kind = "rust-cargo"
bin = "datatool"
"#,
            zero = "0".repeat(40),
        ),
    )
    .unwrap();

    let out = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "plugins", "--fast", "--json"])
        .current_dir(root)
        .env("TAU_HOME", root.join(".tau-global"))
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let found = stdout.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| {
                v.get("findings").and_then(|f| f.as_array()).map(|arr| {
                    arr.iter().any(|f| {
                        f.get("rule_id").and_then(|r| r.as_str())
                            == Some("tau.models.backend_not_llm_capable")
                    })
                })
            })
            .unwrap_or(false)
    });

    assert!(
        found,
        "expected a tau.models.backend_not_llm_capable finding\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "BackendNotLlmCapable is Severity::Error → exit 2\nstdout:\n{stdout}"
    );
}
