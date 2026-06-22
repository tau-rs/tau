//! Root `tau.toml` `[allow]` constitution — deserialization, validation,
//! and the validated `AllowConfig`. See ADR-0057 and
//! `docs/superpowers/specs/2026-06-22-epic-1-story-1.2-allow-config-design.md`.
//!
//! Story 1.2 scope: parse + internal well-formedness + round-trip ONLY.
//! Subset enforcement, closed-world reference checks, and the
//! absent-`[allow]` warning are stories 1.3–1.6.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::project::RawModelEntry;

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

#[cfg(test)]
mod tests {
    use super::*;

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
