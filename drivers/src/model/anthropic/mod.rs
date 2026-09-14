//! The Anthropic model driver: [`ModelRequest`] in, [`ModelReply`] out, over
//! the Messages API, behind a ceiling it enforces itself (ADR-0006).
//!
//! # Configuration is the harness's, not the request's
//!
//! Which model answers, at what prices, under what bound, is
//! [`AnthropicConfig`]. A capability is one endpoint at one model; the
//! request only says what to ask. The API key comes from the environment
//! ([`ApiKey::from_env`]) or from the harness's own secret store
//! ([`ApiKey::new`]) — never from a file in the repository.
//!
//! # The ceiling, from both sides
//!
//! The harness registers the driver with [`AnthropicDriver::ceiling`], which
//! [`AnthropicConfig::ceiling`] derives per ADR-0006 §7:
//!
//! ```text
//! tokens        = input_bound + max_max_tokens
//! cost_microusd = input_bound × input_price + max_max_tokens × output_price
//! ```
//!
//! and the driver keeps that number honest from its side: it **refuses** a
//! request whose input exceeds the bound (`error.over_ceiling`, nothing sent
//! to `/v1/messages`) and **clamps** `max_tokens` to the maximum. How the
//! input is sized is [`AnthropicConfig::estimate`]:
//!
//! - [`InputEstimate::Bytes`], the default: ⌈bytes × 2 ⁄ 5⌉ over the
//!   serialized provider body — bytes ÷ 3, then × 1.2, conservative for
//!   English and JSON. No extra request.
//! - [`InputEstimate::CountTokens`]: `POST /v1/messages/count_tokens` first,
//!   with the same headers and the subset of the body that endpoint takes,
//!   and the bound is checked against the exact `input_tokens` it answers.
//!   The count is part of the same flight: it honours `abandon` and the
//!   timeout like the call, and a count that fails ends the flight with the
//!   same `error.transport` / `error.provider` the call would have given,
//!   nothing sent to `/v1/messages`. It costs no tokens and no money, so it
//!   is not in the reply's `Consumption`; `calls` is the kernel's dimension
//!   and counts the one `send`. The timeout applies to each HTTP exchange,
//!   so a flight in this mode may take up to twice `timeout` in the worst
//!   case.
//!
//! Prices are in microdollars per token at the *uncached* rate.
//!
//! # What it refuses
//!
//! A bridge version other than [`VERSION`], a request that does not parse,
//! and a present `sampling.seed` are `error.unsupported`, nothing sent. The
//! provider's own rejections (4xx/5xx) are `error.provider` with the status
//! and the provider's text; a 200 the driver cannot map is `error.provider`
//! too. A connection failure, the configured timeout, or an `abandon` from
//! `cancel` is `error.transport`. The call is non-streaming, so a provider
//! error never follows a partial answer: consumption is what `usage` says on
//! a 200, and nothing otherwise.
//!
//! # What it does not do
//!
//! No retries — a retry is a second `send`, and that is the loop's call
//! (#34). No effort control, and no thinking control beyond off
//! ([`ThinkingMode`]): thinking blocks in a reply are sealed into the bridge
//! and replayed unchanged (ADR-0007).

mod estimate;
mod wire;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tau_kernel::abi::{Budget, Consumption, Corr};
use tau_kernel::bridge::{ErrorKind, ModelError, ModelReply, ModelRequest, Usage, VERSION};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{BoxFuture, Delivery};
use tokio::sync::Notify;

use super::transport::{encode, error_reply, Exchange, Transport};

// The provider-neutral parts, re-exported so the paths this module always
// had keep resolving. Their home is `model` and `model::ceiling`.
pub use super::ceiling::{clamp_max_tokens, tokens_for_bytes};
pub use super::{ApiKey, ConfigError};
pub use estimate::InputEstimate;
pub use wire::PROVIDER;

/// Where the Messages API lives when the harness does not say otherwise.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// The `anthropic-version` header this driver speaks.
pub const API_VERSION: &str = "2023-06-01";
/// The environment variable [`ApiKey::from_env`] reads by default.
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// The timeout a config gets from [`AnthropicConfig::new`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Everything the harness decides about one Anthropic model capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnthropicConfig {
    /// The model id, as the provider names it (`claude-opus-5`).
    pub model: String,
    /// The API key.
    pub api_key: ApiKey,
    /// The API's base URL. [`DEFAULT_BASE_URL`] in production; a local stub
    /// in tests.
    pub base_url: String,
    /// The largest prompt the driver will send, in tokens as
    /// [`estimate`](Self::estimate) sizes them. A request above it is refused
    /// with `error.over_ceiling`.
    pub input_bound: u64,
    /// The largest `max_tokens` the driver will set. A request asking for
    /// more is clamped.
    pub max_max_tokens: u32,
    /// Input price in microdollars per token, uncached rate.
    pub input_price_microusd: u64,
    /// Output price in microdollars per token.
    pub output_price_microusd: u64,
    /// How long one HTTP exchange may take before it is `error.transport`.
    /// Enforced with `tokio::time`, never by reading a clock. In
    /// [`InputEstimate::CountTokens`] mode the count and the call each get
    /// the full timeout.
    pub timeout: Duration,
    /// How the prompt is sized against `input_bound`.
    /// [`InputEstimate::Bytes`] from [`new`](Self::new).
    pub estimate: InputEstimate,
    /// Whether the model thinks. Driver configuration, not the request's
    /// (ADR-0006 §2, ADR-0007 §5).
    pub thinking: ThinkingMode,
}

