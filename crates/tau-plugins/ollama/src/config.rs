//! Ollama plugin configuration.
//!
//! Deserialized from the handshake `config` field by
//! [`tau_plugin_sdk::run_llm_backend_with_config`]. Three concerns:
//! base URL, optional bearer-token auth, and retry tuning.
//!
//! See `docs/superpowers/specs/2026-04-29-ollama-plugin-design.md` §6.

use serde::Deserialize;
use std::time::Duration;
use tau_plugin_sdk::ConfigError;

/// Top-level config for the Ollama plugin.
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
/// // `OllamaConfig` is `#[non_exhaustive]`; external callers
/// // construct via serde or Default.
/// use ollama_plugin_lib::config::OllamaConfig;
/// let cfg = OllamaConfig::default();
/// assert_eq!(cfg.base_url, "http://localhost:11434");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OllamaConfig {
    /// Override base URL. Default: <http://localhost:11434>.
    /// Tests use this to point at the cassette replayer.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Override env var name for an optional bearer token.
    /// Default: `OLLAMA_BEARER_TOKEN`. Unset env var → no
    /// `Authorization` header sent (correct for local Ollama).
    #[serde(default = "default_bearer_token_env")]
    pub bearer_token_env: String,

    /// Direct bearer-token override. **Test-only** — never put a real
    /// token in project tau.toml. If both `bearer_token` and
    /// `bearer_token_env` are present, `bearer_token` wins and a
    /// `tracing::warn!` is emitted.
    #[serde(default)]
    pub bearer_token: Option<String>,

    /// Per-request HTTP timeout in seconds. Default: 900 (15 min).
    /// Local Ollama can take 30–60s to load a model on first call.
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// Retry behavior. Defaults match the Anthropic plugin.
    #[serde(default)]
    pub retry: RetryConfig,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            bearer_token_env: default_bearer_token_env(),
            bearer_token: None,
            request_timeout_secs: default_request_timeout_secs(),
            retry: RetryConfig::default(),
        }
    }
}

impl OllamaConfig {
    /// Per-request HTTP timeout as a `Duration`.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

/// Retry behavior for transient errors (429, 503-on-model-load,
/// network timeouts).
///
/// 503 is the load-bearing case for Ollama: returned during model
/// load, which can take 10–60s. Standard exponential backoff
/// (1s, 2s, 4s) handles short loads.
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
    "http://localhost:11434".into()
}
fn default_bearer_token_env() -> String {
    "OLLAMA_BEARER_TOKEN".into()
}
fn default_request_timeout_secs() -> u64 {
    900
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

/// Resolve an optional bearer token from config or env.
///
/// Returns `Ok(None)` when neither is set — the common case for local
/// Ollama. **Distinct from the Anthropic plugin's `resolve_api_key`,
/// which errors on missing env var because Anthropic auth is required.**
///
/// Wired into `Configure::from_config` in Task 8.
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

/// Validate retry-config invariants beyond what serde catches.
///
/// Wired into `Configure::from_config` in Task 8.
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
        let cfg = OllamaConfig::default();
        assert_eq!(cfg.base_url, "http://localhost:11434");
        assert_eq!(cfg.bearer_token_env, "OLLAMA_BEARER_TOKEN");
        assert!(cfg.bearer_token.is_none());
        assert_eq!(cfg.request_timeout_secs, 900);
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.retry.base_delay_ms, 1000);
        assert!(cfg.retry.respect_retry_after);
    }

    #[test]
    fn deserializes_empty_object_as_defaults() {
        let cfg: OllamaConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.base_url, "http://localhost:11434");
        assert_eq!(cfg.retry.max_attempts, 3);
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<OllamaConfig, _> = serde_json::from_str(r#"{"unknown_key": "value"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_bearer_token_uses_config_override() {
        let cfg = OllamaConfig {
            bearer_token: Some("hosted-token-xyz".into()),
            ..OllamaConfig::default()
        };
        let token = resolve_bearer_token(&cfg, |_| None).unwrap();
        assert_eq!(token.as_deref(), Some("hosted-token-xyz"));
    }

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
    fn resolve_bearer_token_missing_env_returns_none() {
        // Distinct from Anthropic: Ollama auth is OPTIONAL. A missing
        // env var is not an error; it means "no auth header sent".
        let cfg = OllamaConfig {
            bearer_token_env: "DEFINITELY_NOT_SET_OLLAMA_TOK_QXZ".into(),
            ..OllamaConfig::default()
        };
        let token = resolve_bearer_token(&cfg, |_| None).unwrap();
        assert!(token.is_none());
    }

    #[test]
    fn resolve_bearer_token_empty_env_treated_as_none() {
        // Defensive: an empty-string env var is treated the same as
        // unset. Avoids spurious `Authorization: Bearer ` headers.
        let cfg = OllamaConfig {
            bearer_token_env: "ANY_NAME".into(),
            ..OllamaConfig::default()
        };
        let token = resolve_bearer_token(&cfg, |_| Some(String::new())).unwrap();
        assert!(token.is_none());
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
