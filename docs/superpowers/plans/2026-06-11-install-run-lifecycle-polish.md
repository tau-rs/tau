# Install → Run Lifecycle Polish (D4/D5/D8/D9) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish four low-severity install/run-lifecycle findings in one PR: SHA-pinned git installs (D4), a panic-free sync/async cross-check seam (D5), uuid/ulid run-ids (D8), and structured bundle-verify error rendering (D9).

**Architecture:** Four contained touches across the three declared files. D4 adds a pure clone-strategy decision + clone-then-checkout in `tau-pkg/src/git.rs`. D5 offloads the one async cross-check onto a dedicated OS thread (no nested Tokio runtime) in `tau-pkg/src/install.rs`. D8 replaces the bespoke `tau-run-<nanos>` id with a ULID in `tau-cli/src/cmd/run.rs`. D9 routes bundle-verify failures through a new `error_render::render_verify_error` + an exit-code-carrying marker downcast in `run_main`, instead of `eprintln!` + `process::exit`.

**Tech Stack:** Rust, `thiserror` typed errors, `tokio` (rt), `ulid`, `tracing`, `insta` snapshots.

---

## File Structure

- **`crates/tau-pkg/src/git.rs`** (D4) — add `CloneStrategy` enum + `clone_strategy(rev)` pure fn + `is_sha_shaped(rev)`; restructure `Git::clone` to branch on strategy; add a private `checkout_detached` helper. New `GitError::CheckoutFailed` variant in `error.rs`.
- **`crates/tau-pkg/src/error.rs`** (D4) — add `GitError::CheckoutFailed { exit_code, stderr }`.
- **`crates/tau-pkg/src/install.rs`** (D5) — add `block_on_in_fresh_thread` helper; replace the inline `Builder::new_current_thread()...block_on` with it.
- **`crates/tau-cli/src/cmd/run.rs`** (D8, D9) — ULID run-id; `BundleVerifyFailed` marker error + `bundle_verify_failure()` constructor; return `Err` instead of `process::exit`.
- **`crates/tau-cli/src/cmd/error_render.rs`** (D9) — add `render_verify_error(&VerifyError) -> String`.
- **`crates/tau-cli/src/lib.rs`** (D9) — downcast `BundleVerifyFailed` in `run_main`, print rendered text (no generic prefix), return its exit code.

**D5 scope note:** The brief's preferred "async end-to-end" refactor ripples `install_with_options` → `install` → `update_package` → `resolve_and_install_for_agent`/`install_planned` and ~25 sync test call-sites across 3 test files — far beyond the declared 3-file footprint and squarely the "if large, defer" case. The thread-offload fix below resolves the actual defect (nested `block_on` panic from *any* async context — multi-thread *or* current-thread) entirely within `install.rs`, with no caller ripple and no new tokio feature. Full async-end-to-end stays a tracked follow-up; this plan ships all four findings.

---

## Task 1: D4 — clone-strategy seam (pure decision)

**Files:**
- Modify: `crates/tau-pkg/src/git.rs`
- Test: inline `#[cfg(test)] mod tests` in `git.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `git.rs`:

```rust
    #[test]
    fn clone_strategy_distinguishes_sha_from_branch_and_tag() {
        use super::CloneStrategy;
        assert!(matches!(clone_strategy(None), CloneStrategy::Default));
        assert!(matches!(
            clone_strategy(Some("main")),
            CloneStrategy::Branch(_)
        ));
        assert!(matches!(
            clone_strategy(Some("v1.2.0")),
            CloneStrategy::Branch(_)
        ));
        // 40-char SHA-1
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert!(matches!(
            clone_strategy(Some(sha1)),
            CloneStrategy::CommitSha(_)
        ));
        // 64-char SHA-256
        let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(matches!(
            clone_strategy(Some(sha256)),
            CloneStrategy::CommitSha(_)
        ));
        // hex-ish but wrong length → treated as a branch/tag name
        assert!(matches!(
            clone_strategy(Some("abc123")),
            CloneStrategy::Branch(_)
        ));
        // 40 chars but not all hex → branch/tag
        assert!(matches!(
            clone_strategy(Some("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")),
            CloneStrategy::Branch(_)
        ));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg clone_strategy_distinguishes`
Expected: FAIL — `clone_strategy` / `CloneStrategy` not found.

- [ ] **Step 3: Implement the pure decision**

Add above `impl Git` in `git.rs`:

```rust
/// How [`Git::clone`] should materialize a revision.
///
/// `--branch <rev> --single-branch` only accepts branch and tag names.
/// A commit SHA must be fetched via a full clone followed by a detached
/// `git checkout <sha>` (D4): pinning to an immutable commit is the
/// security-relevant case and previously failed with git's "Remote
/// branch ... not found".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneStrategy {
    /// No `rev` — clone the remote's default branch.
    Default,
    /// A branch or tag name — `git clone --branch <name> --single-branch`.
    Branch(String),
    /// A commit SHA — full clone, then `git checkout --detach <sha>`.
    CommitSha(String),
}

