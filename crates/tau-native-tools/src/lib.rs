//! Deterministic native tools shared by the dev conformance profile and the
//! wasm guest (β.7.5 PR-F). One source of truth for each tool's body so the
//! bytes never drift between execution profiles — the property PR-G's
//! `dev == wasm` conformance gate depends on.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(test)]
extern crate std;

use serde_json::{json, Value};

/// Invoke a native tool by its IR `ToolId` string.
///
/// Returns `Some(body)` for a known tool, `None` otherwise (the caller turns
/// `None` into its own "unknown tool" error). Bodies are deterministic and
/// independent of `args` in v0 — exactly the behaviour the conformance
/// fan-monitor relies on.
pub fn invoke(tool_id: &str, _args: &Value) -> Option<Value> {
    match tool_id {
        "read_temp" => Some(json!(32)),
        "set_fan" => Some(json!({ "ok": true })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_temp_returns_32() {
        assert_eq!(invoke("read_temp", &json!({})), Some(json!(32)));
    }

    #[test]
    fn set_fan_returns_ok_true_ignoring_args() {
        assert_eq!(
            invoke("set_fan", &json!({ "on": true })),
            Some(json!({ "ok": true }))
        );
        assert_eq!(invoke("set_fan", &json!({})), Some(json!({ "ok": true })));
    }

    #[test]
    fn unknown_tool_is_none() {
        assert_eq!(invoke("weather", &json!({})), None);
        assert_eq!(invoke("nope", &json!({})), None);
    }
}
