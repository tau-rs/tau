# Env-or-Directive Test-Mutation Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all `std::env::set_var`/`remove_var` usage from the anthropic/openai/ollama plugin config tests by injecting an env-lookup closure into the credential resolvers.

**Architecture:** Each `pub(crate)` resolver (`resolve_api_key` / `resolve_bearer_token`) gains a `lookup: impl Fn(&str) -> Option<String>` parameter and calls it instead of `std::env::var`. The one production caller per crate (`Configure::from_config`) passes `|n| std::env::var(n).ok()`. Tests pass a fake closure and stop mutating process env; incidental plugin tests switch to the direct credential field. Behavior is unchanged.

**Tech Stack:** Rust (edition 2021 workspace), `cargo nextest`, `ConfigError`.

**Cargo rules (from CLAUDE.md):** prefix every cargo invocation with `timeout <n> env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main`, scope with `-p`, prefer `cargo nextest`.

---

### Task 1: Anthropic plugin

**Files:**
- Modify: `crates/tau-plugins/anthropic/src/config.rs` (`resolve_api_key` + its tests)
- Modify: `crates/tau-plugins/anthropic/src/plugin.rs` (`from_config` caller + its tests)

- [ ] **Step 1: Change `resolve_api_key` signature + body**

In `crates/tau-plugins/anthropic/src/config.rs`, replace the function (currently lines ~146-167):

```rust
pub(crate) fn resolve_api_key(
    cfg: &AnthropicConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let key = if let Some(direct) = cfg.api_key.as_ref() {
        tracing::warn!(
            target: "anthropic_plugin::config",
            "config.api_key set directly — recommended only for tests"
        );
        direct.clone()
    } else {
        lookup(&cfg.api_key_env).ok_or_else(|| ConfigError::InvalidEnvVar {
            name: cfg.api_key_env.clone(),
            detail: "env var is not set; set it or use config.api_key (test-only)".into(),
        })?
    };

    if !key.starts_with("sk-ant-") {
        return Err(ConfigError::InvalidValue {
            field: "api_key",
            detail: "Anthropic API keys start with `sk-ant-`".into(),
        });
    }
    Ok(key)
}
```

- [ ] **Step 2: Update the production caller in `plugin.rs`**

In `crates/tau-plugins/anthropic/src/plugin.rs`, in `from_config` (line ~42), change:

```rust
let api_key = resolve_api_key(&cfg)?;
```

to:

```rust
let api_key = resolve_api_key(&cfg, |n| std::env::var(n).ok())?;
```

- [ ] **Step 3: Rewrite the config.rs tests that touched env**

In `crates/tau-plugins/anthropic/src/config.rs`, replace the `resolve_api_key_uses_config_override`, `resolve_api_key_reads_env_var`, and `resolve_api_key_missing_env_returns_invalid_env_var` tests with:

```rust
    #[test]
    fn resolve_api_key_uses_config_override() {
        let cfg = AnthropicConfig {
            api_key: Some("sk-ant-test123".into()),
            ..AnthropicConfig::default()
        };
        // Direct field wins; lookup must not be consulted.
        let key = resolve_api_key(&cfg, |_| panic!("lookup should not be called")).unwrap();
        assert_eq!(key, "sk-ant-test123");
    }

    #[test]
    fn resolve_api_key_reads_env_var() {
        let cfg = AnthropicConfig {
            api_key_env: "ANY_NAME".into(),
            ..AnthropicConfig::default()
        };
        let key = resolve_api_key(&cfg, |_| Some("sk-ant-fromenv".into())).unwrap();
        assert_eq!(key, "sk-ant-fromenv");
    }

    #[test]
    fn resolve_api_key_missing_env_returns_invalid_env_var() {
        let cfg = AnthropicConfig {
            api_key_env: "DEFINITELY_NOT_SET_OPDIQWXZ".into(),
            ..AnthropicConfig::default()
        };
        let err = resolve_api_key(&cfg, |_| None).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidEnvVar { ref name, .. }
                if name == "DEFINITELY_NOT_SET_OPDIQWXZ"
        ));
    }
```

