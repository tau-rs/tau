# Un-gate #530 Windows home/scope tests — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make global-scope resolution succeed on Windows (via a `%USERPROFILE%` tier) while making `~/.tau` ineligible for project discovery, so 4 gated Tier-2 tests un-gate without regressing main-green tests.

**Architecture:** Two localized changes in `crates/tau-pkg/src/scope.rs`: (1) add a `%USERPROFILE%` fallback tier to `resolve_global_path`; (2) exclude the home-directory `.tau` from `walk_up_for_dot_tau`, keyed on the home convention (`$HOME`/`%USERPROFILE%`), not the resolved global. Then delete 4 `#[cfg_attr(windows, ignore)]` gates.

**Tech Stack:** Rust, `cargo nextest`, `tempfile`.

## Global Constraints

- **Every cargo command:** `timeout <N> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/<role> cargo <cmd> -p <crate>`. Main agent role dir = `target/main`; subagents = `target/agent-<role>`. Timeouts: test 300s, build/check 180s, clippy 240s, fmt 30s. Prefer `cargo nextest run` for tests; `cargo test --doc` for doctests.
- **`forbid(unsafe_code)`** is in force in `tau-pkg`. No `std::env::set_var` in tests (it is `unsafe` on edition 2024). All new unit tests must be hermetic — pass paths/home-dir slices explicitly, never mutate process env.
- **Precedence (Change 1):** `TAU_HOME → XDG_DATA_HOME/tau → HOME/.tau → USERPROFILE/.tau`. `HOME` keeps priority over `USERPROFILE`.
- **Exclusion key (Change 2):** home *convention* dirs `{HOME, USERPROFILE}` (set, non-empty), compared via `fs::canonicalize` on both sides (required — Windows 8.3 short names like `RUNNER~1` vs long-name `%USERPROFILE%` defeat plain equality; caught by the first Windows CI run).
- **`fmt`/`clippy` clean** before any push (rustfmt is a separate required CI gate; `cargo fmt --check` before push).

## File Structure

- `crates/tau-pkg/src/scope.rs` — production (`resolve_global_path_from`, `resolve_global_path`, `walk_up_for_dot_tau`, `Scope::resolve`, `Scope::resolve_with_fallback`, new `home_scope_dirs`) + its `#[cfg(test)] mod tests`. Single file, single responsibility (scope resolution). All logic changes live here.
- `crates/tau-cli/tests/cmd_build_wasm.rs` — remove 3 gate attributes.
- `crates/tau-runtime-tokio/src/skill_resolver_impl.rs` — remove 1 gate attribute.

---

### Task 1: `%USERPROFILE%` tier in `resolve_global_path`

**Files:**
- Modify: `crates/tau-pkg/src/scope.rs` (`resolve_global_path_from`, `resolve_global_path`, and the 5 unit-test call sites)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn resolve_global_path_from(tau_home: Option<OsString>, xdg_data_home: Option<OsString>, home: Option<OsString>, user_profile: Option<OsString>) -> Result<PathBuf, ScopeError>` — 4-arg form. `resolve_global_path()` unchanged signature (`() -> Result<PathBuf, ScopeError>`), now passing `USERPROFILE` as the 4th arg.

- [ ] **Step 1: Write the failing tests** (add to `mod tests`)

```rust
#[test]
fn resolve_global_path_from_falls_back_to_userprofile() {
    use std::ffi::OsString;
    // Windows-runner shape: no TAU_HOME/XDG/HOME, only %USERPROFILE%. See #530.
    let p = resolve_global_path_from(None, None, None, Some(OsString::from("/x/userprofile")))
        .unwrap();
    assert_eq!(p, std::path::Path::new("/x/userprofile/.tau"));
}

