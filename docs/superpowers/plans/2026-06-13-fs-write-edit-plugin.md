# fs-write/edit Tool Plugin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an in-tree, capability-gated `fs-write` Tool plugin that mutates a single absolute path under the agent's `fs.write` scope, with two modes — full base64 `write` and `old_str`→`new_str` `edit` (with `replace_all` opt-in).

**Architecture:** A 1:1 mirror of `crates/tau-plugins/fs-read`: one crate, one `fs-write-plugin` binary driven by `tau_plugin_sdk::run_tool_with_config`, one `Tool` impl exposing tool name `fs-write`. Args are a serde-tagged discriminated union (`tag = "mode"`) parsed from `tau_domain::Value` via a `serde_json` round-trip; the JSON schema is the mirror of that enum. Path validation + glob admission are ported verbatim from `fs-read`. Errors follow `fs-read`'s two tiers: `ToolError::BadArgs` for static/scope faults (RPC reject), `ToolResult{is_error:true}` for filesystem outcomes (retryable). The one net-new behavior is `max_bytes` enforcement.

**Tech Stack:** Rust, tokio (`fs`), `tau-plugin-sdk`, `tau-ports`, `tau-domain`, `globset`, `base64`, `serde`/`serde_json`. Tests via `cargo nextest` + `tau_plugin_protocol::test_support::FakeStdioPeer`.

**Spec:** `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`

---

## CARGO command discipline (read once)

Every cargo command in this plan MUST follow `CLAUDE.md`: prefix with
`env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl`, scope with
`-p fs-write`, wrap with `timeout`. The plan's commands already include
this. If running as a subagent with a different role, substitute
`target/agent-<role>`.

## File Structure

All under `crates/tau-plugins/fs-write/` unless noted:

- `Cargo.toml` — package + `[[bin]] fs-write-plugin` + `[lib] fs_write_plugin_lib`. Mirror of fs-read's Cargo.toml.
- `tau.toml` — manifest: `provides="tool"`, `bin="fs-write-plugin"`, `fs.write` cap, `required_tier="strict"`.
- `Dockerfile`, `.dockerignore` — build the binary; mirror fs-read.
- `README.md` — usage, validation rules, error model.
- `src/lib.rs` — module declarations.
- `src/main.rs` — SDK runner shim.
- `src/config.rs` — empty `FsWriteConfig` (round-trip parity with SDK handshake).
- `src/path_check.rs` — `validate_path`, `admit`, `admit_with_deny`, `BadArgs` (verbatim port, strings re-prefixed `fs-write:`).
- `src/plugin.rs` — `WriteArgs`, `parse_args`, `apply_edit`/`EditOutcome`, `extract_fs_write_paths`, `extract_max_bytes`, `FsWriteSession`, `FsWritePlugin : Tool`.
- `tests/invoke.rs` — `FakeStdioPeer` integration tests.
- Root `Cargo.toml` — add `"crates/tau-plugins/fs-write"` to `members`.

---

## Task 1: Crate skeleton + config + path_check (compiles, unit tests green)

**Files:**
- Create: `crates/tau-plugins/fs-write/Cargo.toml`
- Create: `crates/tau-plugins/fs-write/src/lib.rs`
- Create: `crates/tau-plugins/fs-write/src/main.rs`
- Create: `crates/tau-plugins/fs-write/src/config.rs`
- Create: `crates/tau-plugins/fs-write/src/path_check.rs`
- Create: `crates/tau-plugins/fs-write/src/plugin.rs` (minimal stub)
- Create: `crates/tau-plugins/fs-write/tau.toml`
- Create: `crates/tau-plugins/fs-write/Dockerfile`
- Create: `crates/tau-plugins/fs-write/.dockerignore`
- Modify: root `Cargo.toml` (`members` array)

- [ ] **Step 1: Create `Cargo.toml`** (mirror fs-read, name swapped)

```toml
[package]
name = "fs-write"
description = "fs-write tool plugin: writes/edits a single absolute path under fs.write capability scope."
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true
repository.workspace   = true
authors.workspace      = true

[[bin]]
name = "fs-write-plugin"
path = "src/main.rs"

[lib]
name = "fs_write_plugin_lib"
path = "src/lib.rs"

[dependencies]
tau-domain          = { workspace = true, features = ["serde"] }
tau-ports           = { workspace = true, features = ["serde", "test-fixtures"] }
tau-plugin-protocol = { workspace = true }
tau-plugin-sdk      = { workspace = true }
serde               = { workspace = true }
serde_json          = "1"
thiserror           = { workspace = true }
tokio               = { workspace = true, features = ["macros", "rt", "rt-multi-thread", "fs"] }
tracing             = { workspace = true }
globset             = { workspace = true }
base64              = { workspace = true }

[dev-dependencies]
tempfile            = { workspace = true }
tau-domain          = { workspace = true, features = ["serde"] }
tau-ports           = { workspace = true, features = ["serde", "test-fixtures"] }
tau-plugin-protocol = { workspace = true, features = ["test-support"] }
tau-plugin-sdk      = { workspace = true }
rmp-serde           = { workspace = true }
uuid                = { workspace = true }
assert_matches      = { workspace = true }
tokio               = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 2: Add the crate to the workspace** — edit root `Cargo.toml`, add the line after the `fs-read` / `shell` entries in `members`:

```toml
    "crates/tau-plugins/fs-write",
```

- [ ] **Step 3: Create `src/lib.rs`**

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! `fs-write` Tool plugin internals.
//!
//! The binary entrypoint at `src/main.rs` calls
//! `tau_plugin_sdk::run_tool_with_config::<FsWritePlugin>(...)`.
//!
//! Write-side mirror of the `fs-read` plugin. See
//! `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`.

pub mod config;
pub(crate) mod path_check;
pub mod plugin;
```

- [ ] **Step 4: Create `src/main.rs`**

```rust
//! `fs-write-plugin` binary. Spawned by tau-runtime::plugin_host as a
//! subprocess; talks MessagePack-RPC over stdio per ADR-0008.
//!
//! Thin shim over [`tau_plugin_sdk::run_tool_with_config`].
//!
//! [`FsWritePlugin`]: fs_write_plugin_lib::plugin::FsWritePlugin

use fs_write_plugin_lib::plugin::FsWritePlugin;
use tau_plugin_sdk::{run_tool_with_config, SdkError};

#[tokio::main]
async fn main() -> Result<(), SdkError> {
    run_tool_with_config::<FsWritePlugin>(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).await
}
```

