//! Dyn-compatible wrapper + the [`CredentialChain`] combinator.
//!
//! `CredentialProvider` uses native `async fn in trait`, which is not
//! dyn-compatible. [`DynCredentialProvider`] is the object-safe shim
//! (boxed, non-`Send` futures — matching `tau-runtime-core`'s
//! `BoxFuture` at `builder.rs:84`), with a blanket impl for every
//! `CredentialProvider`. The chain stores `Arc<dyn DynCredentialProvider>`.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

use super::{CredentialProvider, CredentialRequest, ResolvedCredential};
use crate::error::CredentialError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Object-safe wrapper for [`CredentialProvider`]. Authors implement
/// `CredentialProvider`; the blanket impl below handles the dyn-cast.
pub trait DynCredentialProvider: Send + Sync {
    /// Provider name (matches [`CredentialProvider::name`]).
    fn name(&self) -> &str;

    /// Boxed-future wrapper for [`CredentialProvider::resolve`].
    fn resolve<'a>(
        &'a self,
        req: &'a CredentialRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedCredential>, CredentialError>>;
}

impl<T: CredentialProvider + 'static> DynCredentialProvider for T {
    fn name(&self) -> &str {
        CredentialProvider::name(self)
    }

    fn resolve<'a>(
        &'a self,
        req: &'a CredentialRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedCredential>, CredentialError>> {
        Box::pin(CredentialProvider::resolve(self, req))
    }
}

/// A composite provider that walks members in declared order. First
/// `Ok(Some)` wins; `Ok(None)` continues; `Err` fails fast.
pub struct CredentialChain {
    members: Vec<Arc<dyn DynCredentialProvider>>,
}

impl CredentialChain {
    /// An empty chain (resolves everything to `Ok(None)`).
    pub fn new() -> Self {
        Self {
            members: Vec::new(),
        }
    }

    /// Builder-style: append a provider.
    pub fn with(mut self, provider: Arc<dyn DynCredentialProvider>) -> Self {
        self.members.push(provider);
        self
    }

    /// Append a provider in place.
    pub fn push(&mut self, provider: Arc<dyn DynCredentialProvider>) {
        self.members.push(provider);
    }

    /// Number of members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the chain has no members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

impl Default for CredentialChain {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for CredentialChain {
    /// Lists member provider names only — never any resolved secret.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CredentialChain")
            .field(
                "members",
                &self.members.iter().map(|m| m.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl CredentialProvider for CredentialChain {
    fn name(&self) -> &str {
        "chain"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        for member in &self.members {
            match member.resolve(req).await {
                Ok(Some(resolved)) => return Ok(Some(resolved)),
                Ok(None) => continue,
                Err(err) => return Err(err), // fail-fast
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{BakedProvider, CredentialId, CredentialProvider, CredentialRequest};
    use crate::error::CredentialError;
    use alloc::sync::Arc;

    /// Provider that always errors — to test fail-fast.
    struct ErringProvider;
    impl CredentialProvider for ErringProvider {
        fn name(&self) -> &str {
            "erring"
        }
        async fn resolve(
            &self,
            _req: &CredentialRequest,
        ) -> Result<Option<ResolvedCredential>, CredentialError> {
            Err(CredentialError::ProviderUnavailable {
                reason: "boom".into(),
                provider: "erring".into(),
            })
        }
    }

    fn id(s: &str) -> CredentialId {
        CredentialId::parse(s).unwrap()
    }

    #[tokio::test]
    async fn chain_fails_fast_on_err_and_skips_later_providers() {
        // A provider after the erroring one that WOULD satisfy the request.
        let after = BakedProvider::new().with(id("k"), b"should-not-be-reached".to_vec());
        let chain = CredentialChain::new()
            .with(Arc::new(ErringProvider))
            .with(Arc::new(after));
        let req = CredentialRequest::new(id("k"));
        let result = CredentialProvider::resolve(&chain, &req).await;
        assert!(
            matches!(result, Err(CredentialError::ProviderUnavailable { .. })),
            "chain must fail-fast on the erroring provider and never reach `after`",
        );
    }

    #[tokio::test]
    async fn chain_returns_first_match_not_later() {
        let first = BakedProvider::new().with(id("k"), b"first".to_vec());
        let second = BakedProvider::new().with(id("k"), b"second".to_vec());
        let chain = CredentialChain::new()
            .with(Arc::new(first))
            .with(Arc::new(second));
        let got = CredentialProvider::resolve(&chain, &CredentialRequest::new(id("k")))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.secret.expose_bytes(), b"first");
    }
}
