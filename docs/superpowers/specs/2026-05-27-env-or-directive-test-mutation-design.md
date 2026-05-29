# Eliminate env-mutation fragility in plugin config resolution tests

**Date:** 2026-05-27
**Status:** Approved
**Scope:** `crates/tau-plugins/{anthropic,openai,ollama}`

## Problem

The three LLM plugins resolve their auth credential from one of two
sources — a direct config directive (`api_key` / `bearer_token`) **or**
an environment variable named by `api_key_env` / `bearer_token_env`.
Call this the *env-or-directive* pattern.

The unit tests that exercise the env-var branch mutate process-global
state:

```rust
std::env::set_var(env_name, "sk-fromenv");
// ... assertions that may panic ...
std::env::remove_var(env_name);
```

Two fragilities:

1. **Panic leak.** If an assertion between `set_var` and `remove_var`
   panics, the variable is never removed and leaks into other tests in
   the same process.
2. **Process-global mutation.** `std::env::set_var` mutates state shared
   by every test thread. The workspace is edition 2021 today (so it
   compiles), but `set_var` becomes `unsafe` in edition 2024; this is a
   latent migration blocker. The retrospective (`docs/retrospectives/
   phase-0.md` §4) already records sub-project 4 hitting
   `forbid(unsafe_code)` on this exact call and pivoting away.

A second class of tests sets an env var only incidentally — they need
*a* constructed plugin and reach for the env path purely as a means to
get a valid credential, not to test env reading.

## Approach: closure injection

Make the resolver functions take an env-lookup closure instead of
calling `std::env::var` themselves. The single production caller passes
the real env; tests pass a fake. No process env is touched.

### Production changes (behavior-preserving)

- `openai/src/config.rs::resolve_api_key(cfg)` →
  `resolve_api_key(cfg, lookup: impl Fn(&str) -> Option<String>)`.
  Replace `std::env::var(&cfg.api_key_env)` with
  `lookup(&cfg.api_key_env)`, mapping `None` → `ConfigError::InvalidEnvVar`.
- `anthropic/src/config.rs::resolve_api_key` → identical shape.
- `ollama/src/config.rs::resolve_bearer_token(cfg)` → add the same
  `lookup` param. Replace the `match std::env::var(...)` arms with
  `lookup(...)`, preserving the existing semantics:
  - `Some(v)` where `v` is empty → `Ok(None)`
  - `Some(v)` non-empty → `Ok(Some(v))`
  - `None` (unset) → `Ok(None)`
- Each `Configure::from_config` caller (one per crate, in `plugin.rs`)
  passes the production lookup: `|n| std::env::var(n).ok()`.

These are `pub(crate)` signature changes; the only callers are inside
the same crate. No public API changes.

### Test changes (remove all `set_var` / `remove_var`)

- **Env-branch tests** (`resolve_*_reads_env_var`, the missing-env
  tests, `resolve_bearer_token_empty_env_treated_as_none`) pass a fake
  closure expressing the scenario directly:
  - present: `|_| Some("sk-fromenv".into())`
  - empty: `|_| Some(String::new())`
  - missing/unset: `|_| None`
- **Incidental tests** in `plugin.rs` (`from_config_*`, `name_returns_*`,
  the retry-validation test) that only needed *a* valid plugin switch to
  the direct `api_key` / `bearer_token` field, touching neither env nor
  the closure path.

## Out of scope

`set_var` usages in `tests/` integration suites (sandbox, scope_resolve,
CLI common) are a different concern (process spawning, config-dir
resolution) and are not touched by this PR. The TODO is scoped to the
LLM-plugin credential resolution.

## Testing & verification

- `cargo nextest run -p anthropic -p openai -p ollama` — all green.
- `cargo test --doc` for the 3 crates (config.rs doctests reference the
  resolver items).
- `grep -rn 'set_var\|remove_var' crates/tau-plugins/{anthropic,openai,ollama}/src`
  returns zero matches.
- `cargo clippy` + `cargo fmt --check` clean.
- Behavior is unchanged, so CI's existing coverage is the regression net.