/// True when `rev` is a full git object name (40-hex SHA-1 or 64-hex
/// SHA-256). Abbreviated SHAs are intentionally NOT matched: they are
/// ambiguous with short branch names, and commit pinning always uses
/// the full immutable object name.
pub(crate) fn is_sha_shaped(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Decide the clone strategy for an optional `rev`.
pub(crate) fn clone_strategy(rev: Option<&str>) -> CloneStrategy {
    match rev {
        None => CloneStrategy::Default,
        Some(r) if is_sha_shaped(r) => CloneStrategy::CommitSha(r.to_owned()),
        Some(r) => CloneStrategy::Branch(r.to_owned()),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg clone_strategy_distinguishes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-pkg/src/git.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-pkg): add clone-strategy seam distinguishing SHA from branch/tag (D4)"
```

---

## Task 2: D4 — wire clone-then-checkout for SHA revs

**Files:**
- Modify: `crates/tau-pkg/src/error.rs` (new `GitError::CheckoutFailed`)
- Modify: `crates/tau-pkg/src/git.rs` (`Git::clone` + `checkout_detached`)
- Update: doc-comments on the module + `Git::clone`
- Test: `crates/tau-pkg/tests/install_lifecycle.rs` (integration, file:// repo)

- [ ] **Step 1: Add the error variant**

In `crates/tau-pkg/src/error.rs`, inside `pub enum GitError`, after the `CloneFailed` variant:

```rust
    /// `git checkout --detach <sha>` exited non-zero (D4 SHA-pinning).
    #[error("git checkout failed: exit {exit_code}: {stderr}")]
    CheckoutFailed {
        /// Process exit code (`-1` if terminated by signal).
        exit_code: i32,
        /// Captured stderr from the failed `git checkout`.
        stderr: String,
    },
```

- [ ] **Step 2: Write the failing integration test**

First inspect the existing bare-repo fixture helpers at the top of `crates/tau-pkg/tests/install_lifecycle.rs` (look for a `git init --bare` / commit helper). Add a test that creates a local repo with two commits, captures the first commit SHA, and installs pinning that SHA. If the existing helpers expose only `install(&source, &scope)`, drive `Git::clone` directly is not possible (it's `pub(crate)`) — instead assert end-to-end via `install` with a `PackageSource::Git { rev: Some(<first_sha>) }` and verify the checked-out tree matches the first commit (e.g. a file that only exists in commit 1, or `resolved_commit == first_sha`).

```rust
#[test]
fn install_pins_to_a_commit_sha() {
    // Build a source repo with two commits; remember commit #1's SHA.
    let src = tau_ports::fixtures::scratch_dir("d4-sha-src");
    let (repo, first_sha) = make_two_commit_repo(src.path());
    // tau.toml/manifest live in commit #1; commit #2 adds a marker file
    // that must NOT be present when we pin commit #1.
    let scope_dir = tau_ports::fixtures::scratch_dir("d4-sha-scope");
    let scope = Scope::for_root(scope_dir.path()).unwrap();
    let source = PackageSource::Git {
        location: url_for_path(&repo),
        rev: Some(first_sha.clone()),
    };
    let installed = install(&source, &scope).expect("SHA-pinned install must succeed");
    // The lockfile-resolved commit equals the pinned SHA.
    assert_eq!(installed.resolved_commit.as_deref(), Some(first_sha.as_str()));
    // The commit-#2-only marker file is absent in the checked-out tree.
    assert!(
        !installed.install_path.join("ADDED_IN_COMMIT_2.txt").exists(),
        "pinned commit #1 must not contain commit #2's file"
    );
}
```

> Adapt field names (`resolved_commit`, `install_path`, `Scope::for_root`, `url_for_path`, `make_two_commit_repo`) to the actual helpers in that test file. If a two-commit helper does not exist, add a small local one using `std::process::Command::new("git")` mirroring the existing single-commit fixture. If the integration harness cannot easily express this, downgrade to a `git.rs`-level test that calls `Git::clone` with a `file://` two-commit repo and asserts the checkout, and note the substitution in the commit message.

- [ ] **Step 3: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg install_pins_to_a_commit_sha`
Expected: FAIL — today a SHA `rev` becomes `--branch <sha>` and git errors "Remote branch not found".

- [ ] **Step 4: Restructure `Git::clone` + add `checkout_detached`**

Replace the body of `Git::clone` that builds the command (the `cmd.arg("clone"); if let Some(rev) ...` block) so it dispatches on `clone_strategy`:

```rust
        let mut cmd = Command::new("git");
        // Allow file:// protocol even when system/CI git config restricts it.
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
            .env("GIT_CONFIG_VALUE_0", "always");
        cmd.arg("clone");

        let strategy = clone_strategy(rev_opt.as_deref());
        match &strategy {
            CloneStrategy::Default | CloneStrategy::CommitSha(_) => {
                // SHA pinning needs the full history reachable, so we do a
                // plain clone here and `git checkout <sha>` below.
                cmd.arg(&url_string).arg(dest);
            }
            CloneStrategy::Branch(name) => {
                cmd.arg("--branch").arg(name).arg("--single-branch");
                cmd.arg(&url_string).arg(dest);
            }
        }

        // Note: blocks until git exits; no timeout at v0.1 (sync API).
        let output = cmd.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitMissing
            } else {
                GitError::Io {
                    message: format!("spawning `git clone`: {e}"),
                }
            }
        })?;

        if !output.status.success() {
            return Err(GitError::CloneFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        // D4: a commit SHA cannot be requested via `--branch`; check it
        // out explicitly after a full clone.
        if let CloneStrategy::CommitSha(sha) = &strategy {
            Self::checkout_detached(dest, sha)?;
        }

        Ok(())
```

Add the helper inside `impl Git`:

```rust
    /// Detached-checkout `sha` in the freshly-cloned repo at `repo` (D4).
    ///
    /// Runs `git -C <repo> checkout --detach <sha>`. Used only for the
    /// `CloneStrategy::CommitSha` path — pinning an install to an
    /// immutable commit object.
    fn checkout_detached(repo: &Path, sha: &str) -> Result<(), GitError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("checkout")
            .arg("--detach")
            .arg(sha)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitMissing
                } else {
                    GitError::Io {
                        message: format!("spawning `git checkout --detach {sha}`: {e}"),
                    }
                }
            })?;

        if !output.status.success() {
            return Err(GitError::CheckoutFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(())
    }
```

Update the module-level "Revision-pinning limitation" doc-comment (lines 13-18) and `Git::clone`'s doc-comment to state that SHA revs are now supported via clone-then-`checkout --detach`, branch/tag via `--branch --single-branch`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg install_pins_to_a_commit_sha clone_strategy`
Expected: PASS both.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/git.rs crates/tau-pkg/src/error.rs crates/tau-pkg/tests/install_lifecycle.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-pkg): pin installs to a commit SHA via clone-then-checkout (D4)"
```

---

## Task 3: D5 — offload the cross-check onto a fresh thread (no nested runtime)

**Files:**
- Modify: `crates/tau-pkg/src/install.rs` (lines ~422-440 + new helper)
- Test: inline `#[cfg(test)] mod tests` in `install.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `install.rs`:

