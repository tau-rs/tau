//! [`OllamaPlugin`] — top-level type implementing
//! [`tau_ports::LlmBackend`] for Ollama's native `/api/chat` API.
//!
//! Per spec §6.2, §6.3:
//! - [`Configure::from_config`] validates inputs
//!   (`resolve_bearer_token`, `validate_retry`), constructs the
//!   `reqwest::Client` with the configured timeout + a tau-branded
//!   user-agent, and assembles an `OllamaPlugin { client: OllamaClient }`.
//! - [`LlmBackend::name`] returns `"ollama"`.
//! - [`LlmBackend::complete`] builds a non-streaming body via
//!   `build_chat_body`, posts it via `OllamaClient::post_chat`,
//!   and parses the response with `parse_chat_response`.
//! - [`LlmBackend::stream`] builds a streaming body, posts it, and
//!   hands the `reqwest::Response` to `parse_ndjson` to produce a
//!   [`tau_ports::CompletionStream`].

use secrecy::SecretString;
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError};

use crate::client::OllamaClient;
use crate::config::{resolve_bearer_token, validate_retry, OllamaConfig};
use crate::error::{map_client_error, map_response_error};
use crate::request::build_chat_body;
use crate::response::parse_chat_response;
use crate::stream::parse_ndjson;

/// Ollama (local LLM runner) plugin.
///
/// Constructed via [`Configure::from_config`] from an
/// [`OllamaConfig`]. Holds an `OllamaClient` preconfigured with the
/// optional bearer token (None for local Ollama at
/// `http://localhost:11434`), base URL, retry policy, and a
/// `reqwest::Client` carrying the per-request HTTP timeout.
pub struct OllamaPlugin {
    client: OllamaClient,
}

impl Configure for OllamaPlugin {
    type Config = OllamaConfig;

    fn from_config(cfg: Self::Config) -> Result<Self, ConfigError> {
        let bearer_token = resolve_bearer_token(&cfg, |n| std::env::var(n).ok())?;
        validate_retry(&cfg.retry)?;

        let inner = reqwest::Client::builder()
            .timeout(cfg.request_timeout())
            .user_agent(format!("tau-ollama-plugin/{}", env!("CARGO_PKG_VERSION"),))
            .build()
            .map_err(|e| ConfigError::InvalidValue {
                field: "request_timeout",
                detail: format!("could not build HTTP client: {e}"),
            })?;

        let client = OllamaClient::new(
            inner,
            cfg.base_url,
            bearer_token.map(|t| SecretString::new(t.into())),
            cfg.retry,
        );
        Ok(OllamaPlugin { client })
    }
}

impl LlmBackend for OllamaPlugin {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = build_chat_body(&req, false).map_err(|e| LlmError::Internal {
            message: format!("ollama: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_chat(&body, false)
            .await
            .map_err(map_client_error)?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_response_error(status, &headers, &body));
        }

        let body = resp.text().await.map_err(|e| LlmError::Internal {
            message: format!("ollama: read response body: {e}"),
        })?;
        parse_chat_response(&body).map_err(|e| LlmError::Internal {
            message: format!("ollama: parse response: {e}"),
        })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let body = build_chat_body(&req, true).map_err(|e| LlmError::Internal {
            message: format!("ollama: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_chat(&body, true)
            .await
            .map_err(map_client_error)?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_response_error(status, &headers, &body));
        }

        parse_ndjson(resp).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_default_succeeds_with_no_bearer_token() {
        // Local Ollama needs no auth. Default config + no env var set
        // should construct cleanly.
        let cfg = OllamaConfig {
            // Use a definitely-unset env var name to ensure no accidental
            // leakage from the test environment.
            bearer_token_env: "DEFINITELY_NOT_SET_OLLAMA_LOCAL_TOKEN".into(),
            ..OllamaConfig::default()
        };
        let plugin = OllamaPlugin::from_config(cfg).expect("plugin should build");
        assert_eq!(plugin.name(), "ollama");
    }

    #[test]
    fn from_config_invalid_retry_max_attempts_zero_returns_invalid_value() {
        let mut cfg = OllamaConfig::default();
        cfg.retry.max_attempts = 0;
        match OllamaPlugin::from_config(cfg) {
            Ok(_) => panic!("expected ConfigError::InvalidValue"),
            Err(err) => assert!(matches!(
                err,
                ConfigError::InvalidValue {
                    field: "retry.max_attempts",
                    ..
                }
            )),
        }
    }

    #[test]
    fn from_config_with_bearer_token_succeeds() {
        let cfg = OllamaConfig {
            bearer_token: Some("hosted-token-xyz".into()),
            ..OllamaConfig::default()
        };
        let plugin = OllamaPlugin::from_config(cfg).expect("plugin should build");
        assert_eq!(plugin.name(), "ollama");
    }
}
