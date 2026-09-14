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
//! request whose estimated input exceeds the bound (`error.over_ceiling`,
//! nothing sent) and **clamps** `max_tokens` to the maximum. The estimate is
//! ⌈bytes × 2 ⁄ 5⌉ over the serialized provider body — bytes ÷ 3, then
//! × 1.2, conservative for English and JSON. Prices are in microdollars per
//! token at the *uncached* rate.
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
//! (#34). No token-counting endpoint yet (#32). No thinking controls:
//! `thinking` blocks in a reply have no bridge slot and are dropped (#42).

mod estimate;
mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tau_kernel::abi::{Budget, Consumption, Corr, DimKey};
use tau_kernel::bridge::{
    ErrorKind, ModelError, ModelReply, ModelRequest, StopReason, Usage, VERSION,
};
use tau_kernel::driver::Driver;
use tau_kernel::kernel::{BoxFuture, Delivery};
use tokio::sync::Notify;

pub use estimate::{clamp_max_tokens, tokens_for_bytes};

/// Where the Messages API lives when the harness does not say otherwise.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// The `anthropic-version` header this driver speaks.
pub const API_VERSION: &str = "2023-06-01";
/// The environment variable [`ApiKey::from_env`] reads by default.
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// The timeout a config gets from [`AnthropicConfig::new`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// An API key. Its `Debug` output is redacted, so a config can be logged.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key the harness already holds.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// Reads the key from the environment variable `var`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::MissingApiKey`] if `var` is unset, empty, or not
    /// Unicode.
    pub fn from_env(var: &str) -> Result<Self, ConfigError> {
        match std::env::var(var) {
            Ok(key) if !key.is_empty() => Ok(Self(key)),
            _ => Err(ConfigError::MissingApiKey {
                var: var.to_owned(),
            }),
        }
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

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
    /// The largest prompt the driver will send, in estimated tokens. A
    /// request estimated above it is refused with `error.over_ceiling`.
    pub input_bound: u64,
    /// The largest `max_tokens` the driver will set. A request asking for
    /// more is clamped.
    pub max_max_tokens: u32,
    /// Input price in microdollars per token, uncached rate.
    pub input_price_microusd: u64,
    /// Output price in microdollars per token.
    pub output_price_microusd: u64,
    /// How long one call may take before it is `error.transport`. Enforced
    /// with `tokio::time`, never by reading a clock.
    pub timeout: Duration,
}

impl AnthropicConfig {
    /// A config with the production base URL and the default timeout; the
    /// numbers that make the ceiling are yours to set.
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
        let max_out = u64::from(self.max_max_tokens);
        let tokens = self
            .input_bound
            .checked_add(max_out)
            .ok_or(ConfigError::CeilingOverflow {
                dim: DimKey::Tokens,
            })?;
        let cost = self
            .input_bound
            .checked_mul(self.input_price_microusd)
            .and_then(|input| {
                max_out
                    .checked_mul(self.output_price_microusd)
                    .and_then(|output| input.checked_add(output))
            })
            .ok_or(ConfigError::CeilingOverflow {
                dim: DimKey::CostMicroUsd,
            })?;
        Ok(Budget::from_dims([
            (DimKey::Tokens, tokens),
            (DimKey::CostMicroUsd, cost),
        ]))
    }

    /// What `usage` costs at this config's prices: `tokens` = in + out,
    /// `cost_microusd` = in × input price + out × output price. Saturates at
    /// `u64::MAX`, which no real call reaches.
    #[must_use]
    pub fn price(&self, usage: Usage) -> Consumption {
        let tokens = usage.input_tokens.saturating_add(usage.output_tokens);
        let cost = usage
            .input_tokens
            .saturating_mul(self.input_price_microusd)
            .saturating_add(
                usage
                    .output_tokens
                    .saturating_mul(self.output_price_microusd),
            );
        Consumption::from_dims([(DimKey::Tokens, tokens), (DimKey::CostMicroUsd, cost)])
    }
}

