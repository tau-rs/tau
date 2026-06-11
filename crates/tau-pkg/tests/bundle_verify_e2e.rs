//! End-to-end: build a bundle, then verify it against the same source
//! tree. Asserts the happy path + that post-build mutation is caught.
//!
//! `write_fixture` is copied verbatim from `bundle_build_e2e.rs` so the
//! producer and consumer agree on a realistic on-disk fixture: a
//! `tau.toml` with two packages (`fs-read` + `critic`) and two agents
//! (`researcher` with an inline prompt, `writer` loading its prompt
//! from disk), a v6 lockfile, and the corresponding
//! `.tau/packages/<name>/<version>/` trees.

use std::path::Path;

use tau_pkg::bundle::{build, verify_bundle, BuildOptions, VerifyError, VerifyOptions};
use tau_ports::target::TargetTriple;

fn write_fixture(root: &Path) {
    // Project `tau.toml`. Schema notes:
    // - `[project].version` is consumed by `extract_project_version` in
    //   build.rs (not by the validated `ProjectConfig`).
    // - `package` must match `<name>@<semver-req>` (see
    //   `parse_package_ref` in project/agent.rs).
    // - `[agents.<id>.prompt]` uses `system` xor `system_file`.
    std::fs::write(
        root.join("tau.toml"),
        r#"
[project]
name = "e2e-fixture"
version = "0.1.0"

[agents.researcher]
display_name = "Researcher"
package      = "fs-read@^0.1"
llm_backend  = "anthropic"

[agents.researcher.prompt]
system = "you are a researcher"

[agents.writer]
display_name = "Writer"
package      = "critic@^0.1"
llm_backend  = "anthropic"

[agents.writer.prompt]
system_file = "agents/writer.md"
"#,
    )
    .unwrap();

    // Agent prompt file referenced by `agents.writer.prompt.system_file`.
    std::fs::create_dir_all(root.join("agents")).unwrap();
    std::fs::write(root.join("agents/writer.md"), b"you are a writer").unwrap();

    // Installed package dirs — step 3 (`verify_installed`) requires
    // each locked package to exist at `.tau/packages/<name>/<version>/`.
    // The directory contents feed `tree_hash` in step 4, so we drop in
    // a few small files to make the tree hashes non-trivial.
    let pkg_fs_read = root.join(".tau/packages/fs-read/0.1.0");
    std::fs::create_dir_all(pkg_fs_read.join("src")).unwrap();
    std::fs::write(
        pkg_fs_read.join("Cargo.toml"),
        "[package]\nname = \"fs-read\"\n",
    )
    .unwrap();
    std::fs::write(pkg_fs_read.join("src/lib.rs"), "// fs-read\n").unwrap();

    let pkg_critic = root.join(".tau/packages/critic/0.1.0");
    std::fs::create_dir_all(pkg_critic.join("docs")).unwrap();
    std::fs::write(
        pkg_critic.join("SKILL.md"),
        "---\nname: critic\n---\n# Critic\n",
    )
    .unwrap();
    std::fs::write(pkg_critic.join("docs/notes.md"), "notes").unwrap();

    // Lockfile (schema v6 shape per T5 / T7).
    std::fs::write(
        root.join("tau.lock"),
        r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "fs-read"
active_version = "0.1.0"
source = "https://example.com/fs-read.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "critic"
active_version = "0.1.0"
source = "https://example.com/critic.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000002"
installed_at = "2024-01-01T00:00:00Z"
"#,
    )
    .unwrap();
}

#[test]
fn e2e_build_then_verify_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
        agent_filter: None,
        ir_payload: None,
    })
    .unwrap();

    let report = verify_bundle(VerifyOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
        recomputed_ir_hash: None,
    })
    .expect("verify succeeds on freshly-built bundle");

    // The fixture defines two agents: `researcher` (inline prompt) and
    // `writer` (file-based prompt). Both should resolve.
    assert_eq!(report.agent_lookup.len(), 2);
    assert!(report.agent_lookup.contains_key("researcher"));
    assert!(report.agent_lookup.contains_key("writer"));
}

#[test]
fn e2e_verify_catches_post_build_package_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());
    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
        agent_filter: None,
        ir_payload: None,
    })
    .unwrap();

    // Mutate an installed package file. `src/lib.rs` is part of the
    // `fs-read` package tree hashed at build time, so rewriting it after
    // the build must make verify detect a package-tree drift.
    let f = tmp.path().join(".tau/packages/fs-read/0.1.0/src/lib.rs");
    std::fs::write(&f, "// mutated after build\n").unwrap();

    let err = verify_bundle(VerifyOptions {
        bundle_path: artifact.path,
        project_root: tmp.path().to_path_buf(),
        recomputed_ir_hash: None,
    })
    .unwrap_err();
    assert!(
        matches!(err, VerifyError::PackageDrift { .. }),
        "got {err:?}"
    );
}
