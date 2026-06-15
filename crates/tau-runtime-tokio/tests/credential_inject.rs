//! Verifies the chain resolves the value the host would inject into a
//! child's env, and that `build_chain` assembles the configured
//! providers. Does not spawn a child process — it asserts the resolution
//! and wiring that the resolve-then-inject bridge depends on.

use std::collections::BTreeMap;
use std::sync::Arc;

use tau_ports::credential::{CredentialChain, CredentialId, CredentialProvider, CredentialRequest};
use tau_runtime_tokio::credentials::{build_chain, EnvProvider, FileProvider};

/// Build a chain [env, file] where env misses and file hits, then assert
/// the secret resolves — this is the value the host injects into a child.
#[tokio::test]
async fn file_secret_resolves_for_injection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("anthropic-key"), b"sk-ant-injected\n").unwrap();

    let mut key_map = BTreeMap::new();
    key_map.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());

    let chain = CredentialChain::new()
        .with(Arc::new(EnvProvider::new(|_| None)))
        .with(Arc::new(FileProvider::new(
            dir.path().to_path_buf(),
            key_map,
        )));

    let req = CredentialRequest::new(CredentialId::parse("anthropic_api_key").unwrap())
        .with_env_name("ANTHROPIC_API_KEY");
    let resolved = chain
        .resolve(&req)
        .await
        .unwrap()
        .expect("file should resolve");
    assert_eq!(resolved.secret.expose_str().unwrap(), "sk-ant-injected");
    assert_eq!(resolved.source, "file");

    // build_chain over the same logical config also yields a 2-member chain.
    use tau_pkg::scope_credentials::{CredentialsChainConfig, ProviderConfig};
    let mut km = BTreeMap::new();
    km.insert("anthropic_api_key".to_string(), "anthropic-key".to_string());
    let cfg = CredentialsChainConfig {
        chain: vec![
            ProviderConfig::Env,
            ProviderConfig::File {
                dir: dir.path().display().to_string(),
                key_map: km,
            },
        ],
    };
    assert_eq!(build_chain(&cfg).len(), 2);
}
