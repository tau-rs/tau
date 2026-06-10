# Plugin Secret Env Allowlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let env-provided plugin secrets (e.g. `ANTHROPIC_API_KEY`) reach plugin
subprocesses through an explicit allowlist, so a real key no longer has to be
written in cleartext into `tau.toml`.

**Architecture:** The plugin host calls `env_clear()` on the child `Command`
(security intent: plugins get a minimal, reproducible env, not the host's full
environment) and re-adds only `TAU_PLUGIN_RUN_ID`, `TAU_PLUGIN_AGENT_ID`, and
`PATH`. We extend that minimal set with an explicit, fixed allowlist of secret
env-var names (one per shipped LLM plugin). Only names on the allowlist are
re-injected, and only when present in the parent env — the full environment is
never passed back in. Env-var resolution is funneled through an injectable
`lookup` closure (matching the existing `resolve_api_key(cfg, lookup)` pattern in
the plugins) so the policy is unit-testable without spawning a subprocess or
mutating the test process's real environment.

**Tech Stack:** Rust, `tokio::process::Command`, `std::process::Command::get_envs`.

---

## Finding

HIGH/security (`audit/security.md`): Env-var API keys never reach plugins. The
plugin host calls `env_clear()` (`crates/tau-runtime-tokio/src/plugin_host/process.rs:213`)
and never re-adds `ANTHROPIC_API_KEY`, so a plugin's `from_config`
`std::env::var` lookup (`crates/tau-plugins/anthropic/src/plugin.rs:42`) always
fails — forcing the key into plaintext `tau.toml`.

Allowlist (default env-var name per shipped plugin):
- `ANTHROPIC_API_KEY` — anthropic (`config.rs` `default_api_key_env`)
- `OPENAI_API_KEY` — openai
- `OLLAMA_BEARER_TOKEN` — ollama

## File Structure

- Modify: `crates/tau-runtime-tokio/src/plugin_host/process.rs`
  - Add `SECRET_ENV_ALLOWLIST: &[&str]` constant.
  - Add `configure_plugin_command_env(command, run_id, agent_id, env_lookup)`
    helper that owns the `env_clear()` + minimal-env + allowlist-injection
    policy. Replaces the inline env chain in `spawn_and_handshake`.
  - Add unit test asserting allowlisted secrets are injected and
    non-allowlisted vars (including non-allowlisted secrets) are not.

---

### Task 1: Re-inject an explicit secret-env allowlist after `env_clear()`

