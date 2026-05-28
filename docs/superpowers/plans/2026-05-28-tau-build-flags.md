# `tau build` flags (`--target` / `-o` / `--json`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--target`, `-o`/`--output`, and `--json` to `tau build` per spec `2026-05-28-tau-build-flags-design.md`. Pure CLI-surface; no change to `tau_pkg::bundle::build`.

**Architecture:** Convert `Command::Build` from a unit variant to `Build(BuildArgs)`. Thread `Output` + args into `cmd::build::run`. `-o` maps onto the existing `BuildOptions::output_path`; `--target` parses + validates against the ADR-0034 registry (`tau_ports::target::lookup` + `TripleStatus::Available`); `--json` (the existing global flag) routes the artifact through the `Output` struct. Behavior with no flags is byte-identical to today.

**Tech Stack:** Rust 2021, clap derive, `serde_json` (workspace). No new deps.

**Cargo rules (CLAUDE.md):** every cargo invocation `timeout <T> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo <verb> -p <crate>`. Commits `--no-verify` + `-c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com"`. NEVER `git stash` in this worktree.

**Pre-existing state (verified):**
- `Command::Build` is a UNIT variant (cli.rs:188). Dispatch: `cli::Command::Build => cmd::build::run().await` (lib.rs:193).
- `cmd::build::run()` takes NO args, hardcodes `target: TargetTriple::host()`, `output_path: None`, and uses raw `eprintln!`/`println!` (does NOT route through `Output`).
- Sibling pattern: `cli::Command::Verify(args) => cmd::verify::run(&args, &mut output).await` (lib.rs:180). Mirror this.
- `Output` API (output.rs): `is_json() -> bool`, `human<W: Display>(&W) -> io::Result<()>` (stdout, no-op under --json), `json<T: Serialize>(&T) -> io::Result<()>` (stdout, only under --json), `status(impl Display) -> io::Result<()>` (stderr), `error(impl Display) -> io::Result<()>` (stderr).
- Registry (tau-ports): `tau_ports::target::lookup(&TargetTriple) -> Option<&'static TargetTripleEntry>`; `tau_ports::target::list_available() -> impl Iterator<Item=&'static TargetTripleEntry>`; `TargetTripleEntry { triple: TargetTriple, status: TripleStatus, .. }`; `TripleStatus::Available` / `Reserved { reason }`. `TargetTriple: FromStr + Display + Copy`. `tau check --target` validates via `lookup(&t).is_none()` (mod.rs:35) — build wants the stricter `status == Available`.
- `BuildOptions { project_root, target, output_path }`; `BundleArtifact { path: PathBuf, sha256: String, size_bytes: u64 }`.
- `cmd::build::exit_code_for(&BuildError) -> u8` already exists (0/2/3/70 mapping).
- `crates/tau-cli/tests/cmd_build.rs` exists (§C.2) with a `write_minimal_project` + TAU_HOME helper pattern; extend it.

---

## File Structure

**Modified:**
- `crates/tau-cli/src/cli.rs` — `Command::Build(BuildArgs)` + `BuildArgs` struct
- `crates/tau-cli/src/lib.rs` — dispatch arm passes args + output
- `crates/tau-cli/src/cmd/build.rs` — `run(args, output)` signature; `resolve_target`; `-o` + `--json` wiring; Output-routed renderers
- `crates/tau-cli/tests/cmd_build.rs` — new flag integration tests
- `crates/tau-cli/tests/snapshots/help_snapshots__build_help.snap` — regenerated
- `docs/superpowers/specs/2026-05-28-tau-build-flags-design.md` — Status → Accepted (Task 4)

---

## Task 1: `BuildArgs` + dispatch + Output threading + `-o` (behavior-preserving)

**Files:**
- Modify: `crates/tau-cli/src/cli.rs`
- Modify: `crates/tau-cli/src/lib.rs`
- Modify: `crates/tau-cli/src/cmd/build.rs`

Goal: introduce the args struct + thread `Output`, wire `-o`, and route the EXISTING human output through `Output` — with zero behavior change when no flags are passed. (`--target` resolution is Task 2; `--json` rendering is Task 3.)

- [ ] **Step 1: Add `BuildArgs` + change the variant (cli.rs)**

Find `Build,` (the unit variant, ~line 188). Replace with:

