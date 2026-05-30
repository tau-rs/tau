//! Per-concern error enums for `tau-ports`.
//!
//! Each error type is `#[non_exhaustive]` so additive variants are non-breaking.
//! All errors derive `Debug + Error + Clone + PartialEq + Eq`; tests with
//! free-form `String` fields use `matches!()` to avoid brittle wording
//! comparisons.
//!
//! `LlmError`, `ToolError`, `StorageError`, and `CapabilityError` are the
//! per-trait error types. `NamespaceError` and `KeyError` are the validation
//! errors for the `Namespace` and `Key` newtypes.

use alloc::format;
use alloc::string::{String, ToString};

use thiserror::Error;

/// Errors returned by `crate::llm::LlmBackend` implementations.
///
/// Variants partition transport, protocol, and provider failures so adapters
/// can map provider-specific errors to a uniform surface. Use
/// [`LlmError::is_retryable`] for the default retry-policy hint.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LlmError {
    /// The request was malformed or violated provider constraints.
    #[error("invalid request: {reason}")]
    InvalidRequest {
        /// Human-readable reason the request was rejected.
        reason: String,
    },
    /// The provider rate-limited the caller.
    #[error("rate limited: retry after {retry_after_seconds:?}s")]
    RateLimited {
        /// Suggested wait, in seconds, before retrying. `None` if the provider
        /// did not signal one.
        retry_after_seconds: Option<u32>,
    },
    /// Authentication failed (bad credentials, missing API key, etc.).
    #[error("authentication failed: {message}")]
    Auth {
        /// Provider-supplied or adapter-synthesized auth-failure detail.
        message: String,
    },
    /// Network/transport failure reaching the provider.
    #[error("transport: {message}")]
    Transport {
        /// Description of the underlying transport failure.
        message: String,
    },
    /// Mid-stream error (only emitted from `CompletionStream` items, never
    /// from the return of `complete()`).
    #[error("stream error: {message}")]
    Stream {
        /// Description of the stream-time failure.
        message: String,
    },
    /// The provider returned a server-side error not covered by other variants.
    #[error("provider error: {message}")]
    Provider {
        /// Provider-supplied error detail.
        message: String,
    },
    /// The requested feature is not supported by this backend.
    #[error("unsupported: {what}")]
    Unsupported {
        /// Description of the unsupported feature or option.
        what: String,
    },
    /// Plugin internal error.
    ///
    /// See: [escape-hatches.md#llmerror-internal](../docs/explanation/escape-hatches.md#llmerror-internal).
    #[error("internal: {message}")]
    Internal {
        /// Free-form internal-error message; not part of the stable API surface.
        message: String,
    },
}

impl LlmError {
    /// Heuristic: is the error likely transient? Default-policy hint;
    /// nuanced policies should match on variants directly.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. }
                | LlmError::Transport { .. }
                | LlmError::Stream { .. }
                | LlmError::Provider { .. },
        )
    }
}

/// Errors returned by [`crate::storage::Storage`] implementations.
///
/// Backend-time rejection of a `Namespace`/`Key` is distinct from
/// construction-time validation: see [`NamespaceError`] / [`KeyError`] for the
/// latter. Use [`StorageError::is_retryable`] for the default retry-policy
/// hint.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StorageError {
    /// Backend rejected the namespace (length cap, reserved prefix, charset).
    #[error("invalid namespace: {reason}")]
    InvalidNamespace {
        /// Backend-supplied reason the namespace was rejected.
        reason: String,
    },
    /// Backend rejected the key (length, charset, reserved prefix).
    #[error("invalid key: {reason}")]
    InvalidKey {
        /// Backend-supplied reason the key was rejected.
        reason: String,
    },
    /// Backend is currently unavailable (connection refused, degraded mode).
    #[error("unavailable: {message}")]
    Unavailable {
        /// Description of the availability failure.
        message: String,
    },
    /// Operation timed out.
    #[error("timeout")]
    Timeout,
    /// The requested operation is not supported by this backend.
    #[error("unsupported: {what}")]
    Unsupported {
        /// Description of the unsupported operation.
        what: String,
    },
    /// Plugin internal error.
    ///
    /// See: [escape-hatches.md#storageerror-internal](../docs/explanation/escape-hatches.md#storageerror-internal).
    #[error("internal: {message}")]
    Internal {
        /// Free-form internal-error message; not part of the stable API surface.
        message: String,
    },
}

impl StorageError {
    /// Heuristic: is the error likely transient? Default-policy hint;
    /// nuanced policies should match on variants directly.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            StorageError::Unavailable { .. } | StorageError::Timeout,
        )
    }
}

