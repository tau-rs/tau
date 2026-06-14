//! Environment-variable credential provider. This is tau's historical,
//! zero-config default: read the credential from the declared env var.

use tau_ports::credential::{CredentialProvider, CredentialRequest, ResolvedCredential, Secret};
use tau_ports::CredentialError;

/// Resolves a credential by reading the request's `env_name` from a
/// lookup function (process environment by default). An absent or empty
/// variable resolves to `Ok(None)` so the chain continues.
pub struct EnvProvider<F = fn(&str) -> Option<String>> {
    lookup: F,
}

impl EnvProvider<fn(&str) -> Option<String>> {
    /// A provider that reads the real process environment.
    pub fn from_process_env() -> Self {
        Self {
            lookup: |name| std::env::var(name).ok(),
        }
    }
}

impl<F> EnvProvider<F>
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    /// A provider that reads from a custom lookup (used in tests).
    pub fn new(lookup: F) -> Self {
        Self { lookup }
    }
}

impl<F> CredentialProvider for EnvProvider<F>
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn name(&self) -> &str {
        "env"
    }

    async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<ResolvedCredential>, CredentialError> {
        let Some(var) = req.env_name.as_deref() else {
            return Ok(None);
        };
        match (self.lookup)(var) {
            Some(v) if !v.is_empty() => Ok(Some(ResolvedCredential::new(
                Secret::from_bytes(v.into_bytes()),
                "env",
            ))),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::credential::CredentialId;

    fn req(id: &str, env: &str) -> CredentialRequest {
        CredentialRequest::new(CredentialId::parse(id).unwrap()).with_env_name(env)
    }

    #[tokio::test]
    async fn resolves_present_var() {
        let p = EnvProvider::new(|n| (n == "ANTHROPIC_API_KEY").then(|| "sk-ant-z".to_string()));
        let got = p
            .resolve(&req("anthropic_api_key", "ANTHROPIC_API_KEY"))
            .await
            .unwrap();
        assert_eq!(got.unwrap().secret.expose_str().unwrap(), "sk-ant-z");
    }

    #[tokio::test]
    async fn absent_var_is_none() {
        let p = EnvProvider::new(|_| None);
        assert!(p.resolve(&req("x", "MISSING")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_var_is_none() {
        let p = EnvProvider::new(|_| Some(String::new()));
        assert!(p.resolve(&req("x", "EMPTY")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn no_env_hint_is_none() {
        let p = EnvProvider::new(|_| Some("v".to_string()));
        let r = CredentialRequest::new(CredentialId::parse("x").unwrap());
        assert!(p.resolve(&r).await.unwrap().is_none());
    }
}