```rust
    #[test]
    fn block_on_in_fresh_thread_runs_with_no_ambient_runtime() {
        let v = super::block_on_in_fresh_thread(|| async { 21 + 21 }).unwrap();
        assert_eq!(v, 42);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_on_in_fresh_thread_from_multithread_async_does_not_panic() {
        // Regression: building a runtime inline here used to panic with
        // "Cannot start a runtime from within a runtime" (D5).
        let v = super::block_on_in_fresh_thread(|| async { 7 }).unwrap();
        assert_eq!(v, 7);
    }

    #[tokio::test]
    async fn block_on_in_fresh_thread_from_current_thread_async_does_not_panic() {
        // The current-thread async context is the other nesting case the
        // old inline `block_on` could not survive (D5).
        let v = super::block_on_in_fresh_thread(|| async { 9 }).unwrap();
        assert_eq!(v, 9);
    }
```

If `install.rs`'s test module is not already `#[tokio::test]`-capable, add `use` lines as needed; `tokio` with the `macros` + `rt` features is already a dependency.

- [ ] **Step 2: Run tests to verify they fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg block_on_in_fresh_thread`
Expected: FAIL — `block_on_in_fresh_thread` not found.

- [ ] **Step 3: Implement the helper**

Add near the top of `install.rs` (module-private, after imports):

```rust
/// Run `make_fut()` to completion from a synchronous function that may
/// itself be called from inside a Tokio runtime.
///
/// The install pipeline is synchronous but must `await` the Layer-2
/// cross-check exactly once. Building a current-thread runtime *inline*
/// and calling `block_on` panics ("Cannot start a runtime from within a
/// runtime") when `install_with_options` runs inside an existing async
/// context — which it does under `tau run` / `tau install` (both
/// `#[tokio::main]`). Offloading the runtime onto a dedicated OS thread
/// sidesteps the nesting entirely: the new thread has no ambient
/// runtime, so `block_on` is always legal there. The future is built
/// *inside* the thread, so it need not be `Send` — only the builder
/// closure and the output are.
///
/// Full "async end-to-end" of the install pipeline is the cleaner
/// long-term shape but ripples through every sync caller and ~25 test
/// sites; this contained seam fixes the panic without that churn (D5).
fn block_on_in_fresh_thread<T, B, Fut>(make_fut: B) -> Result<T, InstallError>
where
    B: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T>,
    T: Send + 'static,
{
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| InstallError::Internal {
                message: format!("build tokio runtime for cross-check: {e}"),
            })?;
        Ok(rt.block_on(make_fut()))
    })
    .join()
    .map_err(|_| InstallError::Internal {
        message: "cross-check worker thread panicked".into(),
    })?
}
```

> The closure returns `Result<T, InstallError>` (runtime-build failure path); `join()` yields `Result<Result<T, InstallError>, Box<dyn Any>>`, the outer mapped to `Internal`, then `?` flattens.

- [ ] **Step 4: Replace the inline runtime at the cross-check call-site**

In `install_with_options`, replace the `let runtime = tokio::runtime::Builder::new_current_thread()...block_on(...)...?;` block (currently lines ~424-438) with:

```rust
                let binary_path = lp.binary_path.clone();
                let manifest_for_check = manifest.clone();
                let shapes = block_on_in_fresh_thread(move || async move {
                    crate::sandbox_check::cross_check_plugin_capabilities(
                        &binary_path,
                        &manifest_for_check,
                    )
                    .await
                })?
                .map_err(|e| InstallError::CrossCheck {
                    message: e.to_string(),
                })?;
                lp.required_shapes = shapes;
