//! Root `tau.toml` `[allow]` constitution — deserialization, validation,
//! and the validated `AllowConfig`. See ADR-0057 and
//! `docs/superpowers/specs/2026-06-22-epic-1-story-1.2-allow-config-design.md`.
//!
//! Story 1.2 scope: parse + internal well-formedness + round-trip ONLY.
//! Subset enforcement, closed-world reference checks, and the
//! absent-`[allow]` warning are stories 1.3–1.6.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tau_domain::Capability;

use super::project::{ModelEntry, ProjectConfigError, RawModelEntry};

/// Unchecked `[allow]` table. `models` / `mcp` / `tools` bind to named
/// fields; every other key is a raw-cap kind captured by `caps` via
/// `#[serde(flatten)]` (e.g. `"fs.read" = { paths = [...] }`).
///
/// `#[serde(flatten)]` disables `deny_unknown_fields`, so unknown or
/// malformed raw-cap kinds are rejected in `validate_allow`, not here.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct UncheckedAllow {
    /// `[allow.models]` — alias → `{ backend, model }`.
    #[serde(default)]
    pub models: BTreeMap<String, RawModelEntry>,
    /// `[allow.mcp.<name>]` — registered MCP servers.
    #[serde(default)]
    pub mcp: BTreeMap<String, UncheckedMcpAllow>,
    /// `[allow.tools.<name>]` — registered tools + optional cap ceiling.
    #[serde(default)]
    pub tools: BTreeMap<String, UncheckedToolAllow>,
    /// Raw-cap ceiling entries, kind-as-key: `"fs.read" => { paths = [...] }`.
    #[serde(flatten)]
    pub caps: BTreeMap<String, toml::Value>,
}

/// Unchecked `[allow.mcp.<name>]` block. The `url` is the grant of network
/// reach; `hosts` (optional) widens/narrows the derived host ceiling.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedMcpAllow {
    /// MCP server URL.
    pub url: String,
    /// Explicit host ceiling override; empty = derive from `url`.
    #[serde(default)]
    pub hosts: Vec<String>,
}

/// Unchecked `[allow.tools.<name>]` block. Exactly one of `native` / `mcp`
/// is the binding; remaining keys are an optional per-tool cap ceiling
/// (kind-as-key, same shape as `UncheckedAllow::caps`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UncheckedToolAllow {
    /// Native tool symbol binding.
    #[serde(default)]
    pub native: Option<String>,
    /// MCP tool binding (registered MCP name).
    #[serde(default)]
    pub mcp: Option<String>,
    /// Optional per-tool cap ceiling, kind-as-key.
    #[serde(flatten)]
    pub caps: BTreeMap<String, toml::Value>,
}

/// Validated `[allow]` constitution. Produced by [`validate_allow`].
#[derive(Debug, Clone, PartialEq)]
pub struct AllowConfig {
    /// Root raw-cap ceiling, as canonical `Capability` values.
    pub ceiling: Vec<Capability>,
    /// `[allow.models]` — the sole home for alias → `{ backend, model }`.
    pub models: BTreeMap<String, ModelEntry>,
    /// `[allow.mcp.<name>]` — registered MCP servers + host ceiling.
    pub mcp: BTreeMap<String, McpAllowEntry>,
    /// `[allow.tools.<name>]` — registered tools + per-tool cap ceiling.
    pub tools: BTreeMap<String, ToolAllowEntry>,
}

/// Validated `[allow.mcp.<name>]` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct McpAllowEntry {
    /// MCP server URL.
    pub url: String,
    /// Host ceiling (explicit, or derived from `url`).
    pub hosts: Vec<String>,
}

/// Validated `[allow.tools.<name>]` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolAllowEntry {
    /// Native or MCP binding.
    pub binding: ToolBinding,
    /// Per-tool cap ceiling (may be empty).
    pub ceiling: Vec<Capability>,
}

/// A registered tool's binding.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolBinding {
    /// Native tool symbol.
    Native(String),
    /// MCP tool (registered MCP name).
    Mcp(String),
}

/// Raw-cap kinds permitted as `[allow]` keys (and `[allow.tools.*]` ceiling
/// keys). `agent.spawn` flows through the lattice's spawn link, not a raw
/// ceiling entry; `custom`/anything else is not a narrowable ceiling kind.
const ALLOWED_CAP_KINDS: &[&str] =
    &["fs.read", "fs.write", "fs.exec", "net.http", "process.spawn"];

fn err(message: impl Into<String>) -> ProjectConfigError {
    ProjectConfigError::AllowValidation {
        message: message.into(),
    }
}

/// Bridge one kind-as-key raw cap (`"fs.read" => { paths = [...] }`) into a
/// canonical `Capability`, re-emitting it as `{ kind, ... }` for the domain
/// deserializer. Rejects non-whitelisted kinds and non-table values.
fn bridge_cap(kind: &str, value: &toml::Value) -> Result<Capability, ProjectConfigError> {
    if !ALLOWED_CAP_KINDS.contains(&kind) {
        return Err(err(format!(
            "raw-cap kind {kind:?} is not permitted in [allow] \
             (allowed: fs.read, fs.write, fs.exec, net.http, process.spawn)"
        )));
    }
    // toml::Value → serde_json::Value, then inject `kind`.
    let json: JsonValue = serde_json::to_value(value)
        .map_err(|e| err(format!("raw-cap {kind:?}: not serializable: {e}")))?;
    let JsonValue::Object(mut obj) = json else {
        return Err(err(format!("raw-cap {kind:?}: value must be a table")));
    };
    obj.insert("kind".to_string(), JsonValue::String(kind.to_string()));
    serde_json::from_value::<Capability>(JsonValue::Object(obj))
        .map_err(|e| err(format!("raw-cap {kind:?}: malformed: {e}")))
}

