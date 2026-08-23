//! Integration tests: `ProjectConfig::from_path` merges `[dirs]`-scanned
//! agent/tool definitions with the inline `[agents.*]`/`[tools.*]` tables
//! (Task 4, `parse_str_at`).

use tau_pkg::project::{ProjectConfig, ProjectConfigError};

// `packages = ["mock-llm"]` is required so `[models].default`'s backend
// resolves against a declared package (ADR-0057 stage-1 model validation);
// the directory-scanned agent's own `package = "p@^1"` only declares `p`.
const ROOT_TOML: &str = r#"
packages = ["mock-llm"]
[project]
name = "p"
[dirs]
agents = "agents"
tools  = "tools"
[models]
default = { backend = "mock-llm", model = "m" }
"#;
const AGENT_MD: &str = "---\ndisplay_name: A\npackage: p@^1\nmodel: default\n---\nbody\n";

fn write(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

#[test]
fn from_path_merges_dir_definitions() {
    let t = tempfile::TempDir::new().unwrap();
    write(t.path(), "tau.toml", ROOT_TOML);
    write(t.path(), "agents/review/strict.md", AGENT_MD);
    write(
        t.path(),
        "tools/github/search.toml",
        "native = \"ReadTemp\"\n",
    );
    let cfg = ProjectConfig::from_path(t.path().join("tau.toml")).unwrap();
    let agent = &cfg.agents["review/strict"];
    assert!(matches!(
        &agent.prompt,
        tau_pkg::project::PromptEntry::Inline(s) if s == "body\n"
    ));
    assert!(cfg.tools.contains_key("github/search"));
    assert!(cfg.dirs.is_some());
}

#[test]
fn inline_collision_is_hard_error() {
    let t = tempfile::TempDir::new().unwrap();
    let toml = format!(
        "{ROOT_TOML}\n[agents.\"review/strict\"]\ndisplay_name = \"B\"\npackage = \"p@^1\"\n"
    );
    write(t.path(), "tau.toml", &toml);
    write(t.path(), "agents/review/strict.md", AGENT_MD);
    std::fs::create_dir_all(t.path().join("tools")).unwrap();
    let err = ProjectConfig::from_path(t.path().join("tau.toml")).unwrap_err();
    assert!(
        matches!(err, ProjectConfigError::DuplicateDefinition { .. }),
        "{err:?}"
    );
}

#[test]
fn project_without_dirs_is_unaffected() {
    let t = tempfile::TempDir::new().unwrap();
    write(t.path(), "tau.toml", "[project]\nname = \"p\"\n");
    assert!(ProjectConfig::from_path(t.path().join("tau.toml")).is_ok());
}

/// `from_path` delegates to `parse_str_at`, which has no path to report and
/// so always raises a TOML syntax error as `ParseStr`. `from_path` must
/// remap that back to the file-based `Parse { path, source }` variant so
/// callers relying solely on the error's own `Display` (no added
/// `.with_context`, e.g. `tau resolve` / `tau list`) still get the file
/// path, and so `tau check`'s `--json`/SARIF `"kind": "Parse"` contract
/// doesn't silently become `"ParseStr"`.
#[test]
fn from_path_malformed_toml_reports_parse_with_path() {
    let t = tempfile::TempDir::new().unwrap();
    let tau_toml = t.path().join("tau.toml");
    write(t.path(), "tau.toml", "[project\nname = \"p\"\n");
    let err = ProjectConfig::from_path(&tau_toml).unwrap_err();
    assert!(
        matches!(&err, ProjectConfigError::Parse { path, .. } if path == &tau_toml),
        "{err:?}"
    );
    let display = err.to_string();
    assert!(
        display.contains(&tau_toml.display().to_string()),
        "{display}"
    );
}