```

Update the "Bridge via a current-thread tokio runtime spun up just for this step" comment (lines ~419-421) to describe the off-thread offload and why (no nested `block_on`).

> Confirm `PackageManifest: Clone` (it is used by value elsewhere in this module). If for some reason it is not `Clone`, wrap it in `std::sync::Arc` for the move instead.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg block_on_in_fresh_thread`
Expected: PASS all three.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/install.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "fix(tau-pkg): offload install cross-check to a fresh thread, no nested runtime (D5)"
```

---

## Task 4: D8 — ULID run-id

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs:183-189` + comment at 176-182
- Test: inline `#[cfg(test)] mod tests` in `run.rs`

- [ ] **Step 1: Write the failing test**

Add a small `pub(super) fn mint_run_id() -> String` seam (Step 3 defines it), then test it:

```rust
    #[test]
    fn mint_run_id_is_a_unique_ulid_across_fast_successive_calls() {
        let a = super::mint_run_id();
        let b = super::mint_run_id();
        // 26-char Crockford base32 ULID.
        assert_eq!(a.len(), 26, "ulid is 26 chars, got {a:?}");
        assert!(
            a.chars().all(|c| c.is_ascii_alphanumeric()),
            "ulid is alphanumeric, got {a:?}"
        );
        // Unique even back-to-back (the old tau-run-<nanos> scheme could
        // collide under same-nanosecond calls / clock adjustment).
        assert_ne!(a, b, "two run-ids minted back-to-back must differ");
        assert!(ulid::Ulid::from_string(&a).is_ok(), "must parse as ULID");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli mint_run_id_is_a_unique_ulid`
