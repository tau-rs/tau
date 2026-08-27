//! Gates that keep the cargo-fuzz harnesses from becoming silent no-ops.
//!
//! Two independent invariants, each corresponding to a way the fuzzing setup
//! has already failed in this repo while looking healthy:
//!
//!   1. every fuzz `[[bin]]` is listed in BOTH workflow matrices, and
//!   2. every fuzz manifest is its own workspace root (or it will not build).
//!
//! Fuzz harnesses live in standalone (non-workspace) Cargo projects under
//! `crates/<crate>/fuzz/`, and the jobs that actually RUN them enumerate their
//! targets by hand in two places:
//!
//!   * `.github/workflows/fuzz-nightly.yml` — the nightly `fuzz` job
//!   * `.github/workflows/release.yml`      — the release-gating `fuzz` job
//!
//! Because the harness project is outside the workspace, nothing about writing
//! a new one forces either matrix to be updated — so a harness can be merged,
//! look complete, and be fuzzed by *nothing*. That is not hypothetical: both
//! `tau-domain` targets (`parse_skill_md`, #194; `parse_package_manifest`,
//! #201) sat in neither matrix from merge until the change that added this
//! test. Silent no-ops are the worst failure mode for a security control.
//!
//! Coarse, forward-only invariant, deliberately mirroring
//! `xtask/tests/architecture_md.rs`: **every `[[bin]]` declared in a
//! `crates/*/fuzz/Cargo.toml` must appear as a `target:` entry in BOTH
//! workflows.** The matrix keys on the `[[bin]]` name, not the filename, so
//! that is what we read.
//!
//! Deliberately not enforced: the reverse direction (a stale `target:` lingering
//! after a harness is deleted — that fails loudly at fuzz time), the
//! `crate:`/`fuzz_dir:` fields of each entry (a wrong pairing also fails loudly
//! at fuzz time), and any structural equality between the two matrices beyond
//! target coverage.

use std::path::{Path, PathBuf};

/// The two workflows whose `matrix.include` must name every fuzz target.
const MATRIX_WORKFLOWS: [&str; 2] = [
    ".github/workflows/fuzz-nightly.yml",
    ".github/workflows/release.yml",
];

fn repo_root() -> PathBuf {
    // xtask/ sits at the workspace root, so its parent is the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent (the repo root)")
        .to_path_buf()
}

/// Every `[[bin]]` name declared in a cargo-fuzz manifest.
///
/// Plain-text scan rather than a TOML dependency: xtask is intentionally
/// dependency-light (anyhow + clap), and cargo-fuzz manifests are generated
/// boilerplate with a fixed shape. Only `name =` keys that follow a `[[bin]]`
/// header count, so the `[package] name` never leaks in.
fn fuzz_bin_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if !in_bin {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().trim_start_matches('=').trim();
            if let Some(name) = value.strip_prefix('"').and_then(|v| v.split('"').next()) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Whether a workflow declares `target: <name>` as a matrix entry.
///
/// Fuzz `[[bin]]` names are snake_case Rust identifiers, so this cannot collide
/// with release.yml's other `target:` keys (Rust target triples, which are
/// hyphenated).
///
/// The leading `- ` is stripped because YAML writes it on whichever key happens
/// to come first in the list entry; without this, reordering `target:` to the
/// top of an entry would silently defeat the gate.
fn declares_target(workflow: &str, name: &str) -> bool {
    workflow.lines().any(|line| {
        let line = line.trim().trim_start_matches("- ").trim_start();
        line.strip_prefix("target:")
            .is_some_and(|v| v.trim() == name)
    })
}

#[test]
fn every_fuzz_target_is_in_both_matrices() {
    let root = repo_root();

    let workflows: Vec<(&str, String)> = MATRIX_WORKFLOWS
        .iter()
        .map(|rel| {
            let body = std::fs::read_to_string(root.join(rel))
                .unwrap_or_else(|e| panic!("{rel} must exist and be readable: {e}"));
            (*rel, body)
        })
        .collect();

    let mut discovered = 0usize;
    let mut missing = Vec::new();

    for entry in std::fs::read_dir(root.join("crates")).expect("crates/ dir is readable") {
        let manifest_path = entry
            .expect("readable dir entry")
            .path()
            .join("fuzz/Cargo.toml");
        let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
            continue; // crate has no fuzz project
        };
        let fuzz_dir = manifest_path
            .parent()
            .expect("fuzz/Cargo.toml has a parent")
            .strip_prefix(&root)
            .expect("fuzz dir is under the repo root")
            .to_string_lossy()
            .replace('\\', "/");

        for name in fuzz_bin_names(&manifest) {
            discovered += 1;
            for (rel, body) in &workflows {
                if !declares_target(body, &name) {
                    missing.push(format!("{name} (from {fuzz_dir}) is absent from {rel}"));
                }
            }
        }
    }

    assert!(
        discovered > 0,
        "found no `[[bin]]` targets under crates/*/fuzz/ — the discovery scan is broken, \
         not the repo (this test would otherwise pass vacuously forever)"
    );

    missing.sort();
    assert!(
        missing.is_empty(),
        "fuzz matrix is stale — these harnesses exist on disk but are fuzzed by nothing:\n  {}\n\n\
         Add an entry to `matrix.include` in EACH workflow listed above:\n\
         \x20 - crate: <crate>\n\
         \x20   fuzz_dir: crates/<crate>/fuzz\n\
         \x20   target: <bin name>\n\n\
         The matrix keys on the `[[bin]]` name in the fuzz Cargo.toml, not the \
         fuzz_targets/ filename.",
        missing.join("\n  ")
    );
}

