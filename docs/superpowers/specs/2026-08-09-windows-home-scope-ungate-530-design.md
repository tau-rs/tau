# Un-gate #530 Windows home/scope Tier-2 tests — design

**Date:** 2026-08-09
**Issue:** #530 (Windows portability), continuation of PR #537
**Scope:** `crates/tau-pkg/src/scope.rs` (production + unit tests); remove 4 `#[cfg_attr(windows, ignore)]` gates in `tau-cli` and `tau-runtime-tokio`.

## Goal

Un-gate 4 Windows Tier-2 tests without regressing any main-green test:

| Test | File |
|---|---|
| `ungoverned_project_is_refused_on_wasm_path` | `crates/tau-cli/tests/cmd_build_wasm.rs` |
| `allow_ungoverned_flag_lets_it_proceed` | `crates/tau-cli/tests/cmd_build_wasm.rs` |
| `over_reaching_project_is_refused_on_wasm_path` | `crates/tau-cli/tests/cmd_build_wasm.rs` |
| `no_lockfile_scope_returns_not_found` | `crates/tau-runtime-tokio/src/skill_resolver_impl.rs` |

## Root cause

On the Windows GitHub runner `$HOME` is unset. `resolve_global_path()`
(`scope.rs`) resolves `TAU_HOME → XDG_DATA_HOME/tau → HOME/.tau`; with all
three unset it returns `ScopeError::HomeNotFound`. Every gated test
transitively resolves the **global** scope:

- The wasm tests call `wasm_governance_gate` → `CheckCtx::load`
  (`check/runner.rs:54`) → `Scope::resolve` → (no project `.tau` found) →
  `Scope::global()` → `HomeNotFound` → surfaced as
  `"cannot evaluate governance: resolve scope: ..."`. Tests expecting
  `GOV000` / `over_reach` / success fail.
- `no_lockfile_scope_returns_not_found` calls `Scope::resolve` on a bare
  tempdir → `global()` → `HomeNotFound` → mapped to
  `SkillResolveError::Invalid`, but the test asserts `NotFound`.

So all four need global scope resolution to **succeed** on Windows.

## Relationship to #537's reverted attempt (read this first)

PR #537 tried the obvious fix — add a `%USERPROFILE%` tier to
`resolve_global_path_from` — and **reverted it**. The exact reverted code
(checkpoint `78ed669d`) added the 4th tier but left `walk_up_for_dot_tau`
**unchanged**:

```rust
// reverted #537: 4th tier added...
if let Some(userprofile) = non_empty(userprofile) {
    return Ok(PathBuf::from(userprofile).join(".tau"));
}
// ...but walk_up_for_dot_tau was NOT touched.
```

Why that regressed: `Scope::global()` **creates** the resolved dir
(`materialize_global`). With the new tier, the first global resolution on
Windows creates `%USERPROFILE%\.tau`. Windows `TEMP` lives **under**
`%USERPROFILE%`, so `walk_up_for_dot_tau` — which climbs `cwd.ancestors()`
looking for any `.tau/` — finds that stray `%USERPROFILE%\.tau` in the
ancestry of **every** test tempdir and returns `ScopeKind::Project` instead
of `Global`. That regressed three main-green tests:

- `scope_resolve_falls_back_to_global_when_no_dot_tau` (`tests/scope_resolve.rs`)
- `resolve_ignores_dot_tau_that_is_a_file_not_a_dir` (`tests/scope_resolve.rs`)
- `resolve_idempotent_on_already_installed_deps` (`tau-cli/tests/cmd_resolve.rs`)

macOS/Linux hide this because their temp dirs (`/var/folders`, `/tmp`) are
not under `$HOME`.

**The reverted decision was incomplete, not wrong.** The missing piece is
making `~/.tau` (the global-scope location by convention) ineligible for
project discovery. This design re-adds the `%USERPROFILE%` tier **and** adds
that exclusion. Neither half works alone:

- Tier alone → pollution regression (what #537 saw).
- Exclusion alone → Windows still has no home; `global()` still crashes;
  the 4 tests stay red.

## Design

Two changes, both in `crates/tau-pkg/src/scope.rs`.

### Change 1 — `%USERPROFILE%` tier in `resolve_global_path`

Precedence becomes: `TAU_HOME → XDG_DATA_HOME/tau → HOME/.tau →
USERPROFILE/.tau`. `resolve_global_path_from` gains a 4th `user_profile`
param (kept as a pure, cross-platform-testable function). `resolve_global_path`
passes `env::var_os("USERPROFILE")` unconditionally — a no-op on Unix where
it is unset. `HOME` keeps priority over `USERPROFILE`.

### Change 2 — home-directory exclusion in `walk_up_for_dot_tau`

`walk_up_for_dot_tau` skips a `.tau` candidate whose parent ancestor **is a
user home directory**. It is keyed on the **home convention** (`$HOME`,
`%USERPROFILE%`), *not* on the resolved global path.

```rust
fn walk_up_for_dot_tau(cwd: &Path, home_dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    for ancestor in cwd.ancestors() {
        let candidate = ancestor.join(".tau");
        if fs::metadata(&candidate).map(|m| m.is_dir()).unwrap_or(false) {
            // `~/.tau` (or `%USERPROFILE%\.tau`) is the global scope by
            // convention — never a project root. See #530.
            if home_dirs.iter().any(|h| h == ancestor) {
                continue;
            }
            return Some((ancestor.to_path_buf(), candidate));
        }
    }
    None
}
```

`home_dirs` is computed from `{HOME, USERPROFILE}` env vars (the ones that
are set, non-empty). A small private helper `home_scope_dirs()` returns them;
`Scope::resolve` calls it and passes the slice. The `#[cfg(test)]`
`resolve_with_fallback` passes the same real-env home dirs (harmless — its
tempdirs are never *equal* to a real home dir).

**Why keyed on home, not on the resolved global:** the regression test
`resolve_idempotent_on_already_installed_deps` sets `TAU_HOME=proj`, so the
resolved global is `proj` — but the polluting dir is `%USERPROFILE%\.tau`.
Only a home-convention-keyed exclusion skips it. This choice also fixes a
latent Unix quirk: a project directly under `$HOME` with no closer `.tau`
currently resolves `~/.tau` as `Project`; after this change it correctly
falls through to `Global`.

**Why safe cross-platform:** `ancestor == home` matches only the literal
`~/.tau`. Project `.tau` dirs created *inside* a tempdir sit deeper than the
home dir, so `ancestor` is never the home there.

**Comparison must canonicalize (corrected after the first Windows CI run).**
An earlier draft compared `ancestor` and the home dir by plain `PathBuf`
equality, assuming both originate from the same process env / path walk. That
is false on Windows: `TempDir` paths come from `GetTempPath()` using an 8.3
short name (`C:\Users\RUNNER~1\...`) while `%USERPROFILE%` is the long name
(`C:\Users\runneradmin`) for the *same* directory, so plain equality misses
the match and the polluting `%USERPROFILE%\.tau` is treated as a project root
— exactly the #537 regression, reproduced on CI. `walk_up_for_dot_tau`
therefore compares `fs::canonicalize(ancestor)` against the canonicalized home
dirs (canonicalize resolves 8.3 short names, symlinks, and prefix/separator
differences). The ancestor is only canonicalized when a `.tau` candidate is
found *and* there is a home dir to compare against, keeping the common
no-home walk-up path IO-free.

## Behavior walk-through (post-change)

Gated tests (now green on Windows):

- **wasm tests:** fixture under repo tree (not under `%USERPROFILE%`) →
  `walk_up` finds no project `.tau` → `global()` → `%USERPROFILE%\.tau`
  created/opened → governance evaluates the fixture's `[allow]` ceiling →
  `GOV000` / `over_reach` / success as asserted.
- **`no_lockfile_scope_returns_not_found`:** tempdir under `%USERPROFILE%`
  → `walk_up` climbs, home-excludes `%USERPROFILE%\.tau` → `None` →
  `global()` → `Global` scope; global lockfile absent → `Ok(None)` →
  `NotFound`.

Regression tests (stay green, all platforms):

- **`resolve_ignores_dot_tau_that_is_a_file_not_a_dir`:** `proj/.tau` is a
  file → not `is_dir` → skipped; polluted `%USERPROFILE%\.tau` home-excluded
  → `global()` (now succeeds) → `Global`; `assert_ne(Project)` holds.
- **`scope_resolve_falls_back_to_global_when_no_dot_tau`:** no `.tau` found →
  `materialize_global(fake_home)` → `Global` at `fake_home`.
- **`resolve_idempotent_on_already_installed_deps`:** `%USERPROFILE%\.tau`
  home-excluded → falls to `TAU_HOME=proj` global → stable lockfile at
  `proj/tau-lock.toml` → second run reuses.

## Tests to add/update (`scope.rs` unit module)

- Update the 5 existing `resolve_global_path_from(...)` call sites to the
  4-arg form (pass `None` for `user_profile` where unrelated).
- Add `resolve_global_path_from_falls_back_to_userprofile` (only USERPROFILE
  set → `<userprofile>/.tau`).
- Add `resolve_global_path_from_prefers_home_over_userprofile`.
- Add a walk-up test driving the private `walk_up_for_dot_tau` directly with
  an explicit `home_dirs` slice (hermetic — no real-env dependence): a `.tau`
  dir whose parent is in `home_dirs` returns `None`; the same `.tau` with an
  empty `home_dirs` returns `Some(Project)`. This pins the exclusion
  independently of env.

## Then

Remove the 4 `#[cfg_attr(windows, ignore = "...#530")]` attributes. Remove
each gate only as its cluster is confirmed green.

## Non-goals

- No change to `Scope::global()` / `materialize_global` semantics.
- No graceful-degradation path (mapping `HomeNotFound` → empty scope): that
  would leave Windows genuinely home-less and only patch the tests, hiding
  the real gap. Giving Windows a real `%USERPROFILE%\.tau` global scope is
  the correct behavior and what #530 wants.

## Verification

- Local macOS/Linux: `cargo nextest` on `tau-pkg`, `tau-cli`,
  `tau-runtime-tokio` proves no cross-platform regression only.
- Real gate: PR with label `full-matrix`; watch `nextest / windows`
  **per-job** (`--no-fail-fast --retries 2`). The overall tier2 run may fail
  on an unrelated job — check the `nextest / windows` job specifically.