Expected: FAIL — `mint_run_id` not found.

- [ ] **Step 3: Implement the seam and use it**

Add near the other free functions in `run.rs`:

```rust
/// Mint a per-invocation run id.
///
/// A ULID (Crockford base32, 26 chars) — lexicographically sortable and
/// collision-resistant. Matches `run_main`'s workflow-run-id minting
/// (`ulid::Ulid::new()`); replaces the old bespoke `tau-run-<nanos>`
/// string, which could collide under fast successive runs or a backward
/// clock adjustment (D8). The only contract is uniqueness within a host
/// process; the kernel still mints its own `AgentInstanceId`.
pub(super) fn mint_run_id() -> String {
    ulid::Ulid::new().to_string()
}
```

Replace the `run_id` block (currently lines 183-189) with:

```rust
    let run_id = mint_run_id();
```

Trim the comment at lines 176-182 to reflect ULID minting (drop the "avoid a uuid dep / timestamp-based string" rationale).

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli mint_run_id_is_a_unique_ulid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/run.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "refactor(tau-cli): mint run-id as a ULID instead of tau-run-<nanos> (D8)"
```

---

## Task 5: D9 — structured bundle-verify error renderer

**Files:**
- Modify: `crates/tau-cli/src/cmd/error_render.rs` (new `render_verify_error`)
- Test: inline tests in `error_render.rs`

- [ ] **Step 1: Write the failing test**

Add to `error_render.rs` tests:

```rust
    #[test]
    fn verify_error_renders_structured_block() {
        use tau_pkg::bundle::VerifyError;
        let err = VerifyError::SelfHashMismatch {
            claimed: "aaaa".into(),
            computed: "bbbb".into(),
        };
        let rendered = render_verify_error(&err);
        assert!(
            rendered.contains("bundle verification failed"),
            "got: {rendered}"
        );
        // The guided Display detail is preserved in the structured output.
        assert!(rendered.contains("self-hash mismatch"), "got: {rendered}");
    }
```

> Confirm the exact field names of a cheap-to-construct `VerifyError` variant from `crates/tau-pkg/src/bundle/verify_error.rs`; `SelfHashMismatch { claimed, computed }` is used here — adapt if the real fields differ.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli verify_error_renders_structured_block`
Expected: FAIL — `render_verify_error` not found.

- [ ] **Step 3: Implement the renderer**

Add to `error_render.rs`:

```rust
/// Render a [`tau_pkg::bundle::VerifyError`] as a guided, structured
/// error message — the shared path `tau run --bundle` uses instead of a
/// bare `eprintln!("error: {e}")` (D9). The `VerifyError` Display strings
/// already carry remediation hints, so the renderer frames them with the
/// standard "✗" marker used by the other `render_*` helpers.
pub fn render_verify_error(err: &tau_pkg::bundle::VerifyError) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "✗ bundle verification failed");
    let _ = writeln!(out);
    let _ = writeln!(out, "  {err}");
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli verify_error_renders_structured_block`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-cli/src/cmd/error_render.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(tau-cli): add structured bundle-verify error renderer (D9)"
```

---

## Task 6: D9 — route bundle-verify failure through renderer + ExitCode (no process::exit)

**Files:**
- Modify: `crates/tau-cli/src/cmd/run.rs` (marker error + constructor + call-site)
- Modify: `crates/tau-cli/src/lib.rs` (`run_main` downcast)
- Test: inline tests in `run.rs`

- [ ] **Step 1: Write the failing test**

Add to `run.rs` tests:

```rust
    #[test]
    fn bundle_verify_failure_carries_renderer_output_and_mapped_code() {
        use tau_pkg::bundle::VerifyError;
        // integrity/install-state → 3
        let drift = VerifyError::SelfHashMismatch {
            claimed: "a".into(),
            computed: "b".into(),
        };
        let f = super::bundle_verify_failure(&drift);
        assert_eq!(f.code, 3);
        assert!(f.rendered.contains("bundle verification failed"));
        // bad-input/parse → 2
        let parse = VerifyError::UnsupportedSchemaVersion {
            found: 99,
            supported: 2,
        };
        assert_eq!(super::bundle_verify_failure(&parse).code, 2);
    }