#[test]
fn resolve_global_path_from_prefers_home_over_userprofile() {
    use std::ffi::OsString;
    let p = resolve_global_path_from(
        None,
        None,
        Some(OsString::from("/x/home")),
        Some(OsString::from("/x/userprofile")),
    )
    .unwrap();
    assert_eq!(p, std::path::Path::new("/x/home/.tau"));
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg -E 'test(resolve_global_path_from)'`
Expected: FAIL — compile error, `resolve_global_path_from` takes 3 args not 4.

- [ ] **Step 3: Add the 4th tier and update `resolve_global_path`**

Replace `resolve_global_path_from` (add `user_profile` param + branch after the `home` branch) and update its doc comment:

```rust
/// Resolve the global scope path from explicit env values (testable).
///
/// Precedence:
/// 1. `tau_home` if set and non-empty.
/// 2. `<xdg_data_home>/tau` if `xdg_data_home` is set and non-empty.
/// 3. `<home>/.tau`.
/// 4. `<user_profile>/.tau` — the Windows home fallback. Windows runners
///    leave `$HOME` unset but expose `%USERPROFILE%`. See #530.
///
/// Returns [`ScopeError::HomeNotFound`] if all four are missing/empty.
fn resolve_global_path_from(
    tau_home: Option<std::ffi::OsString>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
) -> Result<PathBuf, ScopeError> {
    fn non_empty(s: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
        s.filter(|v| !v.is_empty())
    }

    if let Some(p) = non_empty(tau_home) {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = non_empty(xdg_data_home) {
        return Ok(PathBuf::from(p).join("tau"));
    }
    if let Some(home) = non_empty(home) {
        return Ok(PathBuf::from(home).join(".tau"));
    }
    if let Some(user_profile) = non_empty(user_profile) {
        return Ok(PathBuf::from(user_profile).join(".tau"));
    }
    Err(ScopeError::HomeNotFound)
}
```

Then update `resolve_global_path` to pass the 4th arg (also update its doc `Precedence` list to add `4. $USERPROFILE/.tau`):

```rust
fn resolve_global_path() -> Result<PathBuf, ScopeError> {
    resolve_global_path_from(
        env::var_os("TAU_HOME"),
        env::var_os("XDG_DATA_HOME"),
        env::var_os("HOME"),
        env::var_os("USERPROFILE"),
    )
}
```

- [ ] **Step 4: Update the 5 existing `resolve_global_path_from` call sites to 4 args**

In `mod tests`, add a trailing `None` (or a value) so each call has 4 args:
- `resolve_global_path_from_prefers_tau_home`: append `Some(OsString::from("/x/userprofile"))`.
- `resolve_global_path_from_falls_back_to_xdg`: append `Some(OsString::from("/x/userprofile"))`.
- `resolve_global_path_from_falls_back_to_home`: append `None`.
- `resolve_global_path_from_treats_empty_as_unset`: append `Some(OsString::from(""))`.
- `resolve_global_path_from_returns_home_not_found_when_all_missing`: change to `resolve_global_path_from(None, None, None, None)`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg -E 'test(resolve_global_path)'`
Expected: PASS (all `resolve_global_path*` tests, old + 2 new).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/scope.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "fix(scope): add %USERPROFILE% global-path tier for Windows (#530)"
```

---

### Task 2: home-directory exclusion in `walk_up_for_dot_tau`

**Files:**
- Modify: `crates/tau-pkg/src/scope.rs` (`walk_up_for_dot_tau`, `Scope::resolve`, `Scope::resolve_with_fallback`, new `home_scope_dirs`)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `resolve_global_path` (unchanged) from Task 1.
- Produces:
  - `fn walk_up_for_dot_tau(cwd: &Path, home_dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)>` — now takes a home-dir exclusion slice.
  - `fn home_scope_dirs() -> Vec<PathBuf>` — the set of `{HOME, USERPROFILE}` dirs that are set and non-empty.

- [ ] **Step 1: Write the failing test** (add to `mod tests`)

```rust
#[test]
fn walk_up_excludes_dot_tau_in_home_dir() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".tau")).unwrap();

    // A `.tau` directly in a home dir is the global scope, not a project.
    assert_eq!(
        walk_up_for_dot_tau(&home, std::slice::from_ref(&home)),
        None,
        "home-dir .tau must not be discovered as a project root"
    );

    // With no home dirs excluded, the same `.tau` IS a project root.
    let (root, state) = walk_up_for_dot_tau(&home, &[]).expect("project hit");
    assert_eq!(root, home);
    assert_eq!(state, home.join(".tau"));
}
```

- [ ] **Step 2: Run test to verify it fails to compile**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg -E 'test(walk_up_excludes_dot_tau_in_home_dir)'`
Expected: FAIL — `walk_up_for_dot_tau` takes 1 arg not 2.

- [ ] **Step 3: Implement the exclusion + wiring**

Replace `walk_up_for_dot_tau`:

```rust
/// Walk up from `cwd` looking for a `.tau/` directory.
///
/// Returns `Some((scope_root, state_path))` on the first hit, or `None` if
/// no `.tau/` is found. A `.tau/` located *directly* inside a directory in
/// `home_dirs` is the global scope by convention (`~/.tau`) and is skipped —
/// it is never a project root. See #530.
fn walk_up_for_dot_tau(cwd: &Path, home_dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(".tau");
        if fs::metadata(&candidate)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            if home_dirs.iter().any(|h| h == ancestor) {
                continue;
            }
            return Some((ancestor.to_path_buf(), candidate));
        }
    }
    None
}
```

Add `home_scope_dirs` (place it next to `resolve_global_path`):

```rust
/// The user-home directories that host the global scope by convention
/// (`$HOME`, `%USERPROFILE%`). A `.tau/` located directly in one of these is
/// the global scope, not a project root, so `walk_up_for_dot_tau` skips it.
/// See #530.
fn home_scope_dirs() -> Vec<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| env::var_os(k))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}
```

Update `Scope::resolve`:

```rust
pub fn resolve(cwd: &Path) -> Result<Self, ScopeError> {
    let home_dirs = home_scope_dirs();
    if let Some((path, state_path)) = walk_up_for_dot_tau(cwd, &home_dirs) {
        return Ok(Self {
            path,
            state_path,
            kind: ScopeKind::Project,
        });
    }
    Self::global()
}
```