/// Bridge a kind-as-key cap map into a sorted `Vec<Capability>`.
fn bridge_caps(caps: &BTreeMap<String, toml::Value>) -> Result<Vec<Capability>, ProjectConfigError> {
    // BTreeMap iteration is sorted by key, giving deterministic ceiling order.
    caps.iter().map(|(k, v)| bridge_cap(k, v)).collect()
}

/// Validate an `[allow]` constitution into an [`AllowConfig`].
///
/// Story 1.2: raw-cap ceiling bridge + (Task 3) registry well-formedness.
/// No subset / closed-world / cross-reference checks (stories 1.3–1.6).
pub fn validate_allow(raw: UncheckedAllow) -> Result<AllowConfig, ProjectConfigError> {
    let ceiling = bridge_caps(&raw.caps)?;
    Ok(AllowConfig {
        ceiling,
        models: BTreeMap::new(), // filled in Task 3
        mcp: BTreeMap::new(),    // filled in Task 3
        tools: BTreeMap::new(),  // filled in Task 3
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tau_domain::{Capability, FsCapability, NetCapability, ProcessCapability};

    fn allow_from(toml: &str) -> UncheckedAllow {
        toml::from_str(toml).expect("parse [allow]")
    }

    #[test]
    fn raw_caps_bridge_into_capability_vec() {
        let raw = allow_from(
            r#"
"fs.read" = { paths = ["/proj/**"] }
"net.http" = { hosts = ["api.weather.com"] }
"process.spawn" = { commands = ["git"] }
"#,
        );
        let cfg = validate_allow(raw).expect("validate");
        // Ceiling is sorted by the BTreeMap key order: fs.read, net.http, process.spawn.
        assert!(cfg.ceiling.iter().any(|c| matches!(
            c,
            Capability::Filesystem(FsCapability::Read { paths, .. }) if paths == &["/proj/**".to_string()]
        )));
        assert!(cfg.ceiling.iter().any(|c| matches!(
            c,
            Capability::Network(NetCapability::Http { hosts, .. }) if hosts == &["api.weather.com".to_string()]
        )));
        assert!(cfg.ceiling.iter().any(|c| matches!(
            c,
            Capability::Process(ProcessCapability::Spawn { commands, .. }) if commands == &["git".to_string()]
        )));
    }

    #[test]
    fn agent_spawn_key_rejected() {
        let raw = allow_from(r#""agent.spawn" = { allowed_kinds = ["worker"] }"#);
        let err = validate_allow(raw).unwrap_err();
        assert!(format!("{err}").contains("agent.spawn"), "got: {err}");
    }

    #[test]
    fn custom_key_rejected() {
        let raw = allow_from(r#""task_list" = { mode = "read" }"#);
        let err = validate_allow(raw).unwrap_err();
        assert!(format!("{err}").contains("task_list"), "got: {err}");
    }

    #[test]
    fn unknown_key_rejected() {
        let raw = allow_from(r#""fs.teleport" = { paths = ["/"] }"#);
        let err = validate_allow(raw).unwrap_err();
        assert!(format!("{err}").contains("fs.teleport"), "got: {err}");
    }

    #[test]
    fn malformed_cap_shape_rejected() {
        // net.http with `paths` instead of `hosts` — wrong field for the kind.
        // The bridge tolerates extra fields, so we assert the *value* is wrong
        // by checking hosts is empty (paths is ignored). A stricter shape check
        // belongs to 1.4; here we only require the bridge to succeed-or-name.
        // Use a genuinely un-bridgeable value instead: a non-table.
        let raw = allow_from(r#""fs.read" = "not-a-table""#);
        let err = validate_allow(raw).unwrap_err();
        assert!(format!("{err}").contains("fs.read"), "got: {err}");
    }

    #[test]
    fn full_allow_deserializes_into_named_and_flatten_fields() {
        let toml = r#"
"fs.read" = { paths = ["/proj/**"] }
"process.spawn" = { commands = ["git"] }

[models]
fast = { backend = "anthropic", model = "claude-haiku-4-5" }

[mcp.weather]
url = "https://api.weather.com/mcp"

[tools.read_temp]
native = "ReadTemp"
"fs.read" = { paths = ["/proj/sensors/**"] }
"#;
        let allow: UncheckedAllow = toml::from_str(toml).expect("parse [allow]");
        assert_eq!(allow.caps.len(), 2, "raw caps go to flatten map");
        assert!(allow.caps.contains_key("fs.read"));
        assert!(allow.caps.contains_key("process.spawn"));
        assert_eq!(allow.models.len(), 1);
        assert_eq!(allow.mcp["weather"].url, "https://api.weather.com/mcp");
        assert_eq!(allow.tools["read_temp"].native.as_deref(), Some("ReadTemp"));
        // The tool's per-tool ceiling cap lands in its own flatten map.
        assert!(allow.tools["read_temp"].caps.contains_key("fs.read"));
    }

    #[test]
    fn allow_round_trips() {
        let toml = r#"
"net.http" = { hosts = ["api.weather.com"] }

[models]
fast = { backend = "anthropic", model = "claude-haiku-4-5" }

[mcp.weather]
url = "https://api.weather.com/mcp"
hosts = ["api.weather.com"]
"#;
        let parsed: UncheckedAllow = toml::from_str(toml).expect("parse");
        let serialized = toml::to_string(&parsed).expect("serialize");
        let reparsed: UncheckedAllow = toml::from_str(&serialized).expect("re-parse");
        assert_eq!(parsed, reparsed, "round-trip must be structurally equal");
    }
}