/// Errors returned by [`crate::capability_gate::CapabilityGate`] implementations.
///
/// Stable as of v0.1 of the sandboxing sub-project. Variant evolution
/// is handled by `#[non_exhaustive]` at the enum level and on each
/// struct-style variant.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CapabilityError {
    /// The adapter is not usable on this host (probe returned Unavailable).
    #[error("sandbox unavailable: {reason}")]
    Unavailable {
        /// Reason from [`crate::capability_gate::CapabilityProbe::Unavailable`].
        reason: String,
    },
    /// The plan requires a capability shape this adapter does not support.
    #[error("sandbox: unsupported shape {shape:?}")]
    ShapeUnsupported {
        /// The shape that was rejected.
        shape: tau_domain::CapabilityShape,
    },
    /// The adapter could not apply sandbox enforcement to the spawn.
    /// (Examples: landlock syscall failed, seccomp filter compile failed,
    /// `docker run` returned non-zero.)
    #[error("sandbox wrap-spawn failed: {message}")]
    WrapFailed {
        /// Free-form diagnostic; not part of the stable API surface.
        message: String,
    },
    /// Runtime sandbox violation reported by the kernel
    /// (SIGSYS from seccomp, EACCES from landlock, etc).
    #[error("sandbox violation: {detail}")]
    Violation {
        /// Detail about the violating syscall / path / host.
        detail: String,
    },
    /// The requested feature is not supported by this sandbox.
    #[error("sandbox unsupported: {what}")]
    Unsupported {
        /// Description of the unsupported feature.
        what: String,
    },
    /// A configured resource limit was exceeded.
    #[error("sandbox limit exceeded: {limit}")]
    LimitExceeded {
        /// Identifier of the limit that was exceeded.
        limit: String,
    },
    /// Sandbox proxy failed to set up or relay a connection.
    /// Includes proxy task spawn errors, allow-list violations surfaced
    /// from the bridge, etc.
    #[error("sandbox proxy: {message}")]
    Proxy {
        /// Free-form message including the failure context.
        message: String,
    },
    /// Plugin internal error.
    ///
    /// See: [escape-hatches.md#sandboxerror-internal](../docs/explanation/escape-hatches.md#sandboxerror-internal).
    #[error("sandbox internal: {message}")]
    Internal {
        /// Free-form internal-error message; not part of the stable API surface.
        message: String,
    },
}

/// Errors returned by [`crate::tool::Tool`] implementations.
///
/// `ToolError` composes [`LlmError`] and [`StorageError`] via `#[from]` so
/// tools that internally drive an LLM or storage backend can propagate via
/// `?`. Direction is unidirectional (Tool → LlmError/StorageError, not vice
/// versa) to avoid cycles and preserve layering. Use
/// [`ToolError::is_retryable`] for the default retry-policy hint; it
/// delegates to the inner predicate for `Llm`/`Storage` and returns `false`
/// otherwise.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ToolError {
    /// Caller passed arguments the tool rejected.
    #[error("bad args: {reason}")]
    BadArgs {
        /// Human-readable reason the arguments were rejected.
        reason: String,
    },
    /// The tool's session is no longer usable; the caller should reopen it.
    #[error("session unusable: {reason}")]
    SessionDead {
        /// Reason the session was declared dead.
        reason: String,
    },
    /// The tool exceeded its deadline.
    #[error("deadline exceeded")]
    DeadlineExceeded,
    /// The tool requires a capability that the caller is not granted.
    #[error("capability denied: {capability}")]
    CapabilityDenied {
        /// Identifier of the denied capability.
        capability: String,
    },
    /// Underlying LLM call failed (for tools that internally use an LLM).
    #[error("llm: {0}")]
    Llm(#[from] LlmError),
    /// Underlying storage operation failed.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Plugin internal error.
    ///
    /// See: [escape-hatches.md#toolerror-internal](../docs/explanation/escape-hatches.md#toolerror-internal).
    #[error("internal: {message}")]
    Internal {
        /// Free-form internal-error message; not part of the stable API surface.
        message: String,
    },
}

impl ToolError {
    /// Heuristic: is the error likely transient? Most `ToolError` variants
    /// are NOT retryable — `SessionDead` means reopen the session, `BadArgs`
    /// is permanent, etc. Composed `Llm`/`Storage` errors delegate to their
    /// inner predicate.
    pub fn is_retryable(&self) -> bool {
        match self {
            ToolError::Llm(e) => e.is_retryable(),
            ToolError::Storage(e) => e.is_retryable(),
            _ => false,
        }
    }
}

