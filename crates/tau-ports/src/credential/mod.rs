//! Credential provider chain port (β.5).
//!
//! [`CredentialProvider`] is the Strategy: each provider knows how to
//! resolve a credential from one source (env, file, baked, …).
//! [`CredentialChain`] is the composite that walks providers in order.
//!
//! The port uses native `async fn in trait` (per ADR-0003); the
//! dyn-compatible shim lives in [`chain`] and mirrors the boxed-future
//! pattern from `tau-runtime-core/src/builder.rs`.

mod baked;
mod chain;
pub mod id;
pub mod secret;

pub use baked::BakedProvider;
pub use chain::{CredentialChain, DynCredentialProvider};
pub use id::{CredentialId, InvalidCredentialId};
pub use secret::Secret;

use alloc::string::String;

use crate::error::CredentialError;

/// What a consumer wants resolved.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CredentialRequest {
    /// The logical credential id the chain resolves.
    pub id: CredentialId,
    /// The environment-variable name to read, used by the host's env
    /// provider (β.5 PR-2). Other providers ignore it.
    pub env_name: Option<String>,
}

impl CredentialRequest {
    /// Construct a request for the given id with no provider hints.
    pub fn new(id: CredentialId) -> Self {
        Self { id, env_name: None }
    }

    /// Attach the environment-variable name hint (for the env provider).
    pub fn with_env_name(mut self, env_name: impl Into<String>) -> Self {
        self.env_name = Some(env_name.into());
        self
    }
}

/// A successfully resolved credential.
#[non_exhaustive]
pub struct ResolvedCredential {
    /// The secret value.
    pub secret: Secret,
    /// Optional Unix-millis expiry for rotating providers. `None` = no
    /// known expiry; the consumer re-resolves past expiry.
    pub expires_at: Option<i64>,
    /// Which provider satisfied the request (for tracing/audit).
    pub source: &'static str,
}

impl ResolvedCredential {
    /// Construct a resolved credential with no expiry.
    pub fn new(secret: Secret, source: &'static str) -> Self {
        Self {
            secret,
            expires_at: None,
            source,
        }
    }

    /// Set the expiry (Unix millis).
    pub fn with_expiry(mut self, expires_at: i64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
}

/// A strategy for resolving a credential from one source.
///
/// `Ok(Some(_))` = resolved. `Ok(None)` = not here, try the next
/// provider. `Err(_)` = this provider owns the request but failed.
#[allow(async_fn_in_trait)]
pub trait CredentialProvider: Send + Sync {
    /// Stable provider name (e.g. `"env"`, `"file"`, `"baked"`).
    fn name(&self) -> &str;

    /// Resolve the requested credential.
    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError>;
}
