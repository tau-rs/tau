# `tau verify --wasm` WIT-Reproducibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `tau verify --wasm <project> --wit <path>` that re-derives the guest WIT world from a project's declared capabilities and byte-compares it against a shipped `.wit` sidecar, proving the sidecar is the honest output of `generate_world(declared caps)`.

**Architecture:** A pure comparator (`compare_wit`) diffs two WIT strings and yields a `WitReproReport`. The verify command reads the shipped `.wit`, re-derives the world via the existing `wasm_world_for_project` seam (re-lowers the source — no wasm compile), compares, and maps the verdict to exit 0 (reproducible) / 2 (drift) / 1 (operational error). Mirrors the existing `tau verify --bundle` rebuild-and-compare branch.

**Tech Stack:** Rust, clap (CLI), sha2 (display hashes), assert_cmd + predicates (integration tests). All in `crates/tau-cli`.

## Global Constraints

- **Cargo commands** (CLAUDE.md): `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`. Never bare `cargo`, always `-p tau-cli`, always the target-dir + incremental-off prefix. Doctests use `cargo test --doc -p tau-cli`.
- **rustfmt is a separate required gate**: run `cargo fmt -p tau-cli` before every commit; `cargo fmt --all --check` before push.
- **No IR-format bump, no `tau.caps` section, no reading the `.wasm` binary.** Reproducibility is re-lower-source-and-compare only.
- **Re-derivation uses `wasm_world_for_project`, NOT the governed build path.** It must not run the governance gate — a reproducibility check is not a build.
- **Exit codes:** `0` reproducible, `2` drift, `1` operational error (unreadable `--wit`, project won't load, project not wasm-buildable). Operational errors MUST NOT surface as `2`.
- Existing seams (do not modify): `tau_cli::cmd::build_wasm::wasm_world_for_project(&Path) -> anyhow::Result<String>`; `tau_cli::cmd::build::hex_lower(&[u8]) -> String` (`pub(crate)`).

---

## File Structure

- **Create** `crates/tau-cli/src/cmd/verify_wasm.rs` — pure comparator: `WitReproReport`, `WitLineDiff`, `compare_wit`, private `sha256_hex`, unit tests. One responsibility: comparing two WIT strings.
- **Modify** `crates/tau-cli/src/cmd/mod.rs` — register `pub mod verify_wasm;`.
- **Modify** `crates/tau-cli/src/cli.rs:539` (`VerifyArgs`) — add `--wasm` and `--wit`.
- **Modify** `crates/tau-cli/src/cmd/verify.rs` — new `run_wasm_wit_check` function + branch dispatch; human/JSON rendering.
- **Modify** `crates/tau-cli/tests/cmd_verify.rs` — integration tests.

---

## Task 1: Pure WIT comparator

**Files:**
- Create: `crates/tau-cli/src/cmd/verify_wasm.rs`
- Modify: `crates/tau-cli/src/cmd/mod.rs` (add `pub mod verify_wasm;` in alpha order near line 36)
- Test: unit tests inline in `verify_wasm.rs`

**Interfaces:**
- Consumes: `crate::cmd::build::hex_lower` (`pub(crate)`), `sha2::{Digest, Sha256}`.
- Produces (Task 2 relies on these exact names/types):
  - `pub struct WitReproReport { pub reproducible: bool, pub shipped_sha256: String, pub rederived_sha256: String, pub first_diff: Option<WitLineDiff> }`
  - `pub struct WitLineDiff { pub line: usize, pub shipped: Option<String>, pub rederived: Option<String> }`
  - `pub fn compare_wit(shipped: &str, rederived: &str) -> WitReproReport`

- [ ] **Step 1: Write the failing tests**

Add to a new file `crates/tau-cli/src/cmd/verify_wasm.rs`:

```rust
//! Pure WIT-world reproducibility comparator for `tau verify --wasm`
//! (EPIC 3.5). Byte-compares a shipped `.wit` sidecar against the world
//! re-derived from a project's declared capabilities. No I/O lives here —
//! the caller reads the file and re-derives the world.

use crate::cmd::build::hex_lower;

/// The first line at which two WIT worlds diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitLineDiff {
    /// 1-indexed line number of the first divergence.
    pub line: usize,
    /// The shipped line at `line`, or `None` if shipped has fewer lines.
    pub shipped: Option<String>,
    /// The re-derived line at `line`, or `None` if re-derived has fewer lines.
    pub rederived: Option<String>,
}

/// Outcome of comparing a shipped WIT world against a re-derived one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitReproReport {
    /// True when the shipped and re-derived worlds are byte-identical.
    pub reproducible: bool,
    /// Lowercase-hex sha256 of the shipped world.
    pub shipped_sha256: String,
    /// Lowercase-hex sha256 of the re-derived world.
    pub rederived_sha256: String,
    /// First differing line. `None` when `reproducible`.
    pub first_diff: Option<WitLineDiff>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_worlds_are_reproducible() {
        let w = "package tau:generated@0.1.0;\nworld runner {}\n";
        let r = compare_wit(w, w);
        assert!(r.reproducible);
        assert_eq!(r.first_diff, None);
        assert_eq!(r.shipped_sha256, r.rederived_sha256);
    }

    #[test]
    fn one_changed_line_reports_first_diff() {
        let shipped = "line-a\nimport wasi:sockets/x@0.2.3;\nline-c\n";
        let rederived = "line-a\nimport wasi:http/types@0.2.3;\nline-c\n";
        let r = compare_wit(shipped, rederived);
        assert!(!r.reproducible);
        let d = r.first_diff.expect("diff present");
        assert_eq!(d.line, 2);
        assert_eq!(d.shipped.as_deref(), Some("import wasi:sockets/x@0.2.3;"));
        assert_eq!(d.rederived.as_deref(), Some("import wasi:http/types@0.2.3;"));
    }

    #[test]
    fn shipped_has_extra_trailing_line() {
        let shipped = "line-a\nline-b\nextra\n";
        let rederived = "line-a\nline-b\n";
        let r = compare_wit(shipped, rederived);
        assert!(!r.reproducible);
        let d = r.first_diff.expect("diff present");
        assert_eq!(d.line, 3);
        assert_eq!(d.shipped.as_deref(), Some("extra"));
        assert_eq!(d.rederived, None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (compile error — `compare_wit` undefined)**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli`
Expected: FAIL — `cannot find function compare_wit` (and `pub mod verify_wasm` not yet registered).

- [ ] **Step 3: Register the module**

In `crates/tau-cli/src/cmd/mod.rs`, add near the other `pub mod` lines (keep alpha order, after `pub mod verify;`):

```rust
pub mod verify_wasm;
```

- [ ] **Step 4: Implement `sha256_hex` and `compare_wit`**

Add to `verify_wasm.rs` above the `#[cfg(test)]` module:

```rust
/// Lowercase-hex sha256 of a byte slice (display-only; the verdict is exact
/// string equality, not the hash).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex_lower(&h.finalize())
}

/// Compare a shipped WIT world against a re-derived one. The verdict is exact
/// byte equality; on mismatch, `first_diff` names the first line that differs
/// (walking both sides in lockstep — either may run out of lines first).
pub fn compare_wit(shipped: &str, rederived: &str) -> WitReproReport {
    let reproducible = shipped == rederived;
    let first_diff = if reproducible {
        None
    } else {
        let mut s = shipped.lines();
        let mut r = rederived.lines();
        let mut line = 0usize;
        loop {
            line += 1;
            let (sl, rl) = (s.next(), r.next());
            match (sl, rl) {
                (Some(a), Some(b)) if a == b => continue,
                (None, None) => break None, // trailing-newline-only difference
                (a, b) => {
                    break Some(WitLineDiff {
                        line,
                        shipped: a.map(str::to_string),
                        rederived: b.map(str::to_string),
                    })
                }
            }
        }
    };
    WitReproReport {
        reproducible,
        shipped_sha256: sha256_hex(shipped.as_bytes()),
        rederived_sha256: sha256_hex(rederived.as_bytes()),
        first_diff,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli verify_wasm`
Expected: PASS (3 tests).

- [ ] **Step 6: Format + commit**

```bash
env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-cli
git add crates/tau-cli/src/cmd/verify_wasm.rs crates/tau-cli/src/cmd/mod.rs
git commit -m "feat(epic-3-5): pure WIT-world reproducibility comparator"
```

---

## Task 2: CLI wiring + command + integration tests

**Files:**
- Modify: `crates/tau-cli/src/cli.rs:539` (`VerifyArgs`)
- Modify: `crates/tau-cli/src/cmd/verify.rs` (branch + `run_wasm_wit_check` + rendering)
- Test: `crates/tau-cli/tests/cmd_verify.rs`

**Interfaces:**
- Consumes: `crate::cmd::verify_wasm::{compare_wit, WitReproReport, WitLineDiff}` (Task 1); `crate::cmd::build_wasm::wasm_world_for_project` (existing seam).
- Produces: `tau verify --wasm <PROJECT> --wit <PATH>` CLI surface with exit codes 0/2/1.

- [ ] **Step 1: Write the failing integration tests**

Append to `crates/tau-cli/tests/cmd_verify.rs`. The `wasm-build` fixtures already exist: `trivial` (host-only, empty caps), `net-http` (net cap), `needs-exec` (process-exec → not wasm-buildable).

```rust
use std::path::PathBuf;

/// Absolute path to a committed wasm-build fixture project.
fn wasm_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm-build")
        .join(name)
}

/// Test W1: a `.wit` produced by re-deriving the project's own world verifies
/// reproducible and exits 0.
#[test]
fn verify_wasm_matching_wit_exits_0() {
    let project = wasm_fixture("net-http");
    let world = tau_cli::cmd::build_wasm::wasm_world_for_project(&project).unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), &world).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reproducible"));
}

/// Test W2: a tampered `.wit` (extra import line injected) exits 2 and names
/// the differing line.
#[test]
fn verify_wasm_tampered_wit_exits_2() {
    let project = wasm_fixture("net-http");
    let world = tau_cli::cmd::build_wasm::wasm_world_for_project(&project).unwrap();
    let tampered = world.replace(
        "world runner {",
        "world runner {\n    import wasi:sockets/instance-network@0.2.3;",
    );
    assert_ne!(tampered, world, "tamper must change the world text");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), &tampered).unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("wasi:sockets"));
}

/// Test W3: empty-cap invariant — a host-only project re-derives to a world
/// byte-equal to the committed `wit-baseline/runner.wit`, so verifying against
/// that baseline exits 0. Guards the frozen baseline against generator drift.
#[test]
fn verify_wasm_host_only_matches_committed_baseline() {
    let project = wasm_fixture("trivial");
    let baseline = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tau-wasm-guest/wit-baseline/runner.wit");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// Test W4: a project that cannot target wasm (declares a process-exec tool)
/// exits 1 (operational error), NOT 2 (drift).
#[test]
fn verify_wasm_not_buildable_exits_1_not_2() {
    let project = wasm_fixture("needs-exec");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "irrelevant\n").unwrap();

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .code(1);
}

/// Test W5: a missing `--wit` file exits 1 with a clear read error.
#[test]
fn verify_wasm_missing_wit_exits_1() {
    let project = wasm_fixture("trivial");

    Command::cargo_bin("tau")
        .unwrap()
        .args([
            "verify",
            "--wasm",
            project.to_str().unwrap(),
            "--wit",
            "/nonexistent/does-not-exist.wit",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("does-not-exist.wit"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_verify verify_wasm`
Expected: FAIL — `--wasm`/`--wit` are unknown args (clap error), so every W-test fails.

- [ ] **Step 3: Add the CLI args**

In `crates/tau-cli/src/cli.rs`, inside `struct VerifyArgs` (after the `bundle` field near line 559):

```rust
    /// Reproducibility check for a wasm build: re-derive the guest WIT world
    /// from this project's declared capabilities and byte-compare it against
    /// the shipped `.wit` (`--wit`). Exit 0 reproducible / 2 drift / 1 error.
    /// Mutually exclusive with the package positional and `--bundle`.
    #[arg(long, value_name = "PROJECT", conflicts_with_all = ["package", "bundle"])]
    pub wasm: Option<std::path::PathBuf>,
    /// The shipped `.wit` sidecar to compare against (requires `--wasm`).
    #[arg(long, value_name = "PATH", requires = "wasm")]
    pub wit: Option<std::path::PathBuf>,
```

- [ ] **Step 4: Add the branch + command function**

In `crates/tau-cli/src/cmd/verify.rs`, add the branch at the top of `run` (right after the existing `--bundle` branch near line 34):

```rust
    // 0b. Wasm WIT-world reproducibility branch (EPIC 3.5). `--wit` is
    //     required-by `--wasm` at the clap layer, so it is Some here.
    if let Some(project) = args.wasm.clone() {
        let wit = args
            .wit
            .clone()
            .expect("clap `requires` guarantees --wit is present with --wasm");
        return run_wasm_wit_check(&project, &wit, output);
    }
```

Add the function (place it beside `run_reproducibility_check`):

```rust
/// `tau verify --wasm <project> --wit <path>` — re-derive the guest WIT world
/// from the project's declared caps and byte-compare against the shipped
/// `.wit`. Exit 0 reproducible / 2 drift / 1 operational error.
fn run_wasm_wit_check(
    project: &std::path::Path,
    wit_path: &std::path::Path,
    output: &mut Output,
) -> anyhow::Result<()> {
    use crate::cmd::verify_wasm::compare_wit;

    // Read the shipped sidecar. A missing/unreadable file is an operational
    // error (exit 1), never a drift verdict.
    let shipped = match std::fs::read_to_string(wit_path) {
        Ok(s) => s,
        Err(e) => {
            output.error(&format!("cannot read --wit file {}: {e}", wit_path.display()))?;
            std::process::exit(1);
        }
    };

    // Re-derive the world by re-lowering the source. `wasm_world_for_project`
    // enforces capability-fit; a non-wasm-buildable project (process-exec /
    // agent-spawn) surfaces as Err → exit 1 (operational), not exit 2 (drift).
    let rederived = match crate::cmd::build_wasm::wasm_world_for_project(project) {
        Ok(w) => w,
        Err(e) => {
            output.error(&format!(
                "cannot re-derive WIT world from {}: {e}",
                project.display()
            ))?;
            std::process::exit(1);
        }
    };

    let report = compare_wit(&shipped, &rederived);

    if output.is_json() {
        render_wasm_wit_json(&report, output)?;
    } else {
        render_wasm_wit_human(&report, output)?;
    }

    if report.reproducible {
        Ok(())
    } else {
        std::process::exit(2);
    }
}

/// Human-readable wasm WIT reproducibility report.
fn render_wasm_wit_human(
    report: &crate::cmd::verify_wasm::WitReproReport,
    output: &mut Output,
) -> anyhow::Result<()> {
    if report.reproducible {
        output.human(&format!(
            "\u{2713} WIT world reproducible (sha256 {})",
            abbrev(&report.shipped_sha256)
        ))?;
    } else {
        output.error("\u{2717} WIT world NOT reproducible")?;
        output.error(&format!("  shipped:   sha256 {}", abbrev(&report.shipped_sha256)))?;
        output.error(&format!(
            "  rederived: sha256 {}",
            abbrev(&report.rederived_sha256)
        ))?;
        if let Some(d) = &report.first_diff {
            output.error(&format!("  first diff at line {}:", d.line))?;
            output.error(&format!(
                "    shipped:   {}",
                d.shipped.as_deref().unwrap_or("<none>")
            ))?;
            output.error(&format!(
                "    rederived: {}",
                d.rederived.as_deref().unwrap_or("<none>")
            ))?;
        }
    }
    Ok(())
}

/// JSON wasm WIT reproducibility report (single object).
fn render_wasm_wit_json(
    report: &crate::cmd::verify_wasm::WitReproReport,
    output: &mut Output,
) -> anyhow::Result<()> {
    let first_diff = report.first_diff.as_ref().map(|d| {
        serde_json::json!({
            "line": d.line,
            "shipped": d.shipped,
            "rederived": d.rederived,
        })
    });
    output.json(&serde_json::json!({
        "event": "verify_wasm_wit",
        "reproducible": report.reproducible,
        "shipped_sha256": report.shipped_sha256,
        "rederived_sha256": report.rederived_sha256,
        "first_diff": first_diff,
    }))?;
    Ok(())
}
```

Note: `abbrev` already exists in `verify.rs` (used by the bundle path) — reuse it, do not redefine.

- [ ] **Step 5: Run the integration tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_verify verify_wasm`
Expected: PASS (W1–W5).

- [ ] **Step 6: Run the full tau-cli suite + doctests to catch regressions**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli`
Then: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test --doc -p tau-cli`
Expected: PASS. If a `--help` snapshot test (`help_snapshots`) fails because the two new flags now appear in `tau verify --help`, update the snapshot with `cargo insta review` or by accepting the new `.snap` (verify the diff only adds `--wasm`/`--wit` lines), then re-run.

- [ ] **Step 7: Clippy + format**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets`
Then: `env CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-cli`
Expected: no warnings (workspace lints are `-D warnings`).

- [ ] **Step 8: Commit**

```bash
git add crates/tau-cli/src/cli.rs crates/tau-cli/src/cmd/verify.rs crates/tau-cli/tests/cmd_verify.rs
# include any updated help snapshot:
git add crates/tau-cli/tests/snapshots/ 2>/dev/null || true
git commit -m "feat(epic-3-5): tau verify --wasm WIT-world reproducibility check"
```

---

## Self-Review

**Spec coverage:**
- CLI `tau verify --wasm <PROJECT> --wit <PATH>` → Task 2 Step 3. ✅
- `conflicts_with` package/bundle, `requires` wasm → Task 2 Step 3. ✅
- Re-derive via `wasm_world_for_project` (no governance gate) → Task 2 Step 4. ✅
- `compare_wit` pure comparator + `WitReproReport`/`WitLineDiff` → Task 1. ✅
- Exit 0/2/1 with operational-error ≠ drift → Task 2 Step 4 + tests W4/W5. ✅
- Human + JSON output (`verify_wasm_wit` event) → Task 2 Step 4. ✅
- Tests: unit compare (3), happy path (W1), drift (W2), empty-cap baseline invariant (W3), not-buildable exit-1 (W4), missing-wit exit-1 (W5). ✅
- No `tau.caps`, no IR bump, no `.wasm` binary read → nothing in the plan touches these. ✅

**Placeholder scan:** none — every step carries concrete code or an exact command.

**Type consistency:** `compare_wit`, `WitReproReport { reproducible, shipped_sha256, rederived_sha256, first_diff }`, `WitLineDiff { line, shipped, rederived }` used identically in Task 1 (definition), Task 2 (rendering), and the JSON event. `wasm_world_for_project` signature matches `build_wasm.rs`. `hex_lower`/`abbrev` reused, not redefined. ✅