/// Whether the model thinks (ADR-0007 §5).
///
/// Off is the only control: no effort, no budget. A model that does not
/// allow off (Claude Fable and Mythos, at the time of writing) answers 400,
/// which the driver reports as `error.provider` with the provider's text —
/// a harness misconfiguration, surfaced on the first call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Omit the parameter: the model's default, which is thinking on for
    /// every current model.
    #[default]
    ProviderDefault,
    /// Send `thinking: {"type": "disabled"}`.
    Disabled,
}

impl AnthropicConfig {
    /// A config with the production base URL, the default timeout and the
    /// bytes estimate; the numbers that make the ceiling are yours to set.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        api_key: ApiKey,
        input_bound: u64,
        max_max_tokens: u32,
        input_price_microusd: u64,
        output_price_microusd: u64,
    ) -> Self {
        Self {
            model: model.into(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_owned(),
            input_bound,
            max_max_tokens,
            input_price_microusd,
            output_price_microusd,
            timeout: DEFAULT_TIMEOUT,
            estimate: InputEstimate::Bytes,
            thinking: ThinkingMode::default(),
        }
    }

    /// The registration ceiling this config implies (ADR-0006 §7): the most
    /// one call can cost when the driver enforces the bound and the clamp.
    ///
    /// At Claude Opus 5 list prices (5 and 25 µUSD per token), an input bound
    /// of 8,000 and a maximum `max_tokens` of 1,024 give 9,024 `tokens` and
    /// 65,600 `cost_microusd`. `calls` is not included; the kernel adds it.
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
    /// `cost_microusd` = in × input price + out × output price. Saturates at
    /// `u64::MAX`, which no real call reaches.
    #[must_use]
    pub fn price(&self, usage: Usage) -> Consumption {
        super::ceiling::price(usage, self.input_price_microusd, self.output_price_microusd)
    }
}

/// The driver. Cheap to clone; every clone shares one HTTP client and one
/// table of in-flight calls.
#[derive(Clone, Debug)]
pub struct AnthropicDriver {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: AnthropicConfig,
    ceiling: Budget,
    /// `/v1/messages`.
    endpoint: reqwest::Url,
    /// `/v1/messages/count_tokens`, used in [`InputEstimate::CountTokens`].
    count_endpoint: reqwest::Url,
    /// The client, the headers, the timeout and the flight registry.
    transport: Transport,
}

impl AnthropicDriver {
    /// Builds the driver, validating the config: the ceiling must fit, the
    /// base URL must parse.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], as described on each variant.
    pub fn new(config: AnthropicConfig) -> Result<Self, ConfigError> {
        let ceiling = config.ceiling()?;
        let endpoint = Transport::endpoint(&config.base_url, "/v1/messages")?;
        let count_endpoint = Transport::endpoint(&config.base_url, "/v1/messages/count_tokens")?;
        let headers = vec![
            ("x-api-key", config.api_key.expose().to_owned()),
            ("anthropic-version", API_VERSION.to_owned()),
        ];
        let transport = Transport::new(headers, config.timeout)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                ceiling,
                endpoint,
                count_endpoint,
                transport,
            }),
        })
    }

    /// The ceiling to register this driver with: the same numbers
    /// [`Driver::handle`] enforces. See [`AnthropicConfig::ceiling`].
    #[must_use]
    pub fn ceiling(&self) -> Budget {
        self.inner.ceiling.clone()
    }

    /// The config this driver was built from.
    #[must_use]
    pub fn config(&self) -> &AnthropicConfig {
        &self.inner.config
    }

    /// How many calls are in flight right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inner.transport.in_flight()
    }
}

