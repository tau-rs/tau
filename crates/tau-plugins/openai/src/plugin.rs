//! [`OpenAIPlugin`] — top-level type implementing
//! [`tau_ports::LlmBackend`] for OpenAI's Chat Completions API.
//!
//! Per spec §6:
//! - [`Configure::from_config`] validates inputs
//!   (`resolve_api_key`, `validate_retry`), constructs the
//!   `reqwest::Client` with the configured timeout + a tau-branded
//!   user-agent, and assembles an `OpenAIPlugin { client: OpenAIClient }`.
//! - [`LlmBackend::name`] returns `"openai"`.
//! - [`LlmBackend::complete`] builds a non-streaming body via
//!   `build_chat_completions_body`, posts it via
//!   `OpenAIClient::post_chat_completions`, and parses the response
//!   with `parse_chat_completions_response`.
//! - [`LlmBackend::stream`] builds a streaming body, posts it, and
//!   hands the `reqwest::Response` to `parse_sse` to produce a
//!   [`tau_ports::CompletionStream`].
//!
//! Non-success responses extract `headers` BEFORE consuming the
//! response via `text()` so 429 responses can populate
//! `LlmError::RateLimited.retry_after_seconds`.

use secrecy::SecretString;
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError};

use crate::client::OpenAIClient;
use crate::config::{resolve_api_key, validate_retry, OpenAIConfig};
use crate::error::{map_client_error, map_response_error};
use crate::request::build_chat_completions_body;
use crate::response::parse_chat_completions_response;
use crate::stream::parse_sse;

/// OpenAI Chat Completions API plugin.
///
/// Constructed via [`Configure::from_config`] from an
/// [`OpenAIConfig`]. Holds an `OpenAIClient` preconfigured with the
/// resolved API key, base URL, optional organization id, retry policy,
/// and a `reqwest::Client` carrying the per-request HTTP timeout.
pub struct OpenAIPlugin {
    client: OpenAIClient,
}

impl Configure for OpenAIPlugin {
    type Config = OpenAIConfig;

    fn from_config(cfg: Self::Config) -> Result<Self, ConfigError> {
        let api_key = resolve_api_key(&cfg, |n| std::env::var(n).ok())?;
        validate_retry(&cfg.retry)?;

        let inner = reqwest::Client::builder()
            .timeout(cfg.request_timeout())
            .user_agent(format!("tau-openai-plugin/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ConfigError::InvalidValue {
                field: "request_timeout",
                detail: format!("could not build HTTP client: {e}"),
            })?;

        let client = OpenAIClient::new(
            inner,
            cfg.base_url,
            SecretString::new(api_key.into()),
            cfg.organization,
            cfg.retry,
        );
        Ok(OpenAIPlugin { client })
    }
}

impl LlmBackend for OpenAIPlugin {
    fn name(&self) -> &str {
        "openai"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = build_chat_completions_body(&req, false).map_err(|e| LlmError::Internal {
            message: format!("openai: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_chat_completions(&body, false)
            .await
            .map_err(map_client_error)?;

        let status = resp.status();
        if !status.is_success() {
            // Extract headers BEFORE consuming the response via text().
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_response_error(status, &headers, &body));
        }

        let body = resp.text().await.map_err(|e| LlmError::Internal {
            message: format!("openai: read response body: {e}"),
        })?;
        parse_chat_completions_response(&body).map_err(|e| LlmError::Internal {
            message: format!("openai: parse response: {e}"),
        })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let body = build_chat_completions_body(&req, true).map_err(|e| LlmError::Internal {
            message: format!("openai: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_chat_completions(&body, true)
            .await
            .map_err(map_client_error)?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_response_error(status, &headers, &body));
        }

        parse_sse(resp).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_valid_api_key_constructs_plugin() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-proj-test-key-12345".into()),
            ..OpenAIConfig::default()
        };
        let result = OpenAIPlugin::from_config(cfg);
        assert!(result.is_ok(), "from_config should succeed");
    }

    #[test]
    fn from_config_invalid_retry_max_attempts_zero_returns_invalid_value() {
        let mut cfg = OpenAIConfig {
            api_key: Some("sk-test".into()),
            ..OpenAIConfig::default()
        };
        cfg.retry.max_attempts = 0;
        let err = match OpenAIPlugin::from_config(cfg) {
            Ok(_) => panic!("expected ConfigError::InvalidValue"),
            Err(e) => e,
        };
        assert!(matches!(
            err,
            ConfigError::InvalidValue {
                field: "retry.max_attempts",
                ..
            }
        ));
    }

    #[test]
    fn name_returns_openai() {
        let cfg = OpenAIConfig {
            api_key: Some("sk-test".into()),
            ..OpenAIConfig::default()
        };
        let plugin = OpenAIPlugin::from_config(cfg).unwrap();
        assert_eq!(plugin.name(), "openai");
    }
}