- [ ] **Step 5: Create `src/config.rs`** (port of fs-read config, renamed)

```rust
//! `fs-write` plugin configuration.
//!
//! v0.1 has no knobs; the empty config still goes through
//! `Configure::from_config` for round-trip consistency with the SDK
//! handshake.

use serde::Deserialize;

/// Top-level config for the fs-write plugin.
///
/// Reserved for future expansion. `#[non_exhaustive]` so additive
/// fields remain non-breaking.
///
/// # Example
///
/// ```ignore
/// use fs_write_plugin_lib::config::FsWriteConfig;
/// let cfg = FsWriteConfig::default();
/// let _ = cfg;
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsWriteConfig {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_empty() {
        let _cfg = FsWriteConfig::default();
    }

    #[test]
    fn deserializes_empty_object() {
        let cfg: FsWriteConfig = serde_json::from_str("{}").unwrap();
        let _ = cfg;
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<FsWriteConfig, _> = serde_json::from_str(r#"{"unknown":"x"}"#);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 6: Create `src/path_check.rs`** (verbatim port of fs-read's, strings re-prefixed `fs-write:`)

```rust
//! Path validation + glob admission for `fs-write`.
//!
//! Ported verbatim from the `fs-read` plugin (reason strings
//! re-prefixed). See
//! `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`.

/// Reasons a path is rejected at validation time.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BadArgs {
    /// The path string was empty.
    Empty,
    /// The path contained a NUL byte.
    NullByte,
    /// The path was relative; absolute paths required.
    NotAbsolute,
    /// The path contained a `..` segment.
    Traversal,
    /// The path was outside the agent's fs.write capability scope.
    NotInScope,
}

impl BadArgs {
    /// Human-readable reason string surfaced in `ToolError::BadArgs`.
    pub(crate) fn reason(&self) -> String {
        match self {
            BadArgs::Empty => "fs-write: path is empty".into(),
            BadArgs::NullByte => "fs-write: path contains a NUL byte".into(),
            BadArgs::NotAbsolute => "fs-write: path is not absolute".into(),
            BadArgs::Traversal => "fs-write: path contains a `..` segment".into(),
            BadArgs::NotInScope => "fs-write: path is not in capability scope".into(),
        }
    }
}

/// Validate the syntactic shape of a path. Returns the path on
/// success, or a [`BadArgs`] reason on failure.
pub(crate) fn validate_path(path: &str) -> Result<&str, BadArgs> {
    if path.is_empty() {
        return Err(BadArgs::Empty);
    }
    if path.bytes().any(|b| b == 0) {
        return Err(BadArgs::NullByte);
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(BadArgs::NotAbsolute);
    }
    if path.split(std::path::MAIN_SEPARATOR).any(|seg| seg == "..") {
        return Err(BadArgs::Traversal);
    }
    Ok(path)
}

/// Check whether `path` is admissible under the active glob list.
/// Returns true iff at least one glob matches.
pub(crate) fn admit(path: &str, allowed_globs: &[String]) -> bool {
    use globset::Glob;
    allowed_globs.iter().any(|g| {
        Glob::new(g)
            .ok()
            .map(|gl| gl.compile_matcher().is_match(path))
            .unwrap_or(false)
    })
}

/// Check `path` is admitted by the allow-list AND not denied. Deny
/// wins. Reuses [`admit`] for both checks.
pub(crate) fn admit_with_deny(path: &str, allow: &[String], deny: &[String]) -> bool {
    if !admit(path, allow) {
        return false;
    }
    !admit(path, deny)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_empty_rejected() {
        assert_eq!(validate_path(""), Err(BadArgs::Empty));
    }

    #[test]
    fn validate_path_null_byte_rejected() {
        assert_eq!(validate_path("/tmp/foo\0bar"), Err(BadArgs::NullByte));
    }

    #[test]
    fn validate_path_relative_rejected() {
        assert_eq!(validate_path("./foo"), Err(BadArgs::NotAbsolute));
        assert_eq!(validate_path("foo/bar"), Err(BadArgs::NotAbsolute));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_traversal_rejected_dotdot_segment() {
        assert_eq!(validate_path("/../etc/passwd"), Err(BadArgs::Traversal));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_traversal_rejected_in_middle() {
        assert_eq!(validate_path("/tmp/../etc/passwd"), Err(BadArgs::Traversal));
    }

    #[cfg(unix)]
    #[test]
    fn validate_path_happy_path_returns_path() {
        assert_eq!(validate_path("/tmp/foo.txt"), Ok("/tmp/foo.txt"));
    }

    #[test]
    fn admit_matches_simple_glob() {
        let globs = vec!["/tmp/**".to_string()];
        assert!(admit("/tmp/foo.txt", &globs));
        assert!(admit("/tmp/sub/bar.txt", &globs));
    }

    #[test]
    fn admit_does_not_match_outside_scope() {
        let globs = vec!["/var/**".to_string()];
        assert!(!admit("/tmp/foo.txt", &globs));
    }

    #[test]
    fn admit_returns_false_for_invalid_glob() {
        let globs = vec!["[unclosed".to_string()];
        assert!(!admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_empty_glob_list_returns_false() {
        let globs: Vec<String> = vec![];
        assert!(!admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_multiple_globs_first_match_wins() {
        let globs = vec!["/var/**".to_string(), "/tmp/**".to_string()];
        assert!(admit("/tmp/foo", &globs));
    }

    #[test]
    fn admit_with_deny_denies_when_deny_matches() {
        let allow = vec!["/proj/**".to_string()];
        let deny = vec!["/proj/secrets/**".to_string()];
        assert!(!admit_with_deny("/proj/secrets/api.key", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_admits_when_no_deny_matches() {
        let allow = vec!["/proj/**".to_string()];
        let deny = vec!["/proj/secrets/**".to_string()];
        assert!(admit_with_deny("/proj/src/main.rs", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_denies_when_allow_misses() {
        let allow = vec!["/proj/**".to_string()];
        let deny: Vec<String> = vec![];
        assert!(!admit_with_deny("/etc/passwd", &allow, &deny));
    }

    #[test]
    fn admit_with_deny_empty_deny_falls_through_to_allow() {
        let allow = vec!["/proj/**".to_string()];
        let deny: Vec<String> = vec![];
        assert!(admit_with_deny("/proj/foo", &allow, &deny));
    }
}
```

- [ ] **Step 7: Create `src/plugin.rs` minimal stub** (compiles so the bin links; fleshed out in Tasks 2–5)

```rust
//! `FsWritePlugin` — Tool impl for the fs-write plugin.
//!
//! Mutates a single absolute path under the agent's `fs.write`
//! capability scope. Two modes: `write` (full base64 contents) and
//! `edit` (`old_str`→`new_str`).

use std::sync::OnceLock;
use tau_domain::{Capability, Value};
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{
    fixtures::{make_tool_result, make_tool_spec},
    SessionContext, Tool, ToolContent, ToolError, ToolResult, ToolSpec,
};

use crate::config::FsWriteConfig;

/// Per-session state derived from the agent's granted capabilities.
pub struct FsWriteSession {
    #[allow(dead_code)]
    allowed_globs: Vec<String>,
    #[allow(dead_code)]
    denied_globs: Vec<String>,
    #[allow(dead_code)]
    max_bytes: Option<u64>,
}

/// fs-write Tool plugin.
pub struct FsWritePlugin {
    #[allow(dead_code)]
    config: FsWriteConfig,
}

impl Configure for FsWritePlugin {
    type Config = FsWriteConfig;

    fn from_config(config: Self::Config) -> Result<Self, ConfigError> {
        Ok(FsWritePlugin { config })
    }
}

impl Tool for FsWritePlugin {
    type Session = FsWriteSession;

    fn name(&self) -> &str {
        "fs-write"
    }

    fn schema(&self) -> ToolSpec {
        // Real schema lands in Task 5.
        make_tool_spec(
            "fs-write".to_string(),
            "Write or edit a file at an absolute path.".to_string(),
            Value::Object(std::collections::BTreeMap::new()),
        )
    }

    fn capabilities(&self) -> &[Capability] {
        static CAPS: OnceLock<Vec<Capability>> = OnceLock::new();
        CAPS.get_or_init(|| {
            let cap: Capability = serde_json::from_str(r#"{"kind":"fs.write","paths":[]}"#)
                .expect("static fs.write capability JSON is valid");
            vec![cap]
        })
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        Ok(FsWriteSession {
            allowed_globs: Vec::new(),
            denied_globs: Vec::new(),
            max_bytes: None,
        })
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        _args: Value,
    ) -> Result<ToolResult, ToolError> {
        Ok(make_tool_result(
            vec![ToolContent::Text {
                text: "fs-write: unimplemented".to_string(),
            }],
            true,
        ))
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}
```

- [ ] **Step 8: Create `tau.toml`**

```toml
name = "fs-write"
version = "0.1.0"
description = "Write or edit a single absolute path under fs.write capability scope."

[plugin]
provides = "tool"
kind     = "rust-cargo"
bin      = "fs-write-plugin"

[[capabilities]]
kind = "fs.write"
paths = []

[sandbox]
required_tier = "strict"
```

- [ ] **Step 9: Create `Dockerfile`** (mirror fs-read, binary renamed)

```dockerfile
# syntax=docker/dockerfile:1.6

# ---------- Builder stage: compile fs-write-plugin ----------
FROM rust:1-bookworm AS builder

WORKDIR /workspace

COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p fs-write --bin fs-write-plugin

# ---------- Runtime stage ----------
FROM tau-plugin-base:dev

COPY --from=builder /workspace/target/release/fs-write-plugin /usr/local/bin/fs-write-plugin

USER tau

ENTRYPOINT ["/usr/local/bin/fs-write-plugin"]
```

- [ ] **Step 10: Create `.dockerignore`**

```
target/
**/target/
.git/
.github/
docs/
*.md
```

- [ ] **Step 11: Build + run unit tests — verify the skeleton compiles and config/path_check tests pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write`
Expected: PASS — config (3 tests) + path_check (14 tests) green; plugin stub compiles.

- [ ] **Step 12: Commit**

```bash
git add crates/tau-plugins/fs-write Cargo.toml
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(fs-write): scaffold crate, config, path_check (port from fs-read)"
```

---

## Task 2: `WriteArgs` discriminated union + `parse_args`

**Files:**
- Modify: `crates/tau-plugins/fs-write/src/plugin.rs`

- [ ] **Step 1: Write failing tests** — append to the `#[cfg(test)] mod tests` block in `plugin.rs` (create the block if it does not yet exist):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tau `Value` from a JSON literal for arg-parsing tests.
    fn val(json: serde_json::Value) -> Value {
        serde_json::from_value(json).expect("json to tau Value")
    }

    #[test]
    fn parse_write_variant() {
        let args = val(serde_json::json!({
            "mode": "write", "path": "/p/a", "contents": "aGk="
        }));
        let parsed = parse_args(&args).expect("write parses");
        assert_matches::assert_matches!(
            parsed,
            WriteArgs::Write { path, contents }
                if path == "/p/a" && contents == "aGk="
        );
    }

    #[test]
    fn parse_edit_variant_defaults_replace_all_false() {
        let args = val(serde_json::json!({
            "mode": "edit", "path": "/p/a", "old_str": "x", "new_str": "y"
        }));
        let parsed = parse_args(&args).expect("edit parses");
        assert_matches::assert_matches!(
            parsed,
            WriteArgs::Edit { replace_all: false, .. }
        );
    }

    #[test]
    fn parse_edit_variant_replace_all_true() {
        let args = val(serde_json::json!({
            "mode": "edit", "path": "/p/a", "old_str": "x", "new_str": "y",
            "replace_all": true
        }));
        let parsed = parse_args(&args).expect("edit parses");
        assert_matches::assert_matches!(parsed, WriteArgs::Edit { replace_all: true, .. });
    }

    #[test]
    fn parse_rejects_cross_mode_field() {
        // old_str is not legal in write mode (deny_unknown_fields).
        let args = val(serde_json::json!({
            "mode": "write", "path": "/p/a", "contents": "aGk=", "old_str": "x"
        }));
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn parse_rejects_unknown_mode() {
        let args = val(serde_json::json!({
            "mode": "append", "path": "/p/a", "contents": "aGk="
        }));
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn parse_rejects_missing_mode() {
        let args = val(serde_json::json!({ "path": "/p/a", "contents": "aGk=" }));
        assert!(parse_args(&args).is_err());
    }
}
```

- [ ] **Step 2: Run tests — verify they fail to compile (`WriteArgs`/`parse_args` undefined)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write parse_`
Expected: FAIL — `cannot find type WriteArgs` / `cannot find function parse_args`.

- [ ] **Step 3: Add `WriteArgs` + `parse_args`** — add near the top of `plugin.rs` (after the `use` block), and add `use serde::Deserialize;` to the imports:

```rust
/// Tool arguments, discriminated on `mode`. The single source of
/// truth that the JSON schema in [`FsWritePlugin::schema`] mirrors.
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum WriteArgs {
    /// Full create-or-truncate write of base64-decoded `contents`.
    Write { path: String, contents: String },
    /// Replace `old_str` with `new_str` in an existing file.
    Edit {
        path: String,
        old_str: String,
        new_str: String,
        #[serde(default)]
        replace_all: bool,
    },
}

/// Parse `args` (a `tau_domain::Value`) into [`WriteArgs`] via a
/// `serde_json` round-trip. Shape violations become `BadArgs`.
fn parse_args(args: &Value) -> Result<WriteArgs, ToolError> {
    let json = serde_json::to_value(args).map_err(|e| ToolError::BadArgs {
        reason: format!("fs-write: cannot read args: {e}"),
    })?;
    serde_json::from_value::<WriteArgs>(json).map_err(|e| ToolError::BadArgs {
        reason: format!("fs-write: {e}"),
    })
}
```

- [ ] **Step 4: Run tests — verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write parse_`
Expected: PASS — 6 parse tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-plugins/fs-write/src/plugin.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(fs-write): WriteArgs discriminated union + parse_args"
```

---

## Task 3: Capability extraction (`extract_fs_write_paths`, `extract_max_bytes`)

**Files:**
- Modify: `crates/tau-plugins/fs-write/src/plugin.rs`

- [ ] **Step 1: Write failing tests** — append to `mod tests` in `plugin.rs`:

```rust
    /// Deserialize a `Capability` from JSON (FsCapability is `#[non_exhaustive]`).
    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("test capability JSON must be valid")
    }

    #[test]
    fn extract_paths_collects_from_multiple_write_grants() {
        let granted = vec![
            cap(r#"{"kind":"fs.write","paths":["/tmp/**"]}"#),
            cap(r#"{"kind":"fs.write","paths":["/var/log/**","/etc/**"]}"#),
            cap(r#"{"kind":"fs.read","paths":["/should/be/ignored/**"]}"#),
        ];
        assert_eq!(
            extract_fs_write_paths(&granted),
            vec![
                "/tmp/**".to_string(),
                "/var/log/**".to_string(),
                "/etc/**".to_string()
            ]
        );
    }

    #[test]
    fn extract_paths_empty_when_no_write_grants() {
        assert!(extract_fs_write_paths(&[]).is_empty());
    }

    #[test]
    fn extract_max_bytes_none_when_no_grants() {
        assert_eq!(extract_max_bytes(&[]), None);
    }

    #[test]
    fn extract_max_bytes_uncapped_grant_wins() {
        // One grant has a cap, one is uncapped → uncapped (None) wins.
        let granted = vec![
            cap(r#"{"kind":"fs.write","paths":["/a/**"],"max_bytes":100}"#),
            cap(r#"{"kind":"fs.write","paths":["/b/**"]}"#),
        ];
        assert_eq!(extract_max_bytes(&granted), None);
    }

    #[test]
    fn extract_max_bytes_takes_max_of_present_caps() {
        let granted = vec![
            cap(r#"{"kind":"fs.write","paths":["/a/**"],"max_bytes":100}"#),
            cap(r#"{"kind":"fs.write","paths":["/b/**"],"max_bytes":4096}"#),
        ];
        assert_eq!(extract_max_bytes(&granted), Some(4096));
    }
```

- [ ] **Step 2: Run — verify fail (undefined functions)**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write extract_`
Expected: FAIL — `cannot find function extract_fs_write_paths` / `extract_max_bytes`.

- [ ] **Step 3: Implement** — add to `plugin.rs` (free functions, after `parse_args`); add `FsCapability` to the `tau_domain` import:

```rust
fn extract_fs_write_paths(granted: &[Capability]) -> Vec<String> {
    granted
        .iter()
        .filter_map(|c| match c {
            Capability::Filesystem(FsCapability::Write { paths, .. }) => Some(paths.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Most-permissive `max_bytes` across all `fs.write` grants: `None`
/// (uncapped) if any grant is uncapped, else the maximum present cap.
/// `None` when there are no `fs.write` grants (the kernel gates
/// presence; an empty allow-list then rejects every path anyway).
fn extract_max_bytes(granted: &[Capability]) -> Option<u64> {
    let caps: Vec<Option<u64>> = granted
        .iter()
        .filter_map(|c| match c {
            Capability::Filesystem(FsCapability::Write { max_bytes, .. }) => Some(*max_bytes),
            _ => None,
        })
        .collect();
    if caps.is_empty() || caps.iter().any(Option::is_none) {
        return None;
    }
    caps.into_iter().flatten().max()
}
```

- [ ] **Step 4: Run — verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write extract_`
Expected: PASS — 5 extraction tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-plugins/fs-write/src/plugin.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(fs-write): capability path + max_bytes extraction"
```

---

## Task 4: `apply_edit` helper + `EditOutcome`

**Files:**
- Modify: `crates/tau-plugins/fs-write/src/plugin.rs`

- [ ] **Step 1: Write failing tests** — append to `mod tests`:

```rust
    #[test]
    fn apply_edit_single_match_replaces() {
        let out = apply_edit("hello world", "world", "tau", false);
        assert_matches::assert_matches!(out, EditOutcome::Replaced(s) if s == "hello tau");
    }

    #[test]
    fn apply_edit_zero_matches_not_found() {
        let out = apply_edit("hello world", "zzz", "q", false);
        assert_matches::assert_matches!(out, EditOutcome::NotFound);
    }

    #[test]
    fn apply_edit_multi_match_ambiguous_when_not_replace_all() {
        let out = apply_edit("a x a x a", "a", "b", false);
        assert_matches::assert_matches!(out, EditOutcome::Ambiguous(3));
    }

    #[test]
    fn apply_edit_multi_match_replace_all() {
        let out = apply_edit("a x a x a", "a", "b", true);
        assert_matches::assert_matches!(out, EditOutcome::Replaced(s) if s == "b x b x b");
    }

    #[test]
    fn apply_edit_new_str_empty_deletes() {
        let out = apply_edit("keep DROP keep", " DROP", "", false);
        assert_matches::assert_matches!(out, EditOutcome::Replaced(s) if s == "keep keep");
    }
```

- [ ] **Step 2: Run — verify fail**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write apply_edit`
Expected: FAIL — `cannot find type EditOutcome` / `function apply_edit`.

- [ ] **Step 3: Implement** — add to `plugin.rs` after the extraction functions:

```rust
/// Result of applying an `edit` to a file's text.
#[derive(Debug)]
enum EditOutcome {
    /// Replacement succeeded; carries the new file content.
    Replaced(String),
    /// `old_str` did not occur in the file.
    NotFound,
    /// `old_str` occurred N>=2 times and `replace_all` was false.
    Ambiguous(usize),
}

/// Apply an `old_str`→`new_str` edit. Caller guarantees `old` is
/// non-empty. `str::matches`, `replacen`, and `replace` all count
/// non-overlapping occurrences left-to-right, so the count and the
/// replacement stay consistent.
fn apply_edit(haystack: &str, old: &str, new: &str, replace_all: bool) -> EditOutcome {
    match haystack.matches(old).count() {
        0 => EditOutcome::NotFound,
        1 => EditOutcome::Replaced(haystack.replacen(old, new, 1)),
        n if replace_all => {
            let _ = n;
            EditOutcome::Replaced(haystack.replace(old, new))
        }
        n => EditOutcome::Ambiguous(n),
    }
}
```

- [ ] **Step 4: Run — verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write apply_edit`
Expected: PASS — 5 edit-logic tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/tau-plugins/fs-write/src/plugin.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(fs-write): apply_edit helper with exactly-once + replace_all"
```

---

## Task 5: Wire `schema`, `init`, and the full `invoke` (write + edit + max_bytes)

**Files:**
- Modify: `crates/tau-plugins/fs-write/src/plugin.rs`

- [ ] **Step 1: Replace the `schema` stub with the real discriminated `oneOf`** — replace the `schema` method body:

```rust
    fn schema(&self) -> ToolSpec {
        let schema_json = json!({
            "type": "object",
            "oneOf": [
                {
                    "title": "write",
                    "properties": {
                        "path": { "type": "string",
                            "description": "Absolute path. No `..` segments. Created or truncated." },
                        "mode": { "const": "write" },
                        "contents": { "type": "string",
                            "description": "Base64-encoded file bytes." }
                    },
                    "required": ["path", "mode", "contents"],
                    "additionalProperties": false
                },
                {
                    "title": "edit",
                    "properties": {
                        "path": { "type": "string",
                            "description": "Absolute path. No `..` segments. File must already exist." },
                        "mode": { "const": "edit" },
                        "old_str": { "type": "string",
                            "description": "Exact substring to replace. Non-empty." },
                        "new_str": { "type": "string",
                            "description": "Replacement text. May be empty to delete." },
                        "replace_all": { "type": "boolean", "default": false,
                            "description": "Replace every occurrence. Default false requires old_str to match exactly once." }
                    },
                    "required": ["path", "mode", "old_str", "new_str"],
                    "additionalProperties": false
                }
            ]
        });
        let schema_value: Value = serde_json::from_str(
            &serde_json::to_string(&schema_json).expect("static JSON schema serializes"),
        )
        .expect("static JSON schema round-trips through tau_domain::Value");
        make_tool_spec(
            "fs-write".to_string(),
            "Write (full base64 contents) or edit (old_str->new_str) a file at an absolute path."
                .to_string(),
            schema_value,
        )
    }
```

- [ ] **Step 2: Replace the `init` stub** — populate session from `ctx`:

```rust
    async fn init(&self, ctx: SessionContext) -> Result<Self::Session, ToolError> {
        let allowed_globs = extract_fs_write_paths(&ctx.granted_capabilities);
        let denied_globs = ctx
            .deny_entries
            .iter()
            .find(|e| e.kind == "fs.write")
            .map(|e| e.deny.clone())
            .unwrap_or_default();
        let max_bytes = extract_max_bytes(&ctx.granted_capabilities);
        Ok(FsWriteSession {
            allowed_globs,
            denied_globs,
            max_bytes,
        })
    }
```

- [ ] **Step 3: Replace the `invoke` stub with the full implementation:**

```rust
    async fn invoke(
        &self,
        session: &mut Self::Session,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        match parse_args(&args)? {
            WriteArgs::Write { path, contents } => {
                let path =
                    validate_path(&path).map_err(|e| ToolError::BadArgs { reason: e.reason() })?;
                if !admit_with_deny(path, &session.allowed_globs, &session.denied_globs) {
                    return Err(ToolError::BadArgs {
                        reason: BadArgs::NotInScope.reason(),
                    });
                }
                // base64 decode failure is a Tier ② (retryable) outcome.
                let bytes = match base64::engine::general_purpose::STANDARD.decode(&contents) {
                    Ok(b) => b,
                    Err(e) => return Ok(semantic_error(format!("fs-write: invalid base64: {e}"))),
                };
                if let Some(cap) = session.max_bytes {
                    if bytes.len() as u64 > cap {
                        return Err(ToolError::BadArgs {
                            reason: format!(
                                "fs-write: write of {} bytes exceeds max_bytes cap of {cap}",
                                bytes.len()
                            ),
                        });
                    }
                }
                match tokio::fs::write(path, &bytes).await {
                    Ok(()) => Ok(wrote_result(path, bytes.len() as i64)),
                    Err(io_err) => Ok(semantic_error(format!("fs-write: {io_err}"))),
                }
            }
            WriteArgs::Edit {
                path,
                old_str,
                new_str,
                replace_all,
            } => {
                let path =
                    validate_path(&path).map_err(|e| ToolError::BadArgs { reason: e.reason() })?;
                if !admit_with_deny(path, &session.allowed_globs, &session.denied_globs) {
                    return Err(ToolError::BadArgs {
                        reason: BadArgs::NotInScope.reason(),
                    });
                }
                if old_str.is_empty() {
                    return Err(ToolError::BadArgs {
                        reason: "fs-write: old_str must not be empty".to_string(),
                    });
                }
                // Edit requires an existing, UTF-8 file; both failures
                // are Tier ② (retryable) outcomes.
                let current = match tokio::fs::read_to_string(path).await {
                    Ok(s) => s,
                    Err(io_err) => return Ok(semantic_error(format!("fs-write: {io_err}"))),
                };
                let new_content = match apply_edit(&current, &old_str, &new_str, replace_all) {
                    EditOutcome::Replaced(s) => s,
                    EditOutcome::NotFound => {
                        return Ok(semantic_error(format!(
                            "fs-write: old_str not found in {path}"
                        )))
                    }
                    EditOutcome::Ambiguous(n) => {
                        return Ok(semantic_error(format!(
                            "fs-write: old_str matched {n} times; add context to disambiguate or set replace_all"
                        )))
                    }
                };
                if let Some(cap) = session.max_bytes {
                    if new_content.len() as u64 > cap {
                        return Err(ToolError::BadArgs {
                            reason: format!(
                                "fs-write: edit result of {} bytes exceeds max_bytes cap of {cap}",
                                new_content.len()
                            ),
                        });
                    }
                }
                match tokio::fs::write(path, new_content.as_bytes()).await {
                    Ok(()) => Ok(wrote_result(path, new_content.len() as i64)),
                    Err(io_err) => Ok(semantic_error(format!("fs-write: {io_err}"))),
                }
            }
        }
    }
```

- [ ] **Step 4: Add the two result-builder helpers** — add as free functions in `plugin.rs` (after `apply_edit`); add `use base64::Engine as _;`, `use serde_json::json;`, and `use std::collections::BTreeMap;` to imports:

```rust
/// Build the success `ToolResult` for a write/edit: `{bytes_written, path}`.
fn wrote_result(path: &str, bytes_written: i64) -> ToolResult {
    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    map.insert("bytes_written".into(), Value::Integer(bytes_written));
    map.insert("path".into(), Value::String(path.to_string()));
    make_tool_result(
        vec![ToolContent::Json {
            data: Value::Object(map),
        }],
        false,
    )
}

/// Build a Tier ② semantic error (`is_error: true`) the LLM may retry.
fn semantic_error(text: String) -> ToolResult {
    make_tool_result(vec![ToolContent::Text { text }], true)
}
```

- [ ] **Step 5: Remove the now-stale `#[allow(dead_code)]` attributes** on `FsWriteSession` fields (they are now read by `invoke`/`init`). The struct becomes:

```rust
/// Per-session state derived from the agent's granted capabilities.
pub struct FsWriteSession {
    /// Glob patterns from `FsCapability::Write.paths` (flattened).
    allowed_globs: Vec<String>,
    /// Globs to subtract, from `deny_entries["fs.write"]`. Deny wins.
    denied_globs: Vec<String>,
    /// Most-permissive `max_bytes` across grants; `None` = uncapped.
    max_bytes: Option<u64>,
}
```

- [ ] **Step 6: Build + run all unit tests**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write`
Expected: PASS — all unit tests (config, path_check, parse, extract, apply_edit) green; no `dead_code` warnings.

- [ ] **Step 7: Clippy clean**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p fs-write --all-targets -- -D warnings`
Expected: PASS — no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/tau-plugins/fs-write/src/plugin.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "feat(fs-write): wire schema, init, and full write/edit invoke with max_bytes"
```

---

## Task 6: Integration tests via `FakeStdioPeer`

**Files:**
- Create: `crates/tau-plugins/fs-write/tests/invoke.rs`

- [ ] **Step 1: Write the integration test file** (harness ported from fs-read's `tests/invoke.rs`, capability helper switched to `fs.write`, plus write/edit/max_bytes cases):

```rust
//! Integration tests: FsWritePlugin driven via FakeStdioPeer.
//!
//! Mirrors crates/tau-plugins/fs-read/tests/invoke.rs.

use base64::Engine as _;
use fs_write_plugin_lib::plugin::FsWritePlugin;
use std::time::SystemTime;
use tau_domain::{AgentInstanceId, Capability, PortKind, Value};
use tau_plugin_protocol::{
    handshake::{meta, HandshakeRequest, TraceContext},
    test_support::FakeStdioPeer,
    Frame, PROTOCOL_VERSION,
};
use tau_plugin_sdk::{run_tool_with_io, Configure};
use tau_ports::{DenyEntry, SessionContext};
use uuid::Uuid;

// ---- helpers ----

/// Build an `fs.write` capability via JSON (FsCapability is `#[non_exhaustive]`).
fn fs_write_cap(paths: &[&str], max_bytes: Option<u64>) -> Capability {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        cap: Capability,
    }
    let paths_json: Vec<serde_json::Value> = paths
        .iter()
        .map(|p| serde_json::Value::String((*p).to_string()))
        .collect();
    let mut cap_obj = serde_json::json!({ "kind": "fs.write", "paths": paths_json });
    if let Some(mb) = max_bytes {
        cap_obj
            .as_object_mut()
            .unwrap()
            .insert("max_bytes".to_string(), serde_json::json!(mb));
    }
    let json = serde_json::json!({ "cap": cap_obj });
    serde_json::from_value::<Wrapper>(json)
        .expect("test fs.write capability must parse")
        .cap
}

async fn do_handshake(peer: &mut FakeStdioPeer) {
    let req = HandshakeRequest::new(
        PROTOCOL_VERSION.to_string(),
        PortKind::Tool,
        TraceContext::new("r".into(), "a".into(), "s".into()),
        serde_json::Value::Null,
    );
    let params_bytes = rmp_serde::to_vec(&vec![&req]).unwrap();
    peer.writer
        .write_frame(
            &Frame::Request {
                id: 1,
                method: meta::HANDSHAKE_METHOD.to_string(),
                params: params_bytes,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    let _ = peer.reader.next_frame().await.unwrap().unwrap();
}

async fn send_tool_call(
    peer: &mut FakeStdioPeer,
    id: u32,
    ctx: &SessionContext,
    args: serde_json::Value,
) {
    let args_value: Value = serde_json::from_value(args).expect("args round-trip to tau Value");
    let params_bytes = rmp_serde::to_vec(&(ctx, &args_value)).unwrap();
    peer.writer
        .write_frame(
            &Frame::Request {
                id,
                method: "tool.call".to_string(),
                params: params_bytes,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn recv_tool_response(peer: &mut FakeStdioPeer) -> Result<tau_ports::ToolResult, String> {
    let body = peer.reader.next_frame().await.unwrap().unwrap();
    let frame = Frame::decode(&body).map_err(|e| format!("frame decode: {e}"))?;
    match frame {
        Frame::Response {
            result: Some(bytes),
            error: None,
            ..
        } => {
            let result: tau_ports::ToolResult =
                rmp_serde::from_slice(&bytes).map_err(|e| format!("rmp decode ToolResult: {e}"))?;
            Ok(result)
        }
        Frame::Response {
            error: Some(env),
            result: None,
            ..
        } => Err(format!("rpc error code={} msg={}", env.code, env.message)),
        other => Err(format!("unexpected frame: {other:?}")),
    }
}

async fn shutdown(peer: &mut FakeStdioPeer) {
    peer.writer
        .write_frame(
            &Frame::Notification {
                method: meta::SHUTDOWN_METHOD.to_string(),
                params: rmp_serde::to_vec::<Vec<()>>(&Vec::new()).unwrap(),
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
}

fn make_ctx(grants: Vec<Capability>) -> SessionContext {
    SessionContext::new(
        AgentInstanceId::new(),
        Uuid::now_v7(),
        Some(SystemTime::UNIX_EPOCH),
    )
    .with_granted_capabilities(grants)
}

/// Spawn the plugin runner over a fresh FakeStdioPeer.
fn spawn_plugin() -> (
    FakeStdioPeer,
    tokio::task::JoinHandle<Result<(), tau_plugin_sdk::SdkError>>,
) {
    let (peer, mut sut_reader, mut sut_writer) = FakeStdioPeer::new();
    let plugin = FsWritePlugin::from_config(Default::default()).unwrap();
    let runner = tokio::spawn(async move {
        run_tool_with_io(&mut sut_reader, &mut sut_writer, plugin, "fs-write", "0.1.0").await
    });
    (peer, runner)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn int_field(result: &tau_ports::ToolResult, key: &str) -> i64 {
    let tau_ports::ToolContent::Json { data } = &result.content[0] else {
        panic!("expected Json content, got {result:?}")
    };
    data.as_object()
        .and_then(|m| m.get(key))
        .and_then(Value::as_integer)
        .unwrap_or_else(|| panic!("missing integer field {key} in {result:?}"))
}

// ---- write mode ----

#[tokio::test]
async fn integration_write_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.txt");
    let path_str = path.to_str().unwrap().to_string();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    let payload = b"hello tau\n";
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(payload) }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");

    assert!(!result.is_error, "expected success; got {result:?}");
    assert_eq!(int_field(&result, "bytes_written"), payload.len() as i64);
    assert_eq!(std::fs::read(&path).unwrap(), payload);

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_write_out_of_scope_bad_args() {
    let dir = tempfile::tempdir().unwrap();
    let path_str = dir.path().join("out.txt").to_str().unwrap().to_string();

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&["/var/nope/**"], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"x") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("not in capability scope"), "got: {err}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_write_over_max_bytes_bad_args() {
    let dir = tempfile::tempdir().unwrap();
    let path_str = dir.path().join("out.txt").to_str().unwrap().to_string();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], Some(4))]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"too many bytes") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("max_bytes"), "got: {err}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

// ---- edit mode ----

#[tokio::test]
async fn integration_edit_single_match_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str,
            "old_str": "fn main() {}", "new_str": "fn main() { run(); }"
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");

    assert!(!result.is_error, "got {result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn main() { run(); }\n");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_edit_not_found_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "alpha\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str, "old_str": "zzz", "new_str": "q"
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(result.is_error, "expected is_error; got {result:?}");

    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[tokio::test]
async fn integration_edit_ambiguous_is_error_then_replace_all_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, "a\na\n").unwrap();
    let glob = format!("{}/**", dir.path().to_str().unwrap());

    // First: ambiguous (2 matches, replace_all default false) → is_error.
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "edit", "path": path_str, "old_str": "a", "new_str": "b" }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(result.is_error, "expected ambiguity is_error; got {result:?}");
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\na\n", "file untouched");

    // Then: replace_all true → all replaced, success.
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&[&glob], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({
            "mode": "edit", "path": path_str,
            "old_str": "a", "new_str": "b", "replace_all": true
        }),
    )
    .await;
    let result = recv_tool_response(&mut peer).await.expect("Ok response");
    assert!(!result.is_error, "got {result:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "b\nb\n");
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

// ---- shared validation (mirrors fs-read) ----

#[cfg(unix)]
#[tokio::test]
async fn integration_traversal_rejected() {
    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = make_ctx(vec![fs_write_cap(&["/**"], None)]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": "/tmp/../etc/x", "contents": "" }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(
        err.contains("`..` segment") || err.contains("traversal"),
        "got: {err}"
    );
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}

#[cfg(unix)]
#[tokio::test]
async fn integration_deny_overrides_allow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.txt");
    let path_str = path.to_str().unwrap().to_string();
    std::fs::write(&path, b"old").unwrap();
    let allow_glob = format!("{}/**", dir.path().to_str().unwrap());

    let (mut peer, runner) = spawn_plugin();
    do_handshake(&mut peer).await;
    let ctx = SessionContext::new(
        AgentInstanceId::new(),
        Uuid::now_v7(),
        Some(SystemTime::UNIX_EPOCH),
    )
    .with_granted_capabilities(vec![fs_write_cap(&[&allow_glob], None)])
    .with_deny_entries(vec![DenyEntry::new(
        "fs.write".into(),
        vec![path_str.clone()],
    )]);
    send_tool_call(
        &mut peer,
        2,
        &ctx,
        serde_json::json!({ "mode": "write", "path": path_str, "contents": b64(b"new") }),
    )
    .await;
    let err = recv_tool_response(&mut peer).await.expect_err("RPC error");
    assert!(err.contains("not in capability scope"), "got: {err}");
    shutdown(&mut peer).await;
    drop(peer);
    let _ = runner.await;
}
```

- [ ] **Step 2: Run integration tests — verify pass**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write --test invoke`
Expected: PASS — 8 integration tests green (2 are `#[cfg(unix)]`).

- [ ] **Step 3: Run the full crate test suite**

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo nextest run -p fs-write`
Expected: PASS — all unit + integration tests green.

- [ ] **Step 4: Commit**

```bash
git add crates/tau-plugins/fs-write/tests/invoke.rs
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "test(fs-write): FakeStdioPeer integration tests (write/edit/max_bytes/scope)"
```

---

## Task 7: README + final gates

**Files:**
- Create: `crates/tau-plugins/fs-write/README.md`

- [ ] **Step 1: Create `README.md`**

````markdown
# `fs-write` tool plugin

Write or edit a single absolute path under the calling agent's
`fs.write` capability scope. Write-side mirror of
[`fs-read`](../fs-read/README.md).

## Trust model (v0.1, sandboxing deferred)

Runs **unsandboxed** on the host process. The runtime enforces the
capability check at dispatch; the plugin enforces glob-allowlist
scoping + `max_bytes` at invoke time. No memory / CPU / network
isolation (Constitution G12 / ROADMAP Tier 3). Treat installed
plugins as host-equivalent code.

## Usage

Declare the agent's grant in `tau.toml`:

```toml
[[agents.<id>.requires]]
plugin = "fs-write"

[[agents.<id>.capabilities]]
kind = "fs.write"
paths = ["${PROJECT}/src/**"]
max_bytes = 1048576          # optional
```

### write mode — create or truncate

```json
{ "mode": "write", "path": "/abs/path", "contents": "<base64 bytes>" }
```

### edit mode — replace old_str with new_str

```json
{ "mode": "edit", "path": "/abs/path", "old_str": "...", "new_str": "...", "replace_all": false }
```

`old_str` must match **exactly once** unless `replace_all` is true.

Response (both modes):

```json
{ "bytes_written": 1234, "path": "/abs/path" }
```

## Validation rules

- Path must be **absolute**, contain no `..` segments, no NUL bytes.
- Path must match the agent's `fs.write` allow-globs and not its
  deny-globs (deny wins).
- Decoded write size / post-edit file size must not exceed `max_bytes`
  when the grant sets it.

These are **`BadArgs`** (RPC-rejected). Filesystem outcomes — IO
errors, base64 decode failure, `old_str` not-found / ambiguous — are
returned as `ToolResult { is_error: true }` so the LLM may retry.
`edit` requires an existing, UTF-8-decodable file.

## See also

- Spec: [`docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`](../../../docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md)
- Sibling: [`fs-read`](../fs-read/README.md)
- ADR-0008 §5 (IPC vocabulary).
````

- [ ] **Step 2: fmt check**

Run: `timeout 30 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo fmt -p fs-write -- --check`
Expected: PASS (no diff). If it fails, run `cargo fmt -p fs-write` and re-check.

- [ ] **Step 3: Final clippy across all targets**

Run: `timeout 240 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo clippy -p fs-write --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Doctest the lib** (the `config.rs` example is `ignore`d, but run for completeness)

Run: `timeout 300 env CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=target/agent-impl cargo test -p fs-write --doc`
Expected: PASS (0 doctests run or all pass).

- [ ] **Step 5: Commit**

```bash
git add crates/tau-plugins/fs-write/README.md
git -c user.name="Test User" -c user.email="lebocq.tit@gmail.com" commit -m "docs(fs-write): README — usage, validation rules, error model"
```

---

## Task 8: Open the PR

- [ ] **Step 1: Push the branch**

```bash
git push -u origin fs-write-edit-plugin
```

- [ ] **Step 2: Open the PR to main**

```bash
gh pr create --base main --title "feat(fs-write): capability-gated fs-write/edit tool plugin" --body "$(cat <<'EOF'
## Summary

Adds an in-tree, capability-gated `fs-write` Tool plugin — the
write-side mirror of `fs-read`. One crate, one `fs-write-plugin`
binary, one tool name `fs-write` with two modes:

- **write** — full create-or-truncate of base64 `contents`.
- **edit** — `old_str`→`new_str`, exactly-once by default with an
  explicit `replace_all` opt-in (Claude Code's contract).

Single `fs.write` allowlist grant authorizes both modes. Two-tier
error model mirrors fs-read (`BadArgs` for static/scope faults,
`is_error:true` for filesystem outcomes). Enforces the grant's
optional `max_bytes` (most-permissive across grants).

Spec: `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`
Plan: `docs/superpowers/plans/2026-06-13-fs-write-edit-plugin.md`

## Tests

- Unit: config, path_check (ported), WriteArgs parse table, capability
  extraction, apply_edit cardinality.
- Integration (FakeStdioPeer): write create, out-of-scope, over-max_bytes,
  edit single-match, edit not-found, edit ambiguous→replace_all,
  traversal, deny-overrides-allow.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Enrol auto-merge**

```bash
gh pr merge --squash --delete-branch --auto
```

- [ ] **Step 4: Report the PR URL to the user.**

---

## Self-Review

**Spec coverage:**
- Topology A (one crate/bin/tool) → Task 1. ✅
- Discriminated `oneOf` schema + serde `tag="mode"` mirror → Task 2 (enum) + Task 5 (schema). ✅
- base64 `contents` for write → Task 5 invoke + Task 6 tests. ✅
- Edit exactly-once + `replace_all` → Task 4 (`apply_edit`) + Task 6 tests. ✅
- Two-tier error model → Task 5 invoke (`BadArgs` vs `semantic_error`) + Task 6 tests. ✅
- `max_bytes` most-permissive-wins → Task 3 (`extract_max_bytes`) + Task 5 enforcement + Task 6 test. ✅
- path_check parity (traversal/deny) → Task 1 (port) + Task 6 cfg(unix) tests. ✅
- Empty `old_str` rejected as BadArgs → Task 5 invoke. ✅
- In-tree wiring (workspace member + tau.toml) → Task 1. ✅
- README + error-model docs → Task 7. ✅

**Placeholder scan:** No TBD/TODO; every code step shows full code; commands include expected output. ✅

**Type consistency:** `WriteArgs` variants (`Write{path,contents}`, `Edit{path,old_str,new_str,replace_all}`), `EditOutcome::{Replaced,NotFound,Ambiguous}`, `FsWriteSession{allowed_globs,denied_globs,max_bytes}`, helpers `parse_args`/`apply_edit`/`extract_fs_write_paths`/`extract_max_bytes`/`wrote_result`/`semantic_error` — names used consistently across Tasks 2–6. ✅

**Note on imports:** Tasks 2–5 incrementally add `use` lines (`serde::Deserialize`, `FsCapability`, `base64::Engine`, `serde_json::json`, `BTreeMap`). The executor should consolidate the final import block in `plugin.rs` to match the symbols actually used and let `cargo fmt` (Task 7) order them; clippy `-D warnings` (Tasks 5, 7) will catch any unused import.
