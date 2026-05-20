//! Snapshot coverage for `tau verify --anthropic-strict` error shapes.
//!
//! `cmd_verify.rs` covers exit codes + substring assertions for the
//! three [`AnthropicConformanceIssue`] variants. This file pins down
//! the actual rendered text via `insta::assert_snapshot!` so refactors
//! that drift the operator-facing wording need explicit `cargo insta`
//! acceptance.
//!
//! Per Logging Sub-project Test-suite plan T12b.

use assert_cmd::Command;
use std::path::Path;

/// Build a minimal skill lockfile (schema_version = 6) for a single
/// skill package. Mirrors `cmd_verify::skill_lockfile_toml` —
/// duplicated rather than re-exported to keep the test file
/// self-contained.
fn skill_lockfile_toml(name: &str, version: &str) -> String {
    format!(
        "schema_version = 6\n\
         generated_by_tau_version = \"0.0.0\"\n\
         generated_at = \"2026-05-12T10:00:00Z\"\n\n\
         [[package]]\n\
         name = \"{name}\"\n\
         active_version = \"{version}\"\n\
         source = \"https://example.com/{name}.git\"\n\
         \n\
         [package.skill]\n\
         content_sha256 = \"\"\n\
         [package.skill.frontmatter]\n\
         name = \"{name}\"\n\
         description = \"Test skill.\"\n\
         \n\
         [[package.versions]]\n\
         version = \"{version}\"\n\
         resolved_commit = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n\
         sha256 = \"\"\n\
         installed_at = \"2026-05-12T10:00:00Z\"\n"
    )
}

fn write_skill_package(pkg_dir: &Path, name: &str, version: &str, skill_md: &str) {
    std::fs::create_dir_all(pkg_dir).unwrap();
    let tau_toml = format!(
        "name = \"{name}\"\nversion = \"{version}\"\ndescription = \"Test skill.\"\n\
         authors = []\nsource = \"https://example.com/{name}.git\"\nkind = \"skill\"\n\
         dependencies = []\ncapabilities = []\n\n[skill]\n"
    );
    std::fs::write(pkg_dir.join("tau.toml"), tau_toml).unwrap();
    std::fs::write(pkg_dir.join("SKILL.md"), skill_md).unwrap();
}

/// Run `tau verify --global --anthropic-strict` against a tempdir
/// seeded with a single skill package. Returns combined stdout.
fn run_verify_strict(global_path: &Path) -> String {
    let output = Command::cargo_bin("tau")
        .unwrap()
        .args(["verify", "--global", "--anthropic-strict"])
        .env("TAU_HOME", global_path)
        .output()
        .expect("tau verify ran");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Apply redactions for content that varies between runs:
/// - the temporary install dir prefix (`/var/folders/...` on macOS,
///   `/tmp/...` on Linux, `C:\Users\…\Temp\…` on Windows).
fn redacted(stdout: &str) -> String {
    // Strip any path that looks like an install tree under tempdir.
    let mut out = stdout.to_string();
    for needle in ["/private/var/folders", "/var/folders", "/tmp", "C:\\Users"] {
        while let Some(start) = out.find(needle) {
            // Erase up to (but not including) the next whitespace or
            // newline; that's the path span.
            let rest = &out[start..];
            let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
            out.replace_range(start..start + end, "<TEMPDIR>");
        }
    }
    out
}

#[test]
fn snapshot_missing_description() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("bad-skill", "1.0.0"),
    )
    .unwrap();
    let pkg_dir = global_path.join("packages/bad-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "bad-skill",
        "1.0.0",
        "---\nname: bad-skill\ndescription: \"\"\n---\nDo the thing.\n",
    );
    let stdout = redacted(&run_verify_strict(global_path));
    insta::assert_snapshot!(stdout);
}

#[test]
fn snapshot_empty_body() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("empty-body-skill", "1.0.0"),
    )
    .unwrap();
    let pkg_dir = global_path.join("packages/empty-body-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "empty-body-skill",
        "1.0.0",
        "---\nname: empty-body-skill\ndescription: A skill with no body.\n---\n   \n\n",
    );
    let stdout = redacted(&run_verify_strict(global_path));
    insta::assert_snapshot!(stdout);
}

#[test]
fn snapshot_malformed_frontmatter() {
    let global_dir = tempfile::tempdir().unwrap();
    let global_path = global_dir.path();
    std::fs::write(
        global_path.join("tau-lock.toml"),
        skill_lockfile_toml("broken-skill", "1.0.0"),
    )
    .unwrap();
    let pkg_dir = global_path.join("packages/broken-skill/1.0.0");
    write_skill_package(
        &pkg_dir,
        "broken-skill",
        "1.0.0",
        "---\nname: broken-skill\ndescription: missing closing fence\nDo the thing.\n",
    );
    let stdout = redacted(&run_verify_strict(global_path));
    insta::assert_snapshot!(stdout);
}
