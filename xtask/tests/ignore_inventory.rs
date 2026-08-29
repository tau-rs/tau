//! Drift gate for `docs/test-ignores-inventory.md`.
//!
//! The inventory declares itself the canonical reference for what every
//! `#[ignore]`d test needs in order to run and which CI job lights it up, and
//! it carries the rule "every PR that adds, removes, or promotes a `#[ignore]`
//! annotation must update the corresponding row here in the same commit".
//!
//! That rule was unenforced for fifteen months and drifted accordingly: the
//! header said 22 while the workspace held 42, and six whole crates
//! (`tau-wasm-host`, `tau-cli`'s wasm half, `tau-ir-lower`, `tau-conformance`,
//! `tau-mcp-tokio`, and part of `tau-plugins`) had no bucket at all. An
//! uninventoried `#[ignore]` is a test nobody has decided anything about — the
//! 2026-08-23 sweep found seven such tests in `tau-cli` that were maintained
//! but run by no CI job whatsoever. This repo's stance is that a build-time
//! check that CAN exist MUST exist; an unenforced doc rule is exactly the hole.
//!
//! Coarse, forward-only invariant, deliberately mirroring
//! `xtask/tests/architecture_md.rs` and `xtask/tests/fuzz_matrix.rs`:
//! **the number of `#[ignore]` attributes under `crates/` must equal the total
//! the inventory's header declares.**
//!
//! This is a COUNT check, not a parse. It cannot tell you *which* row is wrong,
//! and that is the point: it is cheap enough for the Tier 0 gate and it forces
//! a human to open the doc and place the new annotation in a bucket. It is
//! explicitly **not** a lint against `#[ignore]` — several annotations here are
//! legitimately permanent (LIVE-DOCUMENTED needs real credentials;
//! ENVIRONMENT-SPECIFIC needs a host shape no GitHub runner provides). Adding
//! an `#[ignore]` is fine. Adding one silently is not.
//!
//! Deliberately not enforced: that each row's file:line is current (line
//! numbers drift on every edit and a stale line number misleads nobody),
//! bucket membership, or the Appendix's feature-gated dark lanes — that class
//! has no `#[ignore]` to count, so the table there is its only control.

use std::path::{Path, PathBuf};

/// The doc this gate keeps honest.
const INVENTORY: &str = "docs/test-ignores-inventory.md";

/// The header line the total is parsed out of. Keep the prefix in sync with
/// the doc; the gate reads the last whitespace-separated token as the count.
const TOTAL_PREFIX: &str = "**Total `#[ignore]` annotations:**";

fn repo_root() -> PathBuf {
    // xtask/ sits at the workspace root, so its parent is the repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent (the repo root)")
        .to_path_buf()
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `crates/*/fuzz/` holds standalone non-workspace projects and
            // `target/` may exist inside them; neither carries inventory rows.
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == "target" {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A line declares an ignored test iff its trimmed form opens the attribute.
///
/// Trimming is what excludes prose: every mention of the annotation in a doc
/// comment or `//` comment starts with a slash, never with `#`. `cfg_attr`-
/// wrapped ignores are invisible to this rule — none exist today, and one
/// appearing would under-count rather than fail open loudly, so the doc says
/// so out loud.
fn is_ignore_attribute(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("#[ignore]") || t.starts_with("#[ignore =") || t.starts_with("#[ignore(")
}

/// Count of `#[ignore]` attributes under `crates/`, with their locations.
fn find_ignores(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    files.sort();

    let mut found = Vec::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        for (i, line) in text.lines().enumerate() {
            if is_ignore_attribute(line) {
                found.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    found
}

/// The total the inventory's header declares.
fn declared_total(doc: &str) -> usize {
    let line = doc
        .lines()
        .find(|l| l.trim_start().starts_with(TOTAL_PREFIX))
        .unwrap_or_else(|| {
            panic!(
                "{INVENTORY} has no line starting with `{TOTAL_PREFIX}`.\n\n\
                 That header line is what this gate reads. Restore it as:\n  \
                 {TOTAL_PREFIX} <n>"
            )
        });

    line.split_whitespace()
        .next_back()
        .and_then(|tok| tok.parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!(
                "{INVENTORY}'s total line does not end in an integer:\n  {line}\n\n\
                 Expected `{TOTAL_PREFIX} <n>`."
            )
        })
}

#[test]
fn ignore_inventory_total_matches_the_workspace() {
    let root = repo_root();
    let doc = std::fs::read_to_string(root.join(INVENTORY))
        .unwrap_or_else(|e| panic!("{INVENTORY} must exist and be readable: {e}"));

    let declared = declared_total(&doc);
    let found = find_ignores(&root);
    let actual = found.len();

    assert_eq!(
        actual,
        declared,
        "`#[ignore]` inventory drift: {actual} annotation(s) under crates/, but \
         {INVENTORY} declares {declared}.\n\n\
         The inventory's rule: every PR that adds, removes, or promotes a \
         `#[ignore]` annotation must update the corresponding row there in the \
         same commit. This gate is that rule.\n\n\
         To fix: open {INVENTORY}, place each new annotation in a bucket \
         (LIVE-DOCUMENTED / DARK / ENVIRONMENT-SPECIFIC / DEFERRED) — naming \
         the CI job that lights it up if any does — and update the header total \
         and the Summary table. Do NOT satisfy this gate by deleting an \
         `#[ignore]`; several are legitimately permanent.\n\n\
         Annotations found ({actual}):\n  {}\n",
        found.join("\n  ")
    );
}