/// Why an [`AnthropicDriver`] could not be built from its config.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The API key's environment variable is unset, empty, or not Unicode.
    #[error("environment variable `{var}` is unset or empty")]
    MissingApiKey {
        /// The variable that was read.
        var: String,
    },
    /// A ceiling sum or product does not fit in `u64`.
    #[error("ceiling overflows u64 along `{dim}`")]
    CeilingOverflow {
        /// The dimension that overflowed.
        dim: DimKey,
    },
    /// The base URL does not parse.
    #[error("base URL `{url}` is invalid: {reason}")]
    BadBaseUrl {
        /// The URL as configured.
        url: String,
        /// The parser's complaint.
        reason: String,
    },
    /// The HTTP client could not be built (a TLS backend problem).
    #[error("HTTP client: {0}")]
    Client(String),
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
    endpoint: reqwest::Url,
    client: reqwest::Client,
    flights: Mutex<Flights>,
}

/// The calls `abandon` can reach. Ordered maps, not hashed: the workspace
/// forbids the latter, and these are never large.
#[derive(Debug, Default)]
struct Flights {
    /// corr → the signal `abandon` pulls. An entry lives exactly as long as
    /// the call: [`Flight`] removes it on drop.
    open: BTreeMap<Corr, Arc<Notify>>,
    /// Abandoned before the driver saw the request. The kernel computes
    /// phase two of `cancel` from the sender's open corrs, which include a
    /// delivery still queued for this driver, so `abandon` can precede
    /// `handle`. A corr is never reused within a run, so remembering it is
    /// safe; the call it names answers `transport` without going out.
    abandoned_early: BTreeSet<Corr>,
}

/// One call's registration, removed when the call ends however it ends —
/// including a driver task torn down at shutdown.
struct Flight {
    inner: Arc<Inner>,
    corr: Corr,
    notify: Arc<Notify>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.inner.lock_flights().open.remove(&self.corr);
    }
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
        let raw = format!("{}/v1/messages", config.base_url.trim_end_matches('/'));
        let endpoint = reqwest::Url::parse(&raw).map_err(|e| ConfigError::BadBaseUrl {
            url: config.base_url.clone(),
            reason: e.to_string(),
        })?;
        // No client-side timeout: the driver's own `tokio::time::timeout`
        // is the one that turns into `error.transport`.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| ConfigError::Client(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                ceiling,
                endpoint,
                client,
                flights: Mutex::new(Flights::default()),
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
        self.inner.lock_flights().open.len()
    }
}

impl Driver for AnthropicDriver {
    fn handle(&self, request: Delivery) -> BoxFuture<(Vec<u8>, Consumption)> {
        // Registered now, not when the future is first polled: `abandon`
        // may arrive in between, and it must find the entry.
        let flight = self.inner.enter(request.corr);
        Box::pin(async move {
            let (reply, consumed) = flight.inner.answer(&request.payload, &flight.notify).await;
            drop(flight);
            (encode(&reply), consumed)
        })
    }

    fn abandon(&self, corr: Corr) {
        let mut flights = self.inner.lock_flights();
        match flights.open.get(&corr) {
            // A permit is stored if nobody is waiting yet, so an abandon that
            // lands before the `select!` is reached still takes effect.
            Some(notify) => notify.notify_one(),
            None => {
                flights.abandoned_early.insert(corr);
            }
        }
    }
}

/// What one HTTP exchange produced, before mapping.
enum Exchange {
    /// The provider answered; here is the status and the body.
    Answered { status: u16, body: Vec<u8> },
    /// It did not: connection, timeout, or abandon.
    Lost(String),
}

impl Inner {
    fn lock_flights(&self) -> std::sync::MutexGuard<'_, Flights> {
        self.flights.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn enter(self: &Arc<Self>, corr: Corr) -> Flight {
        let notify = Arc::new(Notify::new());
        {
            let mut flights = self.lock_flights();
            if flights.abandoned_early.remove(&corr) {
                notify.notify_one();
            }
            flights.open.insert(corr, Arc::clone(&notify));
        }
        Flight {
            inner: Arc::clone(self),
            corr,
            notify,
        }
    }

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
        let body = match wire::to_provider(&request, &self.config.model, self.config.max_max_tokens)
        {
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

        let exchange = tokio::select! {
            exchange = self.post(bytes) => exchange,
            () = notify.notified() => Exchange::Lost("abandoned by cancel".to_owned()),
        };
        match exchange {
            Exchange::Lost(message) => self.refuse(ErrorKind::Transport, message),
            Exchange::Answered { status, body } if (200..300).contains(&status) => {
                self.map_answer(&body)
            }
            Exchange::Answered { status, body } => {
                self.refuse(ErrorKind::Provider, provider_message(status, &body))
            }
        }
    }