Update `Scope::resolve_with_fallback` (test-only) to pass home dirs too:

```rust
#[cfg(test)]
pub(crate) fn resolve_with_fallback(
    cwd: &Path,
    fallback_home: PathBuf,
) -> Result<Self, ScopeError> {
    let home_dirs = home_scope_dirs();
    if let Some((path, state_path)) = walk_up_for_dot_tau(cwd, &home_dirs) {
        return Ok(Self {
            path,
            state_path,
            kind: ScopeKind::Project,
        });
    }
    Self::materialize_global(fallback_home)
}
```

- [ ] **Step 4: Run the full tau-pkg suite (unit + integration) to verify pass + no regression**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-pkg`
Expected: PASS — including `scope_resolve_*`, `walk_up_excludes_dot_tau_in_home_dir`, and the `tests/scope_resolve.rs` integration tests.

- [ ] **Step 5: Run doctests** (`resolve`/`global` doc examples must still hold)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-pkg --doc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-pkg/src/scope.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "fix(scope): exclude home-dir .tau from project walk-up (#530)"
```

---

### Task 3: remove the 4 Windows gate attributes

**Files:**
- Modify: `crates/tau-cli/tests/cmd_build_wasm.rs:70-73,83-86,98-101` (three `#[cfg_attr(windows, ignore = "...#530")]`)
- Modify: `crates/tau-runtime-tokio/src/skill_resolver_impl.rs:80-83` (one `#[cfg_attr(windows, ignore = "...#530")]`)

**Interfaces:**
- Consumes: the now-working Windows scope resolution from Tasks 1–2.
- Produces: nothing (test-gate removal only).

- [ ] **Step 1: Remove the 3 gates in `cmd_build_wasm.rs`**

Delete the attribute block above each of `ungoverned_project_is_refused_on_wasm_path`, `allow_ungoverned_flag_lets_it_proceed`, and `over_reaching_project_is_refused_on_wasm_path`:

```rust
#[cfg_attr(
    windows,
    ignore = "no Windows home/scope resolution for governance eval; see #530"
)]
```

Leave the `#[tokio::test]` line intact directly above each `async fn`.

- [ ] **Step 2: Remove the 1 gate in `skill_resolver_impl.rs`**

Delete the attribute block above `no_lockfile_scope_returns_not_found`:

```rust
#[cfg_attr(
    windows,
    ignore = "no Windows home/scope resolution (Scope::resolve fails, not NotFound); see #530"
)]
```

Leave the `#[test]` line intact directly above `fn no_lockfile_scope_returns_not_found`.

- [ ] **Step 3: Verify no stray #530 gates remain**

Run: `git grep -n "530" crates/tau-cli/tests/cmd_build_wasm.rs crates/tau-runtime-tokio/src/skill_resolver_impl.rs`
Expected: no `cfg_attr … ignore … 530` lines in these two files (the 4 blocks are gone).

- [ ] **Step 4: Compile + run the affected tests on this platform (proves un-gated tests still pass on macOS/Linux)**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-tokio -E 'test(no_lockfile_scope_returns_not_found)'
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-cli -E 'test(wasm)'
```
Expected: PASS (these were never `ignore`d on non-Windows; this only confirms nothing else broke).

- [ ] **Step 5: fmt + clippy gate**

Run:
```
timeout 30 env CARGO_TARGET_DIR=target/main cargo fmt -p tau-pkg -- --check
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-pkg --all-targets
```
Expected: clean (no diff; no warnings — CI treats warnings as deny).

- [ ] **Step 6: Commit**

```bash
git add crates/tau-cli/tests/cmd_build_wasm.rs crates/tau-runtime-tokio/src/skill_resolver_impl.rs
git -c user.name="Titouan Lebocq" -c user.email="lebocq.tit@gmail.com" \
  commit -m "test: un-gate 4 Windows home/scope tests (#530)"
```

---

## Post-implementation: Windows verification (not a code task)

Local macOS/Linux green only proves no cross-platform regression. The real
check is Windows CI:

1. Push branch, open PR (`gh pr create --base main`).
2. Add the `full-matrix` label to trigger Tier 2.
3. Watch the **`nextest / windows`** job specifically (the overall tier2 run
   may red on an unrelated job). `--no-fail-fast --retries 2` is configured.
   `gh run watch --exit-status` returns 0 even on CANCELLED — inspect the
   job conclusion directly; raw log via
   `gh api repos/<owner>/tau/actions/jobs/<id>/logs` (`gh run view --log` truncates).
4. Confirm all 4 previously-gated tests now run and pass on Windows.

## Self-Review notes

- **Spec coverage:** Change 1 → Task 1; Change 2 → Task 2; gate removal →
  Task 3; verification section → post-implementation. All spec sections mapped.
- **Type consistency:** `walk_up_for_dot_tau(&Path, &[PathBuf])`,
  `home_scope_dirs() -> Vec<PathBuf>`, `resolve_global_path_from(_, _, _, _)`
  used consistently across tasks.
- **Hermetic tests:** no `env::set_var`; new tests pass explicit paths/slices.