/// Validation errors for [`crate::storage::Namespace`].
///
/// # Example
///
/// ```
/// use tau_ports::{NamespaceError, storage::Namespace};
///
/// let err = Namespace::try_new("").unwrap_err();
/// assert_eq!(err, NamespaceError::Empty);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NamespaceError {
    /// The input was empty.
    #[error("namespace is empty")]
    Empty,
    /// The input exceeded the byte cap.
    #[error("namespace exceeds {max} bytes: got {got}")]
    TooLong {
        /// Maximum permitted length, in bytes.
        max: usize,
        /// Actual length of the input, in bytes.
        got: usize,
    },
    /// The input contained a NUL byte or control character.
    #[error("namespace contains invalid byte (NUL or control char) at position {pos}")]
    InvalidByte {
        /// Byte position in the input.
        pos: usize,
    },
}

/// Validation errors for [`crate::storage::Key`].
///
/// # Example
///
/// ```
/// use tau_ports::{KeyError, storage::Key};
///
/// let err = Key::try_new("").unwrap_err();
/// assert_eq!(err, KeyError::Empty);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyError {
    /// The input was empty.
    #[error("key is empty")]
    Empty,
    /// The input exceeded the byte cap.
    #[error("key exceeds {max} bytes: got {got}")]
    TooLong {
        /// Maximum permitted length, in bytes.
        max: usize,
        /// Actual length of the input, in bytes.
        got: usize,
    },
    /// The input contained a NUL byte.
    #[error("key contains NUL byte at position {pos}")]
    InvalidByte {
        /// Byte position in the input.
        pos: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_is_retryable() {
        assert!(LlmError::RateLimited {
            retry_after_seconds: None
        }
        .is_retryable());
        assert!(LlmError::RateLimited {
            retry_after_seconds: Some(2)
        }
        .is_retryable());
        assert!(LlmError::Transport {
            message: "tcp reset".into()
        }
        .is_retryable());
        assert!(LlmError::Stream {
            message: "eof".into()
        }
        .is_retryable());
        assert!(LlmError::Provider {
            message: "5xx".into()
        }
        .is_retryable());

        assert!(!LlmError::InvalidRequest {
            reason: "bad".into()
        }
        .is_retryable());
        assert!(!LlmError::Auth {
            message: "no key".into()
        }
        .is_retryable());
        assert!(!LlmError::Unsupported {
            what: "embeds".into()
        }
        .is_retryable());
        assert!(!LlmError::Internal {
            message: "x".into()
        }
        .is_retryable());
    }

    #[test]
    fn storage_is_retryable() {
        assert!(StorageError::Unavailable {
            message: "down".into()
        }
        .is_retryable());
        assert!(StorageError::Timeout.is_retryable());

        assert!(!StorageError::InvalidNamespace { reason: "r".into() }.is_retryable());
        assert!(!StorageError::InvalidKey { reason: "r".into() }.is_retryable());
        assert!(!StorageError::Unsupported { what: "txn".into() }.is_retryable());
        assert!(!StorageError::Internal {
            message: "x".into()
        }
        .is_retryable());
    }

    #[test]
    fn tool_is_retryable_delegates() {
        // Composed Llm/Storage variants delegate to their inner predicate.
        assert!(ToolError::Llm(LlmError::Transport {
            message: "x".into()
        })
        .is_retryable());
        assert!(!ToolError::Llm(LlmError::Auth {
            message: "x".into()
        })
        .is_retryable());
        assert!(ToolError::Storage(StorageError::Timeout).is_retryable());
        assert!(
            !ToolError::Storage(StorageError::InvalidKey { reason: "x".into() }).is_retryable()
        );

        // All non-composed variants are NOT retryable.
        assert!(!ToolError::BadArgs { reason: "r".into() }.is_retryable());
        assert!(!ToolError::SessionDead { reason: "r".into() }.is_retryable());
        assert!(!ToolError::DeadlineExceeded.is_retryable());
        assert!(!ToolError::CapabilityDenied {
            capability: "fs".into()
        }
        .is_retryable());
        assert!(!ToolError::Internal {
            message: "x".into()
        }
        .is_retryable());
    }

    #[test]
    fn tool_from_llm_error() {
        fn op() -> Result<(), ToolError> {
            Err(LlmError::Transport {
                message: "oops".into(),
            })?;
            Ok(())
        }
        let err = op().unwrap_err();
        assert_eq!(
            err,
            ToolError::Llm(LlmError::Transport {
                message: "oops".into()
            }),
        );
        assert!(err.is_retryable());
    }

    #[test]
    fn tool_from_storage_error() {
        fn op() -> Result<(), ToolError> {
            Err(StorageError::Timeout)?;
            Ok(())
        }
        let err = op().unwrap_err();
        assert_eq!(err, ToolError::Storage(StorageError::Timeout));
        assert!(err.is_retryable());
    }

    #[test]
    fn sandbox_error_proxy_renders() {
        let e = CapabilityError::Proxy {
            message: "proxy task spawn failed".to_string(),
        };
        assert_eq!(format!("{e}"), "sandbox proxy: proxy task spawn failed");
    }
}