(If a `resolve_api_key_malformed_prefix` / `InvalidValue` test exists, give it a `|_| None` arg too — it uses the direct `api_key` field so the closure is never called, but the signature still requires the argument.)

- [ ] **Step 4: Rewrite the plugin.rs tests that touched env**

In `crates/tau-plugins/anthropic/src/plugin.rs`, replace the env-setting tests:

```rust
    #[test]
    fn from_config_with_valid_config_constructs_plugin() {
        let cfg = AnthropicConfig {
            api_key: Some("sk-ant-test-key-123".into()),
            ..AnthropicConfig::default()
        };
        let result = AnthropicPlugin::from_config(cfg);
        assert!(result.is_ok());
    }

    #[test]
    fn name_returns_anthropic() {
        let cfg = AnthropicConfig {
            api_key: Some("sk-ant-foo".into()),
            ..AnthropicConfig::default()
        };
        let plugin = AnthropicPlugin::from_config(cfg).unwrap();
        assert_eq!(plugin.name(), "anthropic");
    }
```

Leave `from_config_with_missing_api_key_returns_invalid_env_var` as-is — it already uses an unset env-var name and does not call `set_var`; with the production lookup `|n| std::env::var(n).ok()` an unset var yields `None` → `InvalidEnvVar`, so it still passes.

- [ ] **Step 5: Run the crate tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p anthropic`
Expected: PASS, 0 failures.

- [ ] **Step 6: Confirm no env mutation remains in this crate**

Run: `grep -rn 'set_var\|remove_var' crates/tau-plugins/anthropic/src`
Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-plugins/anthropic/src/config.rs crates/tau-plugins/anthropic/src/plugin.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "refactor(anthropic): inject env lookup into resolve_api_key

Removes std::env::set_var/remove_var from the config + plugin tests."
```

---

### Task 2: OpenAI plugin

**Files:**
- Modify: `crates/tau-plugins/openai/src/config.rs` (`resolve_api_key` + its tests)
- Modify: `crates/tau-plugins/openai/src/plugin.rs` (`from_config` caller + its tests)

- [ ] **Step 1: Change `resolve_api_key` signature + body**

In `crates/tau-plugins/openai/src/config.rs`, replace the function (currently lines ~150-171):

```rust
pub(crate) fn resolve_api_key(
    cfg: &OpenAIConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let key = if let Some(direct) = cfg.api_key.as_ref() {
        tracing::warn!(
            target: "openai_plugin::config",
            "config.api_key set directly — recommended only for tests"
        );
        direct.clone()
    } else {
        lookup(&cfg.api_key_env).ok_or_else(|| ConfigError::InvalidEnvVar {
            name: cfg.api_key_env.clone(),
            detail: "env var is not set; set it or use config.api_key (test-only)".into(),
        })?
    };

    if !key.starts_with("sk-") {
        return Err(ConfigError::InvalidValue {
            field: "api_key",
            detail: "OpenAI API keys start with `sk-` (legacy) or `sk-proj-` (modern)".into(),
        });
    }
    Ok(key)
}
```

- [ ] **Step 2: Update the production caller in `plugin.rs`**

In `crates/tau-plugins/openai/src/plugin.rs`, in `from_config`, change `resolve_api_key(&cfg)?` to:

```rust
let api_key = resolve_api_key(&cfg, |n| std::env::var(n).ok())?;
```

(Match the exact existing binding name; if the existing line is `let api_key = resolve_api_key(&cfg)?;`, only the argument list changes.)

- [ ] **Step 3: Rewrite the config.rs tests that touched env**

In `crates/tau-plugins/openai/src/config.rs`:

