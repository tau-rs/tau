//! Anthropic plugin configuration.
//!
//! Deserialized from the handshake `config` field by
//! [`tau_plugin_sdk::run_llm_backend_with_config`]. Two nested
//! concerns: API auth and retry tuning.
//!
//! See `docs/superpowers/specs/2026-04-29-anthropic-plugin-design.md`
//! §6.1.

use serde::Deserialize;
use std::time::Duration;
use tau_plugin_sdk::ConfigError;

/// Top-level config for the Anthropic plugin.
///
/// Deserialized from the handshake `config: serde_json::Value`. All
/// fields have defaults so a project tau.toml `[agents.<id>.config]`
/// section can be empty.
///
/// `#[non_exhaustive]`: additive fields are non-breaking.
///
/// # Example
///
/// ```ignore
/// // `AnthropicConfig` is `#[non_exhaustive]`; external callers
/// // construct via serde or Default.
/// use anthropic_plugin_lib::config::AnthropicConfig;
/// let cfg = AnthropicConfig::default();
/// assert_eq!(cfg.api_key_env, "ANTHROPIC_API_KEY");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicConfig {
    /// Override env var name for the API key. Default: `ANTHROPIC_API_KEY`.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,

    /// Direct API key override. **Test-only** — never put a real key
    /// in project tau.toml. If both `api_key` and `api_key_env` are
    /// present, `api_key` wins and a `tracing::warn!` is emitted.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Override base URL. Default: <https://api.anthropic.com>. Tests
    /// use this to point at the cassette replayer.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Anthropic API version header. Default: `"2023-06-01"`.
    #[serde(default = "default_api_version")]
    pub api_version: String,

    /// Per-request HTTP timeout in seconds. Default: 600 (Anthropic
    /// streaming can run minutes).
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Retry behavior. Defaults match the design spec §Q8.
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_api_key_env(),
            api_key: None,
            base_url: default_base_url(),
            api_version: default_api_version(),
            request_timeout_secs: default_request_timeout_secs(),
            retry: RetryConfig::default(),
        }
    }
}

impl AnthropicConfig {
    /// Per-request HTTP timeout as a `Duration`.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

/// Retry behavior for transient errors (429, 503, network timeouts).
///
/// `#[non_exhaustive]`: additive fields are non-breaking.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    /// Maximum total attempts including the initial request. `1`
    /// disables retry (one-shot). Default: 3.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// Base delay in milliseconds for exponential backoff. Default: 1000.
    /// Effective delay = `base_delay_ms * 2^(attempt-1)`, capped at 60s.
    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: u64,

    /// Honor the `Retry-After` response header when present (parsed as
    /// integer seconds). Default: true.
    #[serde(default = "default_respect_retry_after")]
    pub respect_retry_after: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            base_delay_ms: default_base_delay_ms(),
            respect_retry_after: default_respect_retry_after(),
        }
    }
}

fn default_api_key_env() -> String {
    "ANTHROPIC_API_KEY".into()
}
fn default_base_url() -> String {
    "https://api.anthropic.com".into()
}
fn default_api_version() -> String {
    "2023-06-01".into()
}
fn default_request_timeout_secs() -> u64 {
    600
}
fn default_max_attempts() -> u32 {
    3
}
fn default_base_delay_ms() -> u64 {
    1_000
}
fn default_respect_retry_after() -> bool {
    true
}

/// Validate + resolve the API key from config or env.
///
/// Returns the resolved API key on success. Errors map to [`ConfigError`]
/// variants per spec §6.2 / Task 2 plan-erratum (`InvalidEnvVar` for
/// missing env var, `InvalidValue` for malformed key shape).
///
/// Wired into `Configure::from_config` in Task 9.
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

/// Validate retry-config invariants beyond what serde catches.
///
/// Wired into `Configure::from_config` in Task 9.
pub(crate) fn validate_retry(retry: &RetryConfig) -> Result<(), ConfigError> {
    if retry.max_attempts == 0 {
        return Err(ConfigError::InvalidValue {
            field: "retry.max_attempts",
            detail: "must be >= 1 (use 1 for no-retry semantics)".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_production_ready() {
        let cfg = AnthropicConfig::default();
        assert_eq!(cfg.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(cfg.base_url, "https://api.anthropic.com");
        assert_eq!(cfg.api_version, "2023-06-01");
        assert_eq!(cfg.request_timeout_secs, 600);
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.retry.base_delay_ms, 1000);
        assert!(cfg.retry.respect_retry_after);
    }

    #[test]
    fn deserializes_empty_object_as_defaults() {
        let cfg: AnthropicConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(cfg.retry.max_attempts, 3);
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<AnthropicConfig, _> =
            serde_json::from_str(r#"{"unknown_key": "value"}"#);
        assert!(result.is_err());
    }

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

    #[test]
    fn resolve_api_key_malformed_prefix_returns_invalid_value() {
        let cfg = AnthropicConfig {
            api_key: Some("nope-not-a-real-key".into()),
            ..AnthropicConfig::default()
        };
        let err = resolve_api_key(&cfg, |_| None).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue {
                field: "api_key",
                ..
            }
        ));
    }

    #[test]
    fn validate_retry_zero_attempts_rejected() {
        let retry = RetryConfig {
            max_attempts: 0,
            base_delay_ms: 100,
            respect_retry_after: true,
        };
        let err = validate_retry(&retry).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidValue {
                field: "retry.max_attempts",
                ..
            }
        ));
    }

    #[test]
    fn validate_retry_one_attempt_ok() {
        let retry = RetryConfig {
            max_attempts: 1,
            base_delay_ms: 100,
            respect_retry_after: true,
        };
        validate_retry(&retry).unwrap();
    }
}