**Files:**
- Modify: `crates/tau-runtime-tokio/src/plugin_host/process.rs`
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn configure_plugin_command_env_injects_allowlisted_secrets_only() {
    use std::collections::HashMap;

    // A fake parent environment: allowlisted secrets + PATH, plus
    // non-allowlisted entries (including a non-allowlisted secret) that
    // must NOT cross into the child.
    let parent: HashMap<&str, &str> = [
        ("ANTHROPIC_API_KEY", "sk-ant-parent"),
        ("OPENAI_API_KEY", "sk-openai-parent"),
        ("OLLAMA_BEARER_TOKEN", "ollama-parent"),
        ("PATH", "/usr/bin:/bin"),
        ("AWS_SECRET_ACCESS_KEY", "leak-me-not"),
        ("HOME", "/home/host-user"),
    ]
    .into_iter()
    .collect();

    let mut command = Command::new("/bin/true");
    configure_plugin_command_env(&mut command, "run-7", "agent-9", |name| {
        parent.get(name).map(|v| (*v).to_string())
    });

    let envs: HashMap<String, Option<String>> = command
        .as_std()
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();

    // Allowlisted secrets are re-injected from the parent env.
    assert_eq!(
        envs.get("ANTHROPIC_API_KEY"),
        Some(&Some("sk-ant-parent".to_string())),
        "ANTHROPIC_API_KEY must reach the plugin from the parent env"
    );
    assert_eq!(
        envs.get("OPENAI_API_KEY"),
        Some(&Some("sk-openai-parent".to_string()))
    );
    assert_eq!(
        envs.get("OLLAMA_BEARER_TOKEN"),
        Some(&Some("ollama-parent".to_string()))
    );

    // Required plumbing is still set.
    assert_eq!(
        envs.get("TAU_PLUGIN_RUN_ID"),
        Some(&Some("run-7".to_string()))
    );
    assert_eq!(
        envs.get("TAU_PLUGIN_AGENT_ID"),
        Some(&Some("agent-9".to_string()))
    );
    assert_eq!(envs.get("PATH"), Some(&Some("/usr/bin:/bin".to_string())));

    // Non-allowlisted vars (even secret-looking ones) stay cleared.
    assert!(
        !envs.contains_key("AWS_SECRET_ACCESS_KEY"),
        "non-allowlisted secret must not cross into the plugin"
    );
    assert!(!envs.contains_key("HOME"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-runtime-tokio --lib configure_plugin_command_env`
Expected: FAIL — `configure_plugin_command_env` not defined.

- [ ] **Step 3: Write minimal implementation**

Add near the top of `process.rs` (after imports):

```rust
/// Secret env-var names re-injected into every plugin child after
/// [`Command::env_clear`]. This is an **explicit allowlist**, not the
/// host's full environment: `env_clear()` exists precisely so plugins
/// run in a minimal, reproducible env, and only these well-known
/// secret names are passed back through.
///
/// One entry per shipped LLM plugin's default env-var name
/// (`default_api_key_env` / `default_bearer_token_env`):
/// - `ANTHROPIC_API_KEY` — `tau-plugins/anthropic`
/// - `OPENAI_API_KEY` — `tau-plugins/openai`
/// - `OLLAMA_BEARER_TOKEN` — `tau-plugins/ollama`
///
/// A plugin configured with a *custom* `api_key_env` name still needs
/// the cleartext-config path or a future per-plugin env declaration;
/// the host cannot enumerate arbitrary names without re-introducing the
/// pass-everything behavior `env_clear()` removed.
const SECRET_ENV_ALLOWLIST: &[&str] =
    &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OLLAMA_BEARER_TOKEN"];

/// Apply the plugin child's minimal, reproducible environment to
/// `command`: clear the inherited env, then re-add only the two
/// `TAU_PLUGIN_*` identity vars, `PATH` (for shared-library
/// resolution), and any [`SECRET_ENV_ALLOWLIST`] names present in the
/// parent env (resolved via `env_lookup`).
///
/// `env_lookup` is injected (rather than calling `std::env::var`
/// directly) so the policy is unit-testable without spawning a
/// subprocess or mutating the test process's real environment. The
/// production caller passes `|n| std::env::var(n).ok()`.
fn configure_plugin_command_env(
    command: &mut Command,
    run_id: &str,
    agent_id: &str,
    env_lookup: impl Fn(&str) -> Option<String>,
) {
    command
        .env_clear()
        .env("TAU_PLUGIN_RUN_ID", run_id)
        .env("TAU_PLUGIN_AGENT_ID", agent_id)
        // Inherit PATH so shared-library lookups (libc, libssl, …) work
        // the same as for the host.
        .env("PATH", env_lookup("PATH").unwrap_or_default());

    // Re-inject env-provided secrets so plaintext `tau.toml` is no
    // longer the only working path. Only allowlisted names cross over,
    // and only when actually set in the parent env.
    for name in SECRET_ENV_ALLOWLIST {
        if let Some(value) = env_lookup(name) {
            command.env(name, value);
        }
    }
}
```

Replace the inline env chain in `spawn_and_handshake` (currently
`.stdin(...).stdout(...).stderr(...).env_clear().env(...).env(...).env("PATH",...).kill_on_drop(true)`)
with stdio + kill_on_drop on `command`, then a call to the helper:

```rust
let mut command = Command::new(binary_path);
command
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
configure_plugin_command_env(&mut command, run_id, agent_id, |n| {
    std::env::var(n).ok()
});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `timeout 180 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test -p tau-runtime-tokio --lib configure_plugin_command_env`
Expected: PASS.

- [ ] **Step 5: Full crate test + clippy + fmt**

Run:
```
timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p tau-runtime-tokio
timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p tau-runtime-tokio --all-targets
timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo fmt -p tau-runtime-tokio --check
```
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-runtime-tokio/src/plugin_host/process.rs \
        docs/superpowers/plans/2026-06-11-plugin-secret-env-allowlist.md
git commit -m "fix(plugin-host): re-inject secret env-var allowlist into plugin children"
```
```
```
```
```

## Self-Review

- **Spec coverage:** Finding's two requirements — (a) env secrets reach plugins,
  (b) non-allowlisted vars stay cleared, full env never re-added — are both
  asserted by the Task 1 test and enforced by the allowlist loop. ✅
- **Placeholder scan:** none. ✅
- **Type consistency:** `configure_plugin_command_env` signature is identical in
  the implementation step, the call site, and the test. ✅
