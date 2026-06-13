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
