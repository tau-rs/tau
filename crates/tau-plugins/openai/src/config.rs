//! OpenAI plugin configuration.
//!
//! Deserialized from the handshake `config` field by
//! [`tau_plugin_sdk::run_llm_backend_with_config`]. Concerns: required
//! API auth (Bearer token), optional organization header, and retry
//! tuning.
//!
//! See `docs/superpowers/specs/2026-04-29-openai-plugin-design.md` §6.

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;
use tau_plugin_sdk::ConfigError;

/// Top-level config for the OpenAI plugin.
///
/// Deserialized from the handshake `config: serde_json::Value`. All
/// fields have defaults so a project tau.toml `[agents.<id>.config]`
/// section can be empty (with `OPENAI_API_KEY` set in the environment).
///
/// `#[non_exhaustive]`: additive fields are non-breaking.
///
/// # Example
///
/// ```ignore
/// // `OpenAIConfig` is `#[non_exhaustive]`; external callers
/// // construct via serde or Default.
/// use openai_plugin_lib::config::OpenAIConfig;
/// let cfg = OpenAIConfig::default();
/// assert_eq!(cfg.base_url, "https://api.openai.com");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAIConfig {
    /// Override base URL. Default: <https://api.openai.com>.
    /// Tests use this to point at the cassette replayer.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Override env var name for the API key. Default: `OPENAI_API_KEY`.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,

    /// Direct API key override. **Test-only** — never put a real key
    /// in project tau.toml. If both `api_key` and `api_key_env` are
    /// present, `api_key` wins and a `tracing::warn!` is emitted.
    ///
    /// Held as a [`SecretString`] so a stray `{:?}` of this config (a
    /// trace, a panic message, an error wrapper) redacts the key
    /// instead of printing it verbatim (audit S8).
    #[serde(default, deserialize_with = "deserialize_optional_secret")]
    pub api_key: Option<SecretString>,

    /// Per-request HTTP timeout in seconds. Default: 600 (matches
    /// Anthropic — OpenAI streaming can run minutes).
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Optional OpenAI organization id, sent as `OpenAI-Organization`
    /// header when set. Default: `None` (header omitted).
    #[serde(default)]
    pub organization: Option<String>,

    /// Retry behavior. Defaults match the Anthropic plugin.
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key_env: default_api_key_env(),
            api_key: None,
            request_timeout_secs: default_request_timeout_secs(),
            organization: None,
            retry: RetryConfig::default(),
        }
    }
}

impl OpenAIConfig {
    /// Per-request HTTP timeout as a `Duration`.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

/// Retry behavior for transient errors (429, 5xx, network timeouts).
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

    /// Base delay in milliseconds for exponential backoff.
    /// Effective delay = `base_delay_ms * 2^(attempt-1)`, capped at 60s.
    /// Default: 1000.
    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: u64,

    /// Honor the `Retry-After` response header when present (parsed
    /// as integer seconds). Default: true.
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

fn default_base_url() -> String {
    "https://api.openai.com".into()
}
fn default_api_key_env() -> String {
    "OPENAI_API_KEY".into()
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

/// Deserialize an optional API key from a plain JSON string into a
/// [`SecretString`]. Used instead of `secrecy`'s gated `serde` feature
/// so the redaction is end-to-end without a workspace feature change.
fn deserialize_optional_secret<'de, D>(deserializer: D) -> Result<Option<SecretString>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.map(SecretString::from))
}

/// Validate + resolve the API key from config or env.
///
/// Returns the resolved API key on success. Errors map to
/// [`ConfigError`] variants:
/// - [`ConfigError::InvalidEnvVar`] when the configured env var is
///   missing (matches Anthropic's required-auth pattern; distinct from
///   Ollama which returns `Ok(None)`).
/// - [`ConfigError::InvalidValue`] when the resolved key doesn't start
///   with `sk-` (covers both legacy `sk-...` and modern `sk-proj-...`
///   prefixes).
///
/// Wired into `Configure::from_config` in Task 11.
pub(crate) fn resolve_api_key(
    cfg: &OpenAIConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
    let key = if let Some(direct) = cfg.api_key.as_ref() {
        tracing::warn!(
            target: "openai_plugin::config",
            "config.api_key set directly — recommended only for tests"
        );
        direct.expose_secret().to_owned()
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

/// Validate retry-config invariants beyond what serde catches.
///
/// Wired into `Configure::from_config` in Task 11.
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
        let cfg = OpenAIConfig::default();
        assert_eq!(cfg.base_url, "https://api.openai.com");
        assert_eq!(cfg.api_key_env, "OPENAI_API_KEY");
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.request_timeout_secs, 600);
        assert!(cfg.organization.is_none());
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.retry.base_delay_ms, 1000);
        assert!(cfg.retry.respect_retry_after);
    }

    #[test]
    fn deserializes_empty_object_as_defaults() {
        let cfg: OpenAIConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.base_url, "https://api.openai.com");
        assert_eq!(cfg.retry.max_attempts, 3);
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<OpenAIConfig, _> = serde_json::from_str(r#"{"unknown_key": "value"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_api_key_uses_config_override() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-test123".into()),
            ..OpenAIConfig::default()
        };
        let key = resolve_api_key(&cfg, |_| None).unwrap();
        assert_eq!(key, "sk-test123");
    }

    #[test]
    fn resolve_api_key_modern_sk_proj_prefix_accepted() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-proj-modernkey123".into()),
            ..OpenAIConfig::default()
        };
        let key = resolve_api_key(&cfg, |_| None).unwrap();
        assert_eq!(key, "sk-proj-modernkey123");
    }

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

    #[test]
    fn resolve_api_key_malformed_prefix_returns_invalid_value() {
        let cfg = OpenAIConfig {
            api_key: Some("not-a-real-key-prefix".into()),
            ..OpenAIConfig::default()
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

    #[test]
    fn debug_output_redacts_api_key() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-supersecret-leaked-value".into()),
            ..OpenAIConfig::default()
        };
        let debug = format!("{cfg:?}");
        assert!(
            !debug.contains("sk-supersecret-leaked-value"),
            "Debug output leaked the API key: {debug}"
        );
    }

    #[test]
    fn api_key_deserializes_from_plain_string() {
        let cfg: OpenAIConfig = serde_json::from_str(r#"{"api_key": "sk-fromjson"}"#).unwrap();
        let key = resolve_api_key(&cfg, |_| None).unwrap();
        assert_eq!(key, "sk-fromjson");
    }
}