/// Every fuzz manifest must mark itself as its own workspace root.
///
/// The root `Cargo.toml` `exclude` entry is NOT sufficient on its own for a
/// package nested under a member crate's directory: without an empty
/// `[workspace]` table, cargo attaches the fuzz project to the outer workspace
/// and every `cargo fuzz` invocation dies with "current package believes it's
/// in a workspace when it's not". All three harnesses shipped without the
/// table, so `Fuzz nightly` failed at BUILD (~40s per run) every night for
/// months while looking like a configured, functioning control.
#[test]
fn every_fuzz_manifest_is_its_own_workspace_root() {
    let root = repo_root();
    let mut unmarked = Vec::new();
    let mut discovered = 0usize;

    for entry in std::fs::read_dir(root.join("crates")).expect("crates/ dir is readable") {
        let manifest_path = entry
            .expect("readable dir entry")
            .path()
            .join("fuzz/Cargo.toml");
        let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
            continue; // crate has no fuzz project
        };
        discovered += 1;
        if !manifest.lines().any(|l| l.trim() == "[workspace]") {
            unmarked.push(
                manifest_path
                    .strip_prefix(&root)
                    .expect("fuzz manifest is under the repo root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }

    assert!(
        discovered > 0,
        "found no crates/*/fuzz/Cargo.toml — the discovery scan is broken"
    );

    unmarked.sort();
    assert!(
        unmarked.is_empty(),
        "these fuzz manifests will not build — cargo attaches them to the outer \
         workspace:\n  {}\n\n\
         Add an empty `[workspace]` table to each. The root Cargo.toml `exclude` \
         entry does not cover a package nested under a member crate's directory.",
        unmarked.join("\n  ")
    );
}

#[test]
fn fuzz_bin_names_ignores_the_package_name() {
    let manifest = r#"
[package]
name = "tau-domain-fuzz"

[[bin]]
name = "parse_skill_md"
path = "fuzz_targets/parse_skill_md.rs"

[[bin]]
name = "parse_package_manifest"
path = "fuzz_targets/parse_package_manifest.rs"

[profile.release]
debug = 1
"#;
    assert_eq!(
        fuzz_bin_names(manifest),
        ["parse_skill_md", "parse_package_manifest"]
    );
}

#[test]
fn declares_target_matches_whole_value_only() {
    let workflow = "        include:\n          - target: parse_skill_md\n          - target: x86_64-unknown-linux-gnu\n";
    assert!(declares_target(workflow, "parse_skill_md"));
    // A prefix of a declared target must not satisfy the gate.
    assert!(!declares_target(workflow, "parse_skill"));
    assert!(!declares_target(workflow, "parse_package_manifest"));
}