- `resolve_api_key_uses_config_override` and `resolve_api_key_modern_sk_proj_prefix_accepted` use the direct `api_key` field — add a `|_| None` lookup argument to each `resolve_api_key(...)` call.
- `resolve_api_key_malformed_prefix_returns_invalid_value` likewise uses the direct field — add `|_| None`.
- Replace `resolve_api_key_reads_env_var` and `resolve_api_key_missing_env_returns_invalid_env_var` with:

```rust
    #[test]
    fn resolve_api_key_reads_env_var() {
        let cfg = OpenAIConfig {
            api_key_env: "ANY_NAME".into(),
            ..OpenAIConfig::default()
        };
        let key = resolve_api_key(&cfg, |_| Some("sk-fromenv".into())).unwrap();
        assert_eq!(key, "sk-fromenv");
    }

    #[test]
    fn resolve_api_key_missing_env_returns_invalid_env_var() {
        let cfg = OpenAIConfig {
            api_key_env: "DEFINITELY_NOT_SET_OPENAI_QXZ".into(),
            ..OpenAIConfig::default()
        };
        let err = resolve_api_key(&cfg, |_| None).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidEnvVar { ref name, .. }
                if name == "DEFINITELY_NOT_SET_OPENAI_QXZ"
        ));
    }
```

- [ ] **Step 4: Rewrite the plugin.rs tests that touched env**

In `crates/tau-plugins/openai/src/plugin.rs`, replace the three env-setting tests:

```rust
    #[test]
    fn from_config_valid_api_key_constructs_plugin() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-proj-test-key-12345".into()),
            ..OpenAIConfig::default()
        };
        let result = OpenAIPlugin::from_config(cfg);
        assert!(result.is_ok(), "from_config should succeed");
    }

    #[test]
    fn from_config_invalid_retry_max_attempts_zero_returns_invalid_value() {
        let mut cfg = OpenAIConfig {
            api_key: Some("sk-test".into()),
            ..OpenAIConfig::default()
        };
        cfg.retry.max_attempts = 0;
        let err = match OpenAIPlugin::from_config(cfg) {
            Ok(_) => panic!("expected ConfigError::InvalidValue"),
            Err(e) => e,
        };
        assert!(matches!(
            err,
            ConfigError::InvalidValue {
                field: "retry.max_attempts",
                ..
            }
        ));
    }

    #[test]
    fn name_returns_openai() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-test".into()),
            ..OpenAIConfig::default()
        };
        let plugin = OpenAIPlugin::from_config(cfg).unwrap();
        assert_eq!(plugin.name(), "openai");
    }
```

- [ ] **Step 5: Run the crate tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p openai`
Expected: PASS, 0 failures.

- [ ] **Step 6: Confirm no env mutation remains in this crate**

Run: `grep -rn 'set_var\|remove_var' crates/tau-plugins/openai/src`
Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add crates/tau-plugins/openai/src/config.rs crates/tau-plugins/openai/src/plugin.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "refactor(openai): inject env lookup into resolve_api_key

Removes std::env::set_var/remove_var from the config + plugin tests."
```

---

### Task 3: Ollama plugin

**Files:**
- Modify: `crates/tau-plugins/ollama/src/config.rs` (`resolve_bearer_token` + its tests)
- Modify: `crates/tau-plugins/ollama/src/plugin.rs` (`from_config` caller, if it calls the resolver)

- [ ] **Step 1: Change `resolve_bearer_token` signature + body**

In `crates/tau-plugins/ollama/src/config.rs`, replace the function (currently lines ~146-159):

```rust
pub(crate) fn resolve_bearer_token(
    cfg: &OllamaConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, ConfigError> {
    if let Some(direct) = cfg.bearer_token.as_ref() {
        tracing::warn!(
            target: "ollama_plugin::config",
            "config.bearer_token set directly — recommended only for tests",
        );
        return Ok(Some(direct.clone()));
    }
    match lookup(&cfg.bearer_token_env) {
        Some(v) if v.is_empty() => Ok(None),
        Some(v) => Ok(Some(v)),
        None => Ok(None),
    }
}
```

