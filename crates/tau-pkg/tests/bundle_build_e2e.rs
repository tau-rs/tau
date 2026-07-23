//! End-to-end test for the `tau build` producer.
//!
//! These tests exercise the full build pipeline against a realistic
//! on-disk fixture: a `tau.toml` with two packages (plugin + skill) and
//! two agents (one with an inline `system` prompt, one loading its
//! prompt from disk via `system_file`), a v6 lockfile pointing at
//! actual installed-package directories, and the corresponding
//! `.tau/packages/<name>/<version>/` trees on disk.
//!
//! The fixture intentionally omits an `[[agents.<id>.requires.tools]]`
//! block: the build pipeline does not consult it (transitive-tool
//! resolution happens at install time via the lockfile), so including
//! it would only add noise around `PackageSource` parsing. The v1 happy
//! path verified here covers the producer end-to-end without it.

use std::path::Path;

use tau_pkg::bundle::{build, BuildOptions};
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

[models]
fs-read-model = { backend = "fs-read", model = "model-v1" }
critic-model  = { backend = "critic",  model = "model-v1" }

[agents.researcher]
display_name = "Researcher"
package      = "fs-read@^0.1"
model        = "fs-read-model"

[agents.researcher.prompt]
system = "you are a researcher"

[agents.writer]
display_name = "Writer"
package      = "critic@^0.1"
model        = "critic-model"

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

/// Hex-encode a byte slice as lower-case (same shape the bundle
/// producer uses; replicated locally so the test does not depend on
/// `tau_pkg::tree_hash::to_hex_lower` being public).
fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}

#[test]
fn e2e_build_produces_parseable_bundle_with_correct_facts() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());

    let artifact = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: None,
        agent_filter: None,
        ir_payload: None,
        governance: None,
        assets: Vec::new(),
    })
    .expect("build");

    // Default output path: `<root>/<name>-<version>.tau`.
    assert_eq!(artifact.path, tmp.path().join("e2e-fixture-0.1.0.tau"));
    assert_eq!(artifact.sha256.len(), 64, "sha256 is 64 hex chars");
    assert!(artifact.size_bytes > 0, "bundle is non-empty");

    // Parse the bundle back and verify the self-hash.
    let bundle_str = std::fs::read_to_string(&artifact.path).unwrap();
    let manifest = tau_pkg::bundle::BundleManifest::parse_str(&bundle_str).expect("bundle parses");
    tau_pkg::bundle::hash::verify_self_hash(&manifest).expect("self-hash verifies");

    // Schema-level facts. schema_version = 2 since the schema_version bump.
    assert_eq!(manifest.schema_version, 2);
    assert_eq!(manifest.project.name, "e2e-fixture");
    assert_eq!(manifest.project.version.to_string(), "0.1.0");

    // Packages — sorted alphabetically (critic < fs-read).
    assert_eq!(manifest.packages.len(), 2, "two packages in the bundle");
    assert_eq!(manifest.packages[0].name, "critic");
    assert_eq!(manifest.packages[1].name, "fs-read");
    // Tree SHA-256s are real (non-zero) values computed from the
    // installed-package directories.
    assert_ne!(manifest.packages[0].tree_sha256, "0".repeat(64));
    assert_ne!(manifest.packages[1].tree_sha256, "0".repeat(64));
    // Critic and fs-read have different contents → different hashes.
    assert_ne!(
        manifest.packages[0].tree_sha256, manifest.packages[1].tree_sha256,
        "distinct package trees hash distinctly",
    );

    // Agents — sorted alphabetically (researcher < writer).
    assert_eq!(manifest.agents.len(), 2, "two agents in the bundle");
    assert_eq!(manifest.agents[0].id.as_str(), "researcher");
    assert_eq!(manifest.agents[1].id.as_str(), "writer");

    // Researcher's inline `system` prompt was hashed.
    let expected_researcher = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"you are a researcher");
        hex_lower(h.finalize().as_slice())
    };
    assert_eq!(
        manifest.agents[0].system_prompt_sha256, expected_researcher,
        "researcher prompt hash matches sha256(\"you are a researcher\")",
    );

    // Writer's `system_file` was loaded from disk and hashed.
    let expected_writer = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"you are a writer");
        hex_lower(h.finalize().as_slice())
    };
    assert_eq!(
        manifest.agents[1].system_prompt_sha256, expected_writer,
        "writer prompt hash matches sha256 of agents/writer.md bytes",
    );
}

#[test]
fn e2e_two_builds_produce_identical_sha256() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixture(tmp.path());

    let a = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: Some(tmp.path().join("a.tau")),
        agent_filter: None,
        ir_payload: None,
        governance: None,
        assets: Vec::new(),
    })
    .expect("first build");

    // Sleep long enough that `created_at` (RFC 3339 seconds resolution)
    // is guaranteed to differ between the two builds.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let b = build(BuildOptions {
        project_root: tmp.path().to_path_buf(),
        target: TargetTriple::host(),
        output_path: Some(tmp.path().join("b.tau")),
        agent_filter: None,
        ir_payload: None,
        governance: None,
        assets: Vec::new(),
    })
    .expect("second build");

    assert_eq!(
        a.sha256, b.sha256,
        "self-hash is reproducible across builds (created_at excluded)",
    );

    let bytes_a = std::fs::read(&a.path).unwrap();
    let bytes_b = std::fs::read(&b.path).unwrap();
    assert_ne!(
        bytes_a, bytes_b,
        "raw file bytes differ because the informational `created_at` differs",
    );
}