    async fn post(&self, bytes: Vec<u8>) -> Exchange {
        let send = self
            .client
            .post(self.endpoint.clone())
            .header("content-type", "application/json")
            .header("x-api-key", self.config.api_key.expose())
            .header("anthropic-version", API_VERSION)
            .body(bytes)
            .send();
        let response = match tokio::time::timeout(self.config.timeout, send).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Exchange::Lost(format!("request failed: {}", without_url(e))),
            Err(_elapsed) => {
                return Exchange::Lost(format!("no answer within {:?}", self.config.timeout))
            }
        };
        let status = response.status().as_u16();
        match tokio::time::timeout(self.config.timeout, response.bytes()).await {
            Ok(Ok(body)) => Exchange::Answered {
                status,
                body: body.to_vec(),
            },
            Ok(Err(e)) => Exchange::Lost(format!("body failed: {}", without_url(e))),
            Err(_elapsed) => Exchange::Lost(format!("no body within {:?}", self.config.timeout)),
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
                let reply = self.error_reply(
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
            Err(err) => (
                self.error_reply(err.kind, err.message, model, usage),
                consumed,
            ),
        }
    }

    /// A reply that says no, with nothing consumed: nothing was sent, or
    /// nothing was billed.
    fn refuse(&self, kind: ErrorKind, message: String) -> (ModelReply, Consumption) {
        let model = Some(self.config.model.clone());
        (
            self.error_reply(kind, message, model, Usage::default()),
            Consumption::none(),
        )
    }

    fn error_reply(
        &self,
        kind: ErrorKind,
        message: String,
        model: Option<String>,
        usage: Usage,
    ) -> ModelReply {
        ModelReply {
            v: VERSION,
            model,
            content: Vec::new(),
            stop: StopReason::Error(ModelError { kind, message }),
            usage,
        }
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

/// reqwest's `Display` includes the URL, which may carry a query string the
/// operator considers sensitive; the base URL is config, not something to
/// echo into every agent's mailbox.
fn without_url(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// A reply, as bytes. Serializing these types cannot fail in practice
/// (`Value` inputs came from JSON); if it ever does, the reply says so
/// instead of being empty.
fn encode(reply: &ModelReply) -> Vec<u8> {
    serde_json::to_vec(reply).unwrap_or_else(|_| {
        br#"{"v":1,"content":[],"stop":{"error":{"kind":"provider","message":"reply could not be serialized"}},"usage":{"input_tokens":0,"output_tokens":0}}"#.to_vec()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

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
        c.input_bound = u64::MAX >> 1;
        c.max_max_tokens = 1;
        assert!(matches!(
            c.ceiling(),
            Err(ConfigError::CeilingOverflow {
                dim: DimKey::CostMicroUsd
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
        let huge = config().price(Usage {
            input_tokens: u64::MAX,
            output_tokens: 1,
        });
        assert_eq!(huge.get(&DimKey::Tokens), Some(u64::MAX));
        assert_eq!(huge.get(&DimKey::CostMicroUsd), Some(u64::MAX));
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
    }

    #[test]
    fn the_api_key_is_redacted_and_read_from_the_environment() {
        assert_eq!(
            format!("{:?}", ApiKey::new("sk-secret")),
            "ApiKey(<redacted>)"
        );
        assert!(!format!("{:?}", config()).contains("k\""));
        let err = ApiKey::from_env("TAU_TEST_NO_SUCH_VARIABLE_7f3a").unwrap_err();
        assert!(err.to_string().contains("TAU_TEST_NO_SUCH_VARIABLE_7f3a"));
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

    #[test]
    fn encode_never_returns_nothing() {
        let reply = ModelReply {
            v: VERSION,
            model: None,
            content: vec![],
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let parsed: ModelReply = serde_json::from_slice(&encode(&reply)).unwrap();
        assert_eq!(parsed, reply);
    }
}