```

> Adapt `UnsupportedSchemaVersion`'s field names to the actual definition if they differ.

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli bundle_verify_failure_carries`
Expected: FAIL — `bundle_verify_failure` / `BundleVerifyFailed` not found.

- [ ] **Step 3: Add the marker error + constructor; replace `process::exit`**

In `run.rs`, near `AgentFailed`:

```rust
/// Marker error: `tau run --bundle` verification failed.
///
/// Carries the structured-renderer output and the spec §C.3 exit code so
/// `run_main` can print the guided message (no generic "error:" prefix)
/// and exit with the mapped code — instead of the command body calling
/// `process::exit` and bypassing the shared error path (D9).
#[derive(Debug, thiserror::Error)]
#[error("bundle verification failed")]
pub(crate) struct BundleVerifyFailed {
    /// Process exit code per spec §C.3 (2 / 3 / 70).
    pub(crate) code: i32,
    /// Pre-rendered guided message (already through `render_verify_error`).
    pub(crate) rendered: String,
}

/// Build a [`BundleVerifyFailed`] from a `VerifyError`: structured
/// renderer output + the §C.3 exit-code mapping.
pub(crate) fn bundle_verify_failure(e: &tau_pkg::bundle::VerifyError) -> BundleVerifyFailed {
    BundleVerifyFailed {
        code: bundle_verify_exit_code(e),
        rendered: crate::cmd::error_render::render_verify_error(e),
    }
}
```

Replace the `Err(e) => { eprintln!("error: {e}"); std::process::exit(bundle_verify_exit_code(&e)); }` arm (lines 80-83) with:

```rust
            Err(e) => {
                return Err(bundle_verify_failure(&e).into());
            }
```

- [ ] **Step 4: Downcast in `run_main`**

In `crates/tau-cli/src/lib.rs`, in the `Err(err)` arm of `match dispatch(...)`, before the `AgentFailed` check (so it takes precedence over the generic prefix), add:

```rust
            if let Some(bvf) = err.downcast_ref::<crate::cmd::run::BundleVerifyFailed>() {
                // Already structured by render_verify_error — print as-is,
                // no generic "error:" prefix, and use the mapped §C.3 code.
                eprint!("{}", bvf.rendered);
                return std::process::ExitCode::from(bvf.code as u8);
            }
```

> Place this inside the existing `Err(err) => { ... }` block, ahead of the `if err.downcast_ref::<AgentFailed>()` branch. `run_main` returns `std::process::ExitCode`, so `return` here is valid.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-cli bundle_verify_failure_carries`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/src/cmd/run.rs crates/tau-cli/src/lib.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "fix(tau-cli): route bundle-verify failure through renderer + ExitCode (D9)"
```

---

## Task 7: Full verification + scope review

- [ ] **Step 1: Crate test suites green**

Run:
```bash
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-pkg
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p tau-cli
timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p tau-pkg --doc
```
Expected: all pass.

- [ ] **Step 2: Clippy + fmt**

Run:
```bash
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p tau-pkg -p tau-cli --all-targets
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p tau-pkg -p tau-cli -- --check
```
Expected: clean.

- [ ] **Step 3: Evidence captures (brief §3)**

- SHA-pinned install resolving: run/show the `install_pins_to_a_commit_sha` test output.
- Bundle-verify failure rendered through the structured path: a unit assertion or a quick `tau run --bundle <bad>` showing the `✗ bundle verification failed` block + exit code.

- [ ] **Step 4: `requesting-code-review`** — verify scope is install/run lifecycle only; no stray pipeline refactors.

- [ ] **Step 5: PR**

```bash
git push -u origin HEAD
gh pr create -R tau-rs/tau --base main \
  --title "fix(install-run): SHA pinning, cross-check seam, ULID run-id, bundle-verify rendering (D4/D5/D8/D9)" \
  --body "<cite D4, D5, D8, D9; note D5 done via thread-offload, async-end-to-end follow-up tracked>"
```
STOP — no merge.
```
```
