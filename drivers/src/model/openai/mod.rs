//! The OpenAI-compatible model driver: [`ModelRequest`] in, [`ModelReply`]
//! out, over `POST /v1/chat/completions`, behind a ceiling it enforces
//! itself (ADR-0006). "Compatible" is the point: OpenAI, vLLM, llama.cpp,
//! Ollama, LM Studio and the rest all answer the same shape, and one driver
//! pointed at a different base URL covers them.
//!
//! # Configuration is the harness's, not the request's
//!
//! Which model answers, at what prices, under what bound, is
//! [`OpenAiConfig`]. A capability is one endpoint at one model; the request
//! only says what to ask. The API key is optional — a local vLLM has none —
//! and comes from the environment ([`ApiKey::from_env`]) or from the
//! harness's own secret store ([`ApiKey::new`]), never from a file in the
//! repository.
//!
//! # The ceiling, from both sides
//!
//! The harness registers the driver with [`OpenAiDriver::ceiling`], which
//! [`OpenAiConfig::ceiling`] derives per ADR-0006 §7:
//!
//! ```text
//! tokens        = input_bound + max_max_tokens
//! cost_microusd = input_bound × input_price + max_max_tokens × output_price
//! ```
//!
//! and the driver keeps that number honest from its side: it **refuses** a
//! request whose estimated input exceeds the bound (`error.over_ceiling`,
//! nothing sent) and **clamps** `max_tokens` to the maximum. The estimate is
//! ⌈bytes × 2 ⁄ 5⌉ over the serialized provider body. Prices are in
//! microdollars per token at the *uncached* rate; a local server is priced
//! at zero and the ceiling is then tokens only.
//!
//! # `max_tokens` or `max_completion_tokens`
//!
//! The clamped cap goes in one field, chosen by [`OpenAiConfig::output_cap`].
//! The default is `max_tokens`: it is the field every compatible server
//! understands, while `max_completion_tokens` is OpenAI's newer name that
//! its reasoning models require and that older or smaller servers reject.
//! The operator flips it per capability; the driver never sends both.
//!
//! # What it refuses
//!
//! A bridge version other than [`VERSION`], a request that does not parse,
//! and a block in a turn that has no chat-completions shape are
//! `error.unsupported`, nothing sent. Every sampling field passes through:
//! this column has a `seed`. The provider's own rejections (4xx/5xx) are
//! `error.provider` with the status and the provider's text; a 200 the
//! driver cannot map is `error.provider` too. A connection failure, the
//! configured timeout, or an `abandon` from `cancel` is `error.transport`.
//! The call is non-streaming, so a provider error never follows a partial
//! answer: consumption is what `usage` says on a 200, and nothing otherwise.
//!
//! # What it does not do
//!
//! No retries — a retry is a second `send`, and that is the loop's call
//! (#34). No token counting: chat-completions servers have no counting
//! endpoint, so the byte estimate is the estimate. No thinking format of
//! its own: a `thinking` block in a request is another provider's and is
//! dropped (ADR-0007 §2); `reasoning_content` in a reply is ignored, not
//! sealed.

mod wire;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tau_kernel::abi::{Budget, Consumption, Corr};
use tau_kernel::bridge::{ErrorKind, ModelReply, ModelRequest, Usage, VERSION};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{BoxFuture, Delivery};
use tokio::sync::Notify;

use super::transport::{encode, error_reply, Exchange, Transport};

pub use super::ceiling::{clamp_max_tokens, tokens_for_bytes};
pub use super::{ApiKey, ConfigError};

/// Where chat completions live when the harness does not say otherwise.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";
/// The environment variable [`ApiKey::from_env`] reads by default.
pub const API_KEY_ENV: &str = "OPENAI_API_KEY";
/// The timeout a config gets from [`OpenAiConfig::new`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Which request field carries the clamped output cap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputCap {
    /// `max_tokens`: the original field, understood by every compatible
    /// server. The default.
    #[default]
    MaxTokens,
    /// `max_completion_tokens`: OpenAI's newer name, required by its
    /// reasoning models and unknown to some smaller servers.
    MaxCompletionTokens,
}

