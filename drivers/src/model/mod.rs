//! Model drivers: a `send` to a model capability carries a
//! [`ModelRequest`](tau_kernel::bridge::ModelRequest) and the reply is a
//! [`ModelReply`](tau_kernel::bridge::ModelReply), in JSON, per ADR-0006.
//!
//! Every driver here is registered behind a *ceiling* derived from its own
//! configuration (ADR-0006 §7), refuses what it cannot honour instead of
//! degrading silently, and reports its consumption honestly, error or not.
//! What is the same for every provider — the ceiling arithmetic, the API
//! key, the reasons a config is rejected — lives here; the wire format and
//! the transport live with each driver.

use std::fmt;

use tau_kernel::abi::DimKey;

pub mod ceiling;

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "openai")]
pub mod openai;

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

    /// The key itself, for the one header that carries it.
    #[cfg(any(feature = "anthropic", feature = "openai"))]
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Why a model driver could not be built from its config.
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_api_key_is_redacted_and_read_from_the_environment() {
        assert_eq!(
            format!("{:?}", ApiKey::new("sk-secret")),
            "ApiKey(<redacted>)"
        );
        let err = ApiKey::from_env("TAU_TEST_NO_SUCH_VARIABLE_7f3a").unwrap_err();
        assert!(err.to_string().contains("TAU_TEST_NO_SUCH_VARIABLE_7f3a"));
    }
}