- [ ] **Step 2: Update the production caller**

Find the caller: `grep -rn 'resolve_bearer_token' crates/tau-plugins/ollama/src/plugin.rs`. At the call site in `from_config`, change `resolve_bearer_token(&cfg)?` to:

```rust
let bearer_token = resolve_bearer_token(&cfg, |n| std::env::var(n).ok())?;
```

(Match the exact existing binding name.)

- [ ] **Step 3: Rewrite the config.rs tests that touched env**

In `crates/tau-plugins/ollama/src/config.rs`:

- `resolve_bearer_token_uses_config_override` uses the direct field — add a `|_| None` lookup argument.
- `resolve_bearer_token_missing_env_returns_none` already relies on an unset var — change its call to pass `|_| None`.
- Replace `resolve_bearer_token_reads_env_var` and `resolve_bearer_token_empty_env_treated_as_none` with:

```rust
    #[test]
    fn resolve_bearer_token_reads_env_var() {
        let cfg = OllamaConfig {
            bearer_token_env: "ANY_NAME".into(),
            ..OllamaConfig::default()
        };
        let token = resolve_bearer_token(&cfg, |_| Some("envtoken123".into())).unwrap();
        assert_eq!(token.as_deref(), Some("envtoken123"));
    }

    #[test]
    fn resolve_bearer_token_empty_env_treated_as_none() {
        let cfg = OllamaConfig {
            bearer_token_env: "ANY_NAME".into(),
            ..OllamaConfig::default()
        };
        let token = resolve_bearer_token(&cfg, |_| Some(String::new())).unwrap();
        assert!(token.is_none());
    }
```

- [ ] **Step 4: Run the crate tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p ollama`
Expected: PASS, 0 failures.

- [ ] **Step 5: Confirm no env mutation remains in this crate**

Run: `grep -rn 'set_var\|remove_var' crates/tau-plugins/ollama/src`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/tau-plugins/ollama/src/config.rs crates/tau-plugins/ollama/src/plugin.rs
git -c user.name="Titouan Le Bocq" -c user.email="lebocq.tit@gmail.com" commit --no-verify -m "refactor(ollama): inject env lookup into resolve_bearer_token

Removes std::env::set_var/remove_var from the config tests."
```

---

### Task 4: Workspace verification sweep

**Files:** none (verification only)

- [ ] **Step 1: Confirm zero env mutation across all three crates' src**

Run: `grep -rn 'set_var\|remove_var' crates/tau-plugins/anthropic/src crates/tau-plugins/openai/src crates/tau-plugins/ollama/src`
Expected: no output.

- [ ] **Step 2: Run doctests for the three crates**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo test --doc -p anthropic -p openai -p ollama`
Expected: PASS (or "0 tests" — either is fine; must not fail to compile).

- [ ] **Step 3: clippy on the three crates**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo clippy -p anthropic -p openai -p ollama --all-targets`
Expected: no warnings/errors.

- [ ] **Step 4: rustfmt check**

Run: `timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo fmt -p anthropic -p openai -p ollama -- --check`
Expected: no diff. If it reports drift, run without `--check` and amend the relevant per-crate commit.

- [ ] **Step 5: Final full nextest of the three crates**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/main cargo nextest run -p anthropic -p openai -p ollama`
Expected: 188 passed (matches baseline), 0 failures.

---

## Self-Review

- **Spec coverage:** Task 1 = anthropic, Task 2 = openai, Task 3 = ollama (all three resolvers + callers + tests), Task 4 = the verification commands the spec lists. All spec sections covered.
- **Placeholder scan:** every code step shows full code; no TBD/TODO.
- **Type consistency:** all three resolvers use the same `lookup: impl Fn(&str) -> Option<String>` signature; production callers all pass `|n| std::env::var(n).ok()`; ollama preserves the empty→None / unset→None semantics from the spec.