/// Everything the harness decides about one OpenAI-compatible model
/// capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiConfig {
    /// The model id, as the server names it (`gpt-4.1`, `Qwen/Qwen3-8B`).
    pub model: String,
    /// The API key, sent as `Authorization: Bearer`. `None` sends no header,
    /// which is what a local server without auth expects.
    pub api_key: Option<ApiKey>,
    /// The server's base URL, without the `/v1/chat/completions` path.
    /// [`DEFAULT_BASE_URL`] in production; `http://localhost:8000` for a
    /// local vLLM; a stub in tests.
    pub base_url: String,
    /// Which field carries the output cap. See [`OutputCap`].
    pub output_cap: OutputCap,
    /// The largest prompt the driver will send, in estimated tokens. A
    /// request estimated above it is refused with `error.over_ceiling`.
    pub input_bound: u64,
    /// The largest output cap the driver will set. A request asking for
    /// more is clamped.
    pub max_max_tokens: u32,
    /// Input price in microdollars per token, uncached rate. Zero for a
    /// server you run yourself.
    pub input_price_microusd: u64,
    /// Output price in microdollars per token.
    pub output_price_microusd: u64,
    /// How long one call may take before it is `error.transport`. Enforced
    /// with `tokio::time`, never by reading a clock.
    pub timeout: Duration,
}

impl OpenAiConfig {
    /// A config with the production base URL, `max_tokens` as the cap field,
    /// and the default timeout; the numbers that make the ceiling are yours
    /// to set.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        api_key: Option<ApiKey>,
        input_bound: u64,
        max_max_tokens: u32,
        input_price_microusd: u64,
        output_price_microusd: u64,
    ) -> Self {
        Self {
            model: model.into(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_owned(),
            output_cap: OutputCap::default(),
            input_bound,
            max_max_tokens,
            input_price_microusd,
            output_price_microusd,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The registration ceiling this config implies (ADR-0006 §7): the most
    /// one call can cost when the driver enforces the bound and the clamp.
    /// `calls` is not included; the kernel adds it.
    ///
    /// # Errors
    ///
    /// [`ConfigError::CeilingOverflow`] if a sum or product exceeds `u64`.
    pub fn ceiling(&self) -> Result<Budget, ConfigError> {
        super::ceiling::derive_ceiling(
            self.input_bound,
            self.max_max_tokens,
            self.input_price_microusd,
            self.output_price_microusd,
        )
    }

    /// What `usage` costs at this config's prices: `tokens` = in + out,
    /// `cost_microusd` = in × input price + out × output price.
    #[must_use]
    pub fn price(&self, usage: Usage) -> Consumption {
        super::ceiling::price(usage, self.input_price_microusd, self.output_price_microusd)
    }
}

/// The driver. Cheap to clone; every clone shares one HTTP client and one
/// table of in-flight calls.
#[derive(Clone, Debug)]
pub struct OpenAiDriver {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: OpenAiConfig,
    ceiling: Budget,
    /// `/v1/chat/completions`.
    endpoint: reqwest::Url,
    /// The client, the headers, the timeout and the flight registry.
    transport: Transport,
}

impl OpenAiDriver {
    /// Builds the driver, validating the config: the ceiling must fit, the
    /// base URL must parse.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], as described on each variant.
    pub fn new(config: OpenAiConfig) -> Result<Self, ConfigError> {
        let ceiling = config.ceiling()?;
        let endpoint = Transport::endpoint(&config.base_url, "/v1/chat/completions")?;
        // No key, no header: what a local server without auth expects.
        let headers = config
            .api_key
            .iter()
            .map(|key| ("authorization", format!("Bearer {}", key.expose())))
            .collect();
        let transport = Transport::new(headers, config.timeout)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                ceiling,
                endpoint,
                transport,
            }),
        })
    }

    /// The ceiling to register this driver with: the same numbers
    /// [`Driver::handle`] enforces. See [`OpenAiConfig::ceiling`].
    #[must_use]
    pub fn ceiling(&self) -> Budget {
        self.inner.ceiling.clone()
    }

    /// The config this driver was built from.
    #[must_use]
    pub fn config(&self) -> &OpenAiConfig {
        &self.inner.config
    }

    /// How many calls are in flight right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.transport.in_flight()
    }
}

