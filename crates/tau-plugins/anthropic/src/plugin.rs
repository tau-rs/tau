//! [`AnthropicPlugin`] — top-level type implementing
//! [`tau_ports::LlmBackend`] for the Anthropic Messages API.
//!
//! Per spec §6.3:
//! - [`Configure::from_config`] validates inputs
//!   (`resolve_api_key`, `validate_retry`), constructs the
//!   `reqwest::Client` with the configured timeout + a tau-branded
//!   user-agent, and assembles an `AnthropicPlugin { client: AnthropicClient }`.
//! - [`LlmBackend::name`] returns `"anthropic"`.
//! - [`LlmBackend::complete`] builds a non-streaming body via
//!   `build_messages_body`, posts it via `AnthropicClient::post_messages`,
//!   and parses the response with `parse_messages_response`.
//! - [`LlmBackend::stream`] builds a streaming body, posts it, and hands
//!   the `reqwest::Response` to `parse_sse` to produce a
//!   [`tau_ports::CompletionStream`].

use secrecy::SecretString;
use tau_plugin_sdk::{ConfigError, Configure};
use tau_ports::{CompletionRequest, CompletionResponse, CompletionStream, LlmBackend, LlmError};

use crate::client::AnthropicClient;
use crate::config::{resolve_api_key, validate_retry, AnthropicConfig};
use crate::error::{map_client_error, map_response_error};
use crate::request::build_messages_body;
use crate::response::parse_messages_response;
use crate::stream::parse_sse;

/// Anthropic Claude (Messages API) plugin.
///
/// Constructed via [`Configure::from_config`] from an
/// [`AnthropicConfig`]. Holds an `AnthropicClient` preconfigured with
/// the resolved API key, base URL, API-version header, retry policy,
/// and a `reqwest::Client` carrying the per-request HTTP timeout.
pub struct AnthropicPlugin {
    client: AnthropicClient,
}

impl Configure for AnthropicPlugin {
    type Config = AnthropicConfig;

    fn from_config(cfg: Self::Config) -> Result<Self, ConfigError> {
        let api_key = resolve_api_key(&cfg, |n| std::env::var(n).ok())?;
        validate_retry(&cfg.retry)?;

        let inner = reqwest::Client::builder()
            .timeout(cfg.request_timeout())
            .user_agent(format!(
                "tau-anthropic-plugin/{}",
                env!("CARGO_PKG_VERSION"),
            ))
            .build()
            .map_err(|e| ConfigError::InvalidValue {
                field: "request_timeout",
                detail: format!("could not build HTTP client: {e}"),
            })?;

        let client = AnthropicClient::new(
            inner,
            cfg.base_url,
            SecretString::new(api_key.into()),
            cfg.api_version,
            cfg.retry,
        );
        Ok(AnthropicPlugin { client })
    }
}

impl LlmBackend for AnthropicPlugin {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let body = build_messages_body(&req, false).map_err(|e| LlmError::Internal {
            message: format!("anthropic: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_messages(&body, false)
            .await
            .map_err(map_client_error)?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(map_response_error(status, &headers, &body));
        }

        let body = resp.text().await.map_err(|e| LlmError::Internal {
            message: format!("anthropic: read response body: {e}"),
        })?;
        parse_messages_response(&body).map_err(|e| LlmError::Internal {
            message: format!("anthropic: parse response: {e}"),
        })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let body = build_messages_body(&req, true).map_err(|e| LlmError::Internal {
            message: format!("anthropic: build request body: {e}"),
        })?;
        let resp = self
            .client
            .post_messages(&body, true)
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
    fn from_config_with_valid_config_constructs_plugin() {
        let cfg = AnthropicConfig {
            api_key: Some("sk-ant-test-key-123".into()),
            ..AnthropicConfig::default()
        };
        let result = AnthropicPlugin::from_config(cfg);
        assert!(result.is_ok());
    }

    #[test]
    fn from_config_with_missing_api_key_returns_invalid_env_var() {
        let cfg = AnthropicConfig {
            api_key_env: "DEFINITELY_NOT_SET_QWERTY_ZZZ".into(),
            ..AnthropicConfig::default()
        };
        // `AnthropicPlugin` intentionally doesn't derive `Debug` (it owns
        // a `SecretString`), so we can't use `unwrap_err()`. Match the
        // result instead.
        match AnthropicPlugin::from_config(cfg) {
            Ok(_) => panic!("expected ConfigError::InvalidEnvVar"),
            Err(err) => assert!(matches!(
                err,
                ConfigError::InvalidEnvVar { ref name, .. }
                    if name == "DEFINITELY_NOT_SET_QWERTY_ZZZ"
            )),
        }
    }

    #[test]
    fn name_returns_anthropic() {
        let cfg = AnthropicConfig {
            api_key: Some("sk-ant-foo".into()),
            ..AnthropicConfig::default()
        };
        let plugin = AnthropicPlugin::from_config(cfg).unwrap();
        assert_eq!(plugin.name(), "anthropic");
    }
}
