//! `FsWritePlugin` — Tool impl for the fs-write plugin.
//!
//! Mutates a single absolute path under the agent's `fs.write`
//! capability scope. Two modes: `write` (full base64 contents) and
//! `edit` (`old_str`→`new_str`).

use serde::Deserialize;
use std::sync::OnceLock;
use tau_domain::{Capability, FsCapability, Value};
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{
    fixtures::{make_tool_result, make_tool_spec},
    SessionContext, Tool, ToolContent, ToolError, ToolResult, ToolSpec,
};

use crate::config::FsWriteConfig;

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
}
