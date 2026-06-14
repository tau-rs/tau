//! In-memory credential provider. `no_std`-friendly; deterministic.
//! Doubles as a test provider and the seed for embedded/wasm hosts.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::{CredentialId, CredentialProvider, CredentialRequest, ResolvedCredential, Secret};
use crate::error::CredentialError;

/// A credential provider backed by an in-memory map.
#[derive(Default)]
pub struct BakedProvider {
    entries: BTreeMap<CredentialId, Vec<u8>>,
}

impl BakedProvider {
    /// An empty provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style: insert an entry.
    pub fn with(mut self, id: CredentialId, value: impl Into<Vec<u8>>) -> Self {
        self.entries.insert(id, value.into());
        self
    }

    /// Insert an entry in place.
    pub fn insert(&mut self, id: CredentialId, value: impl Into<Vec<u8>>) {
        self.entries.insert(id, value.into());
    }
}

impl CredentialProvider for BakedProvider {
    fn name(&self) -> &str {
        "baked"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        Ok(self
            .entries
            .get(&req.id)
            .map(|v| ResolvedCredential::new(Secret::from_bytes(v.clone()), "baked")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> CredentialId {
        CredentialId::parse(s).unwrap()
    }

    #[tokio::test]
    async fn resolves_present_key() {
        let p = BakedProvider::new().with(id("anthropic_api_key"), b"sk-ant-x".to_vec());
        let req = CredentialRequest::new(id("anthropic_api_key"));
        let got = p.resolve(&req).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_bytes(), b"sk-ant-x");
        assert_eq!(got.source, "baked");
        assert_eq!(got.expires_at, None);
    }

    #[tokio::test]
    async fn absent_key_is_none() {
        let p = BakedProvider::new();
        let req = CredentialRequest::new(id("missing"));
        assert!(p.resolve(&req).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn chain_walks_to_first_match() {
        use super::super::CredentialChain;
        let empty = BakedProvider::new();
        let full = BakedProvider::new().with(id("k"), b"v".to_vec());
        let chain = CredentialChain::new()
            .with(alloc::sync::Arc::new(empty))
            .with(alloc::sync::Arc::new(full));
        let req = CredentialRequest::new(id("k"));
        let got = chain.resolve(&req).await.unwrap().unwrap();
        assert_eq!(got.secret.expose_bytes(), b"v");
    }
}