impl Driver for OpenAiDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        // Registered now, not when the future is first polled: `abandon`
        // may arrive in between, and it must find the entry.
        let inner = Arc::clone(&self.inner);
        let flight = inner.transport.enter(request.corr);
        Box::pin(async move {
            let (reply, consumed) = inner.answer(&request.payload, flight.notify()).await;
            drop(flight);
            (encode(&reply), consumed)
        })
    }

    fn abandon(&self, corr: Corr) {
        self.inner.transport.abandon(corr);
    }
}

impl Inner {
    /// The whole pipeline: parse, refuse or map, estimate, call, map back.
    async fn answer(&self, payload: &[u8], notify: &Notify) -> (ModelReply, Consumption) {
        let request: ModelRequest = match serde_json::from_slice(payload) {
            Ok(request) => request,
            Err(e) => {
                return self.refuse(
                    ErrorKind::Unsupported,
                    format!("payload is not a bridge v{VERSION} request: {e}"),
                )
            }
        };
        if request.v != VERSION {
            return self.refuse(
                ErrorKind::Unsupported,
                format!(
                    "bridge version {} is not supported; this driver speaks v{VERSION}",
                    request.v
                ),
            );
        }
        let body = match wire::to_provider(
            &request,
            &self.config.model,
            self.config.max_max_tokens,
            self.config.output_cap,
        ) {
            Ok(body) => body,
            Err(err) => return self.refuse(err.kind, err.message),
        };
        let bytes = match serde_json::to_vec(&body) {
            Ok(bytes) => bytes,
            Err(e) => {
                return self.refuse(
                    ErrorKind::Unsupported,
                    format!("request does not serialize: {e}"),
                )
            }
        };
        let estimate = tokens_for_bytes(bytes.len());
        if estimate > self.config.input_bound {
            return self.refuse(
                ErrorKind::OverCeiling,
                format!(
                    "input estimated at {estimate} tokens, bound is {}",
                    self.config.input_bound
                ),
            );
        }

        match self.transport.exchange(&self.endpoint, bytes, notify).await {
            Exchange::Lost(message) => self.refuse(ErrorKind::Transport, message),
            Exchange::Answered { status, body } if (200..300).contains(&status) => {
                self.map_answer(&body)
            }
            Exchange::Answered { status, body } => {
                self.refuse(ErrorKind::Provider, provider_message(status, &body))
            }
        }
    }

    /// A 200: bill what `usage` says, whatever else the body holds.
    fn map_answer(&self, body: &[u8]) -> (ModelReply, Consumption) {
        let value: Value = match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(e) => {
                return self.refuse(
                    ErrorKind::Provider,
                    format!("HTTP 200 body is not JSON: {e}"),
                )
            }
        };
        // The usage first, leniently: a body the driver cannot map still
        // consumed what it says it consumed, and that is reported.
        let usage: Usage = value
            .get("usage")
            .cloned()
            .and_then(|u| serde_json::from_value::<wire::ResponseUsage>(u).ok())
            .map(Into::into)
            .unwrap_or_default();
        let consumed = self.config.price(usage);
        let model = value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(self.config.model.clone()));
        let response: wire::Response = match serde_json::from_value(value) {
            Ok(response) => response,
            Err(e) => {
                let reply = error_reply(
                    ErrorKind::Provider,
                    format!("HTTP 200 body could not be mapped: {e}"),
                    model,
                    usage,
                );
                return (reply, consumed);
            }
        };
        match wire::to_bridge(&response, VERSION) {
            Ok(reply) => (
                ModelReply {
                    model: reply.model.or(model),
                    ..reply
                },
                consumed,
            ),
            Err(err) => (error_reply(err.kind, err.message, model, usage), consumed),
        }
    }

    /// A reply that says no, with nothing consumed: nothing was sent, or
    /// nothing was billed.
    fn refuse(&self, kind: ErrorKind, message: String) -> (ModelReply, Consumption) {
        let model = Some(self.config.model.clone());
        (
            error_reply(kind, message, model, Usage::default()),
            Consumption::none(),
        )
    }
}

