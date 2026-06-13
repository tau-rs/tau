//! `FsWritePlugin` — Tool impl for the fs-write plugin.
//!
//! Mutates a single absolute path under the agent's `fs.write`
//! capability scope. Two modes: `write` (full base64 contents) and
//! `edit` (`old_str`→`new_str`).

use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use tau_domain::{Capability, FsCapability, Value};
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{
    fixtures::{make_tool_result, make_tool_spec},
    SessionContext, Tool, ToolContent, ToolError, ToolResult, ToolSpec,
};

use crate::config::FsWriteConfig;
use crate::path_check::{admit_with_deny, validate_path, BadArgs};

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

/// Per-session state derived from the agent's granted capabilities.
pub struct FsWriteSession {
    /// Glob patterns from `FsCapability::Write.paths` (flattened).
    allowed_globs: Vec<String>,
    /// Globs to subtract, from `deny_entries["fs.write"]`. Deny wins.
    denied_globs: Vec<String>,
    /// Most-permissive `max_bytes` across grants; `None` = uncapped.
    max_bytes: Option<u64>,
}

/// fs-write Tool plugin.
pub struct FsWritePlugin {
    #[allow(dead_code)] // reserved for future config knobs
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

    fn capabilities(&self) -> &[Capability] {
        static CAPS: OnceLock<Vec<Capability>> = OnceLock::new();
        CAPS.get_or_init(|| {
            let cap: Capability = serde_json::from_str(r#"{"kind":"fs.write","paths":[]}"#)
                .expect("static fs.write capability JSON is valid");
            vec![cap]
        })
    }

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
}
