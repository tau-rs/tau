//! Freshness gate for the repo's `ARCHITECTURE.md` code-map (the stable spine
//! of the living-documentation system).
//!
//! Coarse, forward-only invariant: **every top-level `crates/<name>` that is a
//! real crate (`[package]`) must be named in `ARCHITECTURE.md`.** This catches
//! the one failure mode a hand-written map actually suffers — "added a crate,
//! forgot the map" — without pretending to track fine-grained detail (that
//! lives in the ADRs / roadmap / per-EPIC implementation trees).
//!
//! Deliberately not enforced: the reverse direction (a stale name lingering in
//! the map), plugin sub-crates under `crates/tau-plugins/*` (grouped in the
//! map), and any dir without a `[package]` (group dirs like `tau-plugins/`).

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // xtask/ sits at the workspace root, so its parent is the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent (the repo root)")
        .to_path_buf()
}

/// A directory is a crate iff it has a `Cargo.toml` declaring a `[package]`.
fn is_crate(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml"))
        .map(|s| s.contains("[package]"))
        .unwrap_or(false)
}

#[test]
fn architecture_md_names_every_top_level_crate() {
    let root = repo_root();
    let arch = std::fs::read_to_string(root.join("ARCHITECTURE.md"))
        .expect("ARCHITECTURE.md must exist at the repo root");

    let mut missing = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).expect("crates/ dir is readable") {
        let path = entry.expect("readable dir entry").path();
        if !path.is_dir() || !is_crate(&path) {
            continue;
        }
        let name = path
            .file_name()
            .expect("crate dir has a name")
            .to_string_lossy()
            .into_owned();
        // Match the backtick-wrapped exact name so `tau-ir` isn't satisfied by
        // an incidental `tau-ir-lower` mention.
        if !arch.contains(&format!("`{name}`")) {
            missing.push(name);
        }
    }
    missing.sort();

    assert!(
        missing.is_empty(),
        "ARCHITECTURE.md is stale — these crates are not named in the code-map:\n  {}\n\n\
         Add each to ARCHITECTURE.md (as `crate-name` in backticks). The map is the \
         stable spine; keep it honest.",
        missing.join("\n  ")
    );
}