/// `HTTP <status> <type>: <message>` when the body is either error shape
/// the compatible world uses; `HTTP <status>: <body, truncated>` otherwise.
fn provider_message(status: u16, body: &[u8]) -> String {
    match wire::parse_error(body) {
        Some(detail) if detail.kind.is_empty() => format!("HTTP {status}: {}", detail.message),
        Some(detail) => format!("HTTP {status} {}: {}", detail.kind, detail.message),
        None => {
            let text = String::from_utf8_lossy(body);
            let short: String = text.chars().take(200).collect();
            format!("HTTP {status}: {short}")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use tau_kernel::abi::DimKey;

    use super::*;

    fn config() -> OpenAiConfig {
        OpenAiConfig::new("gpt-4.1", Some(ApiKey::new("k")), 8_000, 1_024, 5, 25)
    }

    #[test]
    fn the_worked_example_ceiling() {
        let ceiling = config().ceiling().unwrap();
        assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
        assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(65_600));
        assert_eq!(ceiling.get(&DimKey::Calls), None, "the kernel adds calls");
        let driver = OpenAiDriver::new(config()).unwrap();
        assert_eq!(driver.ceiling(), ceiling);
        assert_eq!(driver.config(), &config());
        assert_eq!(driver.config().output_cap, OutputCap::MaxTokens);
        assert_eq!(driver.in_flight(), 0);
    }

    #[test]
    fn a_free_local_server_has_a_tokens_only_cost() {
        let c = OpenAiConfig::new("Qwen/Qwen3-8B", None, 8_000, 1_024, 0, 0);
        let ceiling = c.ceiling().unwrap();
        assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
        assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(0));
        let consumed = c.price(Usage {
            input_tokens: 120,
            output_tokens: 34,
        });
        assert_eq!(consumed.get(&DimKey::Tokens), Some(154));
        assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(0));
    }

    #[test]
    fn the_ceiling_refuses_to_overflow() {
        let mut c = config();
        c.input_bound = u64::MAX;
        assert!(matches!(
            c.ceiling(),
            Err(ConfigError::CeilingOverflow {
                dim: DimKey::Tokens
            })
        ));
        assert!(OpenAiDriver::new(c).is_err());
    }

    #[test]
    fn a_bad_base_url_is_a_config_error() {
        let mut c = config();
        c.base_url = "not a url".into();
        assert!(matches!(
            OpenAiDriver::new(c),
            Err(ConfigError::BadBaseUrl { .. })
        ));
        let mut c = config();
        c.base_url = "http://localhost:8000/".into();
        let driver = OpenAiDriver::new(c).unwrap();
        assert_eq!(
            driver.inner.endpoint.as_str(),
            "http://localhost:8000/v1/chat/completions"
        );
    }

    #[test]
    fn the_config_debug_output_redacts_the_key() {
        assert!(!format!("{:?}", config()).contains("k\""));
    }

    #[test]
    fn provider_messages_carry_the_status_and_the_servers_text() {
        let wrapped = br#"{"error":{"message":"slow down","type":"rate_limit_error"}}"#;
        assert_eq!(
            provider_message(429, wrapped),
            "HTTP 429 rate_limit_error: slow down"
        );
        let flat =
            br#"{"object":"error","message":"busy","type":"ServiceUnavailableError","code":503}"#;
        assert_eq!(
            provider_message(503, flat),
            "HTTP 503 ServiceUnavailableError: busy"
        );
        let untyped = br#"{"message":"nope"}"#;
        assert_eq!(provider_message(400, untyped), "HTTP 400: nope");
        assert_eq!(
            provider_message(502, b"<html>bad gateway</html>"),
            "HTTP 502: <html>bad gateway</html>"
        );
    }
}