impl Driver for AnthropicDriver {
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
    /// The whole pipeline: parse, refuse or map, size the input (locally or
    /// by asking the provider), call, map back.
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
            self.config.thinking,
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
        let (input, sized) = match self.config.estimate {
            InputEstimate::Bytes => (tokens_for_bytes(bytes.len()), "estimated"),
            InputEstimate::CountTokens => match self.count(&body, notify).await {
                Ok(count) => (count, "counted"),
                Err(err) => return self.refuse(err.kind, err.message),
            },
        };
        if input > self.config.input_bound {
            return self.refuse(
                ErrorKind::OverCeiling,
                format!(
                    "input {sized} at {input} tokens, bound is {}",
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

    /// The exact input size, from `count_tokens`. Same flight as the call:
    /// the same `notify`, the same timeout, the same error rules, and a
    /// failure here means the call never goes out.
    async fn count(&self, body: &wire::Request, notify: &Notify) -> Result<u64, ModelError> {
        let bytes =
            serde_json::to_vec(&wire::CountRequest::from(body)).map_err(|e| ModelError {
                kind: ErrorKind::Unsupported,
                message: format!("count request does not serialize: {e}"),
            })?;
        match self
            .transport
            .exchange(&self.count_endpoint, bytes, notify)
            .await
        {
            Exchange::Lost(message) => Err(ModelError {
                kind: ErrorKind::Transport,
                message,
            }),
            Exchange::Answered { status, body } if (200..300).contains(&status) => {
                serde_json::from_slice::<wire::CountResponse>(&body)
                    .map(|count| count.input_tokens)
                    .map_err(|e| ModelError {
                        kind: ErrorKind::Provider,
                        message: format!(
                            "count_tokens answered HTTP {status} with a body that is not a count: {e}"
                        ),
                    })
            }
            Exchange::Answered { status, body } => Err(ModelError {
                kind: ErrorKind::Provider,
                message: provider_message(status, &body),
            }),
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
            Ok(reply) => (reply, consumed),
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

/// `HTTP <status> <type>: <message>` when the body is the provider's error
/// shape; `HTTP <status>: <body, truncated>` otherwise.
fn provider_message(status: u16, body: &[u8]) -> String {
    match serde_json::from_slice::<wire::ErrorBody>(body) {
        Ok(parsed) => format!(
            "HTTP {status} {}: {}",
            parsed.error.kind, parsed.error.message
        ),
        Err(_) => {
            let text = String::from_utf8_lossy(body);
            let short: String = text.chars().take(200).collect();
            format!("HTTP {status}: {short}")
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tau_kernel::abi::DimKey;

    fn config() -> AnthropicConfig {
        AnthropicConfig::new("claude-opus-5", ApiKey::new("k"), 8_000, 1_024, 5, 25)
    }

    #[test]
    fn the_worked_example_ceiling() {
        let ceiling = config().ceiling().unwrap();
        assert_eq!(ceiling.get(&DimKey::Tokens), Some(9_024));
        assert_eq!(ceiling.get(&DimKey::CostMicroUsd), Some(65_600));
        assert_eq!(ceiling.get(&DimKey::Calls), None, "the kernel adds calls");
        let driver = AnthropicDriver::new(config()).unwrap();
        assert_eq!(driver.ceiling(), ceiling);
        assert_eq!(driver.config(), &config());
        assert_eq!(driver.in_flight(), 0);
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
        assert!(AnthropicDriver::new(c).is_err());
    }

    #[test]
    fn pricing_is_in_plus_out_at_the_configured_rates() {
        let consumed = config().price(Usage {
            input_tokens: 120,
            output_tokens: 34,
        });
        assert_eq!(consumed.get(&DimKey::Tokens), Some(154));
        assert_eq!(consumed.get(&DimKey::CostMicroUsd), Some(600 + 850));
    }

    #[test]
    fn a_bad_base_url_is_a_config_error() {
        let mut c = config();
        c.base_url = "not a url".into();
        assert!(matches!(
            AnthropicDriver::new(c),
            Err(ConfigError::BadBaseUrl { .. })
        ));
        let mut c = config();
        c.base_url = "http://127.0.0.1:9/".into();
        let driver = AnthropicDriver::new(c).unwrap();
        assert_eq!(
            driver.inner.endpoint.as_str(),
            "http://127.0.0.1:9/v1/messages"
        );
        assert_eq!(
            driver.inner.count_endpoint.as_str(),
            "http://127.0.0.1:9/v1/messages/count_tokens"
        );
    }

    #[test]
    fn the_config_debug_output_redacts_the_key() {
        assert!(!format!("{:?}", config()).contains("k\""));
    }

    #[test]
    fn provider_messages_carry_the_status_and_the_providers_text() {
        let body = br#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#;
        assert_eq!(
            provider_message(529, body),
            "HTTP 529 overloaded_error: busy"
        );
        assert_eq!(
            provider_message(502, b"<html>bad gateway</html>"),
            "HTTP 502: <html>bad gateway</html>"
        );
    }
}