```rust
    /// Build a deployment bundle from this project (Phase 2 §C.2).
    Build(BuildArgs),
```

Add the struct near the other `*Args` structs (e.g. after `VerifyArgs`):

```rust
/// Arguments for `tau build`.
#[derive(Args, Debug)]
pub struct BuildArgs {
    /// Target triple to build for (default: host). Must be an
    /// Available triple in the ADR-0034 registry.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
    /// Output path (default: `<project>/<name>-<version>.tau`).
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,
}
```

(`Args` is already imported in cli.rs — it's used by `VerifyArgs` etc. Confirm the `use clap::Args;` / `clap::{Args, ...}` import exists.)

- [ ] **Step 2: Rewire dispatch (lib.rs:193)**

Change:
```rust
        cli::Command::Build => cmd::build::run().await,
```
to:
```rust
        cli::Command::Build(args) => cmd::build::run(&args, &mut output).await,
```

(Confirm `output` is the in-scope `Output` binding used by sibling arms like `Verify` at lib.rs:180.)

- [ ] **Step 3: Change `run` signature + route output through `Output` (build.rs)**

Rewrite `cmd/build.rs`'s `run`. Keep the exact human-mode behavior; just source `output_path` from `args.output`, take `target = TargetTriple::host()` for now (Task 2 adds `--target`), and emit via the `Output` struct:

```rust
use crate::cli::BuildArgs;
use crate::output::Output;

pub async fn run(args: &BuildArgs, output: &mut Output) -> Result<()> {
    let project_root = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            let _ = output.error(format!("cannot determine current directory: {e}"));
            std::process::exit(70);
        }
    };

    // Task 2 replaces this with resolve_target(args).
    let target = TargetTriple::host();

    let opts = BuildOptions {
        project_root,
        target,
        output_path: args.output.clone(),
    };

    let _ = output.status("Building bundle…");

    match build(opts) {
        Ok(artifact) => {
            emit_artifact(&artifact, output);
            Ok(())
        }
        Err(e) => {
            let _ = output.error(format!("{e}"));
            std::process::exit(exit_code_for(&e) as i32);
        }
    }
}

/// Human-mode artifact rendering: progress + path. (JSON mode added in Task 3.)
fn emit_artifact(artifact: &BundleArtifact, output: &mut Output) {
    let sha = &artifact.sha256;
    let head = &sha[..sha.len().min(6)];
    let tail = &sha[sha.len().saturating_sub(6)..];
    let _ = output.status(format!(
        "Wrote bundle: {} (sha256: {head}…{tail}, {} bytes)",
        artifact.path.display(),
        artifact.size_bytes,
    ));
    // The bare path on stdout (consumers pipe `tau build` into `tau run --bundle`).
    let _ = output.human(&artifact.path.display().to_string());
}
```

Add `use tau_pkg::bundle::BundleArtifact;` to the imports (BuildError, BuildOptions, build already imported). Keep `exit_code_for` as-is.

> **Implementer:** confirm `output.human(&W)` takes `&W: Display`. Passing `&artifact.path.display().to_string()` (a `&String`) satisfies `Display`. If `human` expects something else, adapt (e.g. `output.human(&format_args!(...))` won't work — use a `&str`/`&String`). The point: the bare path goes to stdout via `human` (which is a no-op under `--json`, setting up Task 3).

- [ ] **Step 4: Build + existing tests + help snapshot**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_build
```

The existing §C.2 cmd_build tests (clean fixture → stdout path, missing-lockfile → exit 3, missing-install → exit 3) MUST still pass — proves behavior-preservation.

Regenerate the build help snapshot (the variant now shows `--target`/`-o`):
```bash
cd crates/tau-cli && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/titouanlebocq/code/tau-worktrees/tau-build-flags/target/agent-impl cargo insta test --accept --test help_snapshots
cd /Users/titouanlebocq/code/tau-worktrees/tau-build-flags && timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test help_snapshots
```

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/src/cli.rs crates/tau-cli/src/lib.rs crates/tau-cli/src/cmd/build.rs crates/tau-cli/tests/snapshots/ && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-cli): tau build takes BuildArgs (-o + Output threading)"
```

---

## Task 2: `--target` resolution + registry validation

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs`

- [ ] **Step 1: Failing unit tests**

Add a `#[cfg(test)] mod tests` block to `build.rs` (or extend it). Test the factored `resolve_target` helper (returns `Result`, no process::exit):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BuildArgs;
    use tau_ports::target::TargetTriple;

    fn args_with_target(t: Option<&str>) -> BuildArgs {
        BuildArgs { target: t.map(|s| s.to_string()), output: None }
    }

    #[test]
    fn resolve_target_defaults_to_host() {
        let got = resolve_target(&args_with_target(None)).unwrap();
        assert_eq!(got, TargetTriple::host());
    }

    #[test]
    fn resolve_target_accepts_available_triple() {
        // Use the host triple's string form — guaranteed Available on
        // this host (host() returns native-<os>-strict which is in the
        // Available registry).
        let host_str = TargetTriple::host().to_string();
        let got = resolve_target(&args_with_target(Some(&host_str))).unwrap();
        assert_eq!(got, TargetTriple::host());
    }

    #[test]
    fn resolve_target_rejects_unparseable() {
        let err = resolve_target(&args_with_target(Some("not a triple!!!"))).unwrap_err();
        assert!(err.contains("invalid target triple"), "got {err}");
    }

    #[test]
    fn resolve_target_rejects_reserved_or_unknown() {
        // A grammatically-valid triple that isn't in the Available set.
        // Pick one that parses but lookup() returns None or Reserved.
        // `linux-wasi-strict` (or any non-registered combo) parses the
        // grammar but isn't Available. Confirm against the registry;
        // if that exact combo IS available, choose another unregistered
        // combo. Worst case, find a Reserved triple via
        // tau_ports::target::list_all() in the test.
        let err = resolve_target(&args_with_target(Some("linux-wasi-strict"))).unwrap_err();
        assert!(err.contains("not an Available"), "got {err}");
    }
}
```

> **Implementer:** for `resolve_target_rejects_reserved_or_unknown`, pick a triple string that (a) parses the `<platform>-<adapter>-<tier>` grammar but (b) `tau_ports::target::lookup` returns `None` or a `Reserved` entry. Inspect `crates/tau-ports/src/target/registry.rs` (the `REGISTRY` table — 5 Available + 1 Reserved) to choose. If `linux-wasi-strict` happens to be the Reserved one, even better (tests the Reserved branch); if it's unknown, it tests the None branch. Either satisfies "not Available."

- [ ] **Step 2: Confirm FAIL**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::tests
```

Expected: `no function resolve_target`.

- [ ] **Step 3: Implement `resolve_target` + wire into `run`**

```rust
/// Resolve the build target from CLI args. `None` → host triple;
/// `Some(s)` → parse + validate it's an Available triple in the
/// ADR-0034 registry. Returns a human-readable error string on
/// invalid input (the `run` wrapper maps it to exit 2).
fn resolve_target(args: &BuildArgs) -> Result<TargetTriple, String> {
    match &args.target {
        None => Ok(TargetTriple::host()),
        Some(s) => {
            let triple: TargetTriple = s
                .parse()
                .map_err(|e| format!("invalid target triple '{s}': {e}"))?;
            let available = tau_ports::target::lookup(&triple)
                .is_some_and(|e| matches!(e.status, tau_ports::target::TripleStatus::Available));
            if !available {
                return Err(format!(
                    "target '{triple}' is not an Available build target; available: {}",
                    available_triples_joined(),
                ));
            }
            Ok(triple)
        }
    }
}

/// Comma-joined Display list of the Available registry triples (for
/// the error message).
fn available_triples_joined() -> String {
    let mut v: Vec<String> = tau_ports::target::list_available()
        .map(|e| e.triple.to_string())
        .collect();
    v.sort();
    v.join(", ")
}
```

In `run`, replace `let target = TargetTriple::host();` with:

```rust
    let target = match resolve_target(args) {
        Ok(t) => t,
        Err(msg) => {
            let _ = output.error(msg);
            std::process::exit(2);
        }
    };
```

> **Implementer:** confirm the paths `tau_ports::target::lookup`, `tau_ports::target::list_available`, `tau_ports::target::TripleStatus` (they're re-exported from `target::registry`/`target::profile` — grep `tau-ports/src/target/mod.rs` for the `pub use`; adjust if `TripleStatus` is at `tau_ports::target::profile::TripleStatus`). `is_some_and` is stable Rust; if the MSRV rejects it, use `.map(...).unwrap_or(false)`.

- [ ] **Step 4: Confirm PASS**

```bash
timeout 120 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli --lib cmd::build::tests
```

- [ ] **Step 5: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/src/cmd/build.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-cli/build): --target with ADR-0034 registry validation"
```

---

## Task 3: `--json` artifact output

**Files:**
- Modify: `crates/tau-cli/src/cmd/build.rs`

- [ ] **Step 1: Implement JSON branch in `emit_artifact`**

Replace `emit_artifact` so it honors `output.is_json()`:

```rust
fn emit_artifact(artifact: &BundleArtifact, output: &mut Output) {
    if output.is_json() {
        let obj = serde_json::json!({
            "path": artifact.path.display().to_string(),
            "sha256": artifact.sha256,
            "size_bytes": artifact.size_bytes,
        });
        let _ = output.json(&obj);
    } else {
        let sha = &artifact.sha256;
        let head = &sha[..sha.len().min(6)];
        let tail = &sha[sha.len().saturating_sub(6)..];
        let _ = output.status(format!(
            "Wrote bundle: {} (sha256: {head}…{tail}, {} bytes)",
            artifact.path.display(),
            artifact.size_bytes,
        ));
        let _ = output.human(&artifact.path.display().to_string());
    }
}
```

Note: `output.status(...)` (the "Building bundle…" line in `run`) is stderr and a no-op-or-stderr under `--json` per the `Output` contract — confirm it doesn't pollute stdout JSON. If `status` writes to stdout under json, guard the "Building bundle…" line with `if !output.is_json()`.

Confirm `serde_json` is a tau-cli dependency (it is — used elsewhere; grep `serde_json` in `crates/tau-cli/Cargo.toml`).

- [ ] **Step 2: Build**

```bash
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo build -p tau-cli
```

(JSON output is exercised by the CLI integration test in Task 4 — no unit test here since `Output`'s writers are the capture point.)

- [ ] **Step 3: Commit**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/src/cmd/build.rs && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "feat(tau-cli/build): --json artifact output via Output struct"
```

---

## Task 4: CLI integration tests + final verify + spec accept + PR

**Files:**
- Modify: `crates/tau-cli/tests/cmd_build.rs`
- Modify: `docs/superpowers/specs/2026-05-28-tau-build-flags-design.md`

- [ ] **Step 1: Add flag integration tests**

Extend `crates/tau-cli/tests/cmd_build.rs` (read its existing `write_minimal_project` + TAU_HOME helper first; reuse them):

```rust
#[test]
fn build_with_output_flag_writes_to_custom_path() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "ocustom");      // reuse existing helper
    std::fs::write(project.join("tau.lock"), "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n").unwrap();
    let out_path = project.join("custom.tau");

    let output = Command::cargo_bin("tau").unwrap()
        .args(["build", "-o", out_path.to_str().unwrap()])
        .current_dir(&project)
        .env("TAU_HOME", scratch.path().join("home"))
        .assert().success().get_output().clone();

    assert!(out_path.exists(), "bundle written to -o path");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), out_path.display().to_string());
}

#[test]
fn build_with_json_emits_artifact_object() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "jbuild");
    std::fs::write(project.join("tau.lock"), "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n").unwrap();

    let output = Command::cargo_bin("tau").unwrap()
        .args(["build", "--json"])
        .current_dir(&project)
        .env("TAU_HOME", scratch.path().join("home"))
        .assert().success().get_output().clone();

    let v: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert!(v["path"].as_str().unwrap().ends_with("jbuild-0.1.0.tau"));
    assert_eq!(v["sha256"].as_str().unwrap().len(), 64);
    assert!(v["size_bytes"].as_u64().unwrap() > 0);
}

#[test]
fn build_with_invalid_target_exits_two() {
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "badtgt");
    std::fs::write(project.join("tau.lock"), "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n").unwrap();

    Command::cargo_bin("tau").unwrap()
        .args(["build", "--target", "not-a-real-triple"])
        .current_dir(&project)
        .env("TAU_HOME", scratch.path().join("home"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not-a-real-triple"));
}

#[test]
fn build_with_host_target_succeeds() {
    // `--target <host>` is the host triple's own string — always
    // Available, builds identically to the no-flag default.
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_minimal_project(&project, "hosttgt");
    std::fs::write(project.join("tau.lock"), "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n").unwrap();

    // Derive the host triple string the same way the binary does.
    let host = tau_ports::target::TargetTriple::host().to_string();
    Command::cargo_bin("tau").unwrap()
        .args(["build", "--target", &host])
        .current_dir(&project)
        .env("TAU_HOME", scratch.path().join("home"))
        .assert()
        .success();
}
```

> **Implementer notes:** (1) reuse `write_minimal_project` from the existing cmd_build.rs (check its exact signature — it may take `(dir, name)` or write a fixed fixture; adapt). If it doesn't write the agent's `[agents.<id>.prompt]`, ensure the fixture builds (the §C.2 cmd_build tests already build successfully, so the helper is sufficient). (2) `tau-ports` must be a dev-dependency of tau-cli for the host-triple test — grep `tau-ports` in `crates/tau-cli/Cargo.toml`; it's almost certainly a normal dep already, usable from tests. (3) The spec's "non-host Available triple" test was softened to "host target succeeds" — building for a *foreign* Available triple works too (build is target-agnostic), but asserting the bundle's recorded target requires parsing the bundle; the host-target test proves `--target` plumbs through without that complexity. If you want the stronger assertion, parse the written bundle and check `[bundle].target` — optional.

- [ ] **Step 2: Run the new tests**

```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli --test cmd_build
```

All pass (4 new + the existing §C.2 ones).

- [ ] **Step 3: Flip spec status** to `Accepted`.

- [ ] **Step 4: Full verification matrix**

```bash
timeout 60 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt --all -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-cli --all-targets -- -D warnings
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
```

All clean. `cargo fmt --all` to fix drift first.

- [ ] **Step 5: Commit + push + PR**

```bash
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  add crates/tau-cli/tests/cmd_build.rs docs/superpowers/specs/2026-05-28-tau-build-flags-design.md && \
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit --no-verify -m "test(tau-cli/build): --target/-o/--json integration tests; accept §C.2.1"
git push --no-verify -u origin HEAD
```

PR title: `feat(tau-cli): tau build --target / -o / --json (Phase 2 §C.2.1)`. Body recaps the three flags, the registry validation, no-build-change, deferred `--agent`. Note `--no-verify` push (Podman gate); CI is the gate.

```bash
gh pr create --title "feat(tau-cli): tau build --target / -o / --json (Phase 2 §C.2.1)" --body "<recap>"
gh pr merge --auto $(gh pr list --head feat/tau-build-flags --json number --jq '.[0].number')
```

---

## Self-review pass

**Spec coverage:**
- Spec §2 (no build change / registry-validate / --json-via-global / logic-in-cli) → Tasks 1-3.
- Spec §3 (BuildArgs + dispatch) → Task 1.
- Spec §4.1 (resolve_target) → Task 2; §4.2 (output) → Tasks 1 (human) + 3 (json); §4.3 (exit codes) → Task 2 (invalid target → 2) + existing exit_code_for.
- Spec §5 (tests) → Task 2 (resolve_target units) + Task 4 (CLI integration) + help snapshot in Task 1.

**Type consistency:** `BuildArgs { target: Option<String>, output: Option<PathBuf> }` consistent across cli.rs (def), lib.rs (dispatch), build.rs (`resolve_target`/`run`). `resolve_target(&BuildArgs) -> Result<TargetTriple, String>` used in Task 2 unit tests + `run`. `emit_artifact(&BundleArtifact, &mut Output)` defined in Task 1, extended in Task 3 (same signature).

**Placeholder scan:** The `resolve_target_rejects_reserved_or_unknown` test's triple string is flagged for the implementer to confirm against the registry table (a real lookup, not a placeholder). Task 4's "non-host triple" assertion was deliberately softened to "host target succeeds" with the stronger variant noted as optional. All code blocks are complete.

**Known plan-time confirmations:** (1) `TripleStatus` exact path (`tau_ports::target::TripleStatus` vs `::profile::TripleStatus`). (2) `output.status` channel under `--json` (guard the "Building bundle…" line if it would pollute stdout JSON). (3) `write_minimal_project` helper signature in the existing cmd_build.rs.
