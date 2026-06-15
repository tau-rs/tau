//! Build a [`CredentialChain`] from validated scope config.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use tau_pkg::scope_credentials::{CredentialsChainConfig, ProviderConfig};
use tau_ports::credential::CredentialChain;

use super::{EnvProvider, FileProvider};

/// Construct a runnable [`CredentialChain`] from validated config.
/// `env` members read the real process environment.
pub fn build_chain(config: &CredentialsChainConfig) -> CredentialChain {
    let mut chain = CredentialChain::new();
    for provider in &config.chain {
        match provider {
            ProviderConfig::Env => {
                chain.push(Arc::new(EnvProvider::from_process_env()));
            }
            ProviderConfig::File { dir, key_map } => {
                let key_map: BTreeMap<String, String> = key_map.clone();
                chain.push(Arc::new(FileProvider::new(PathBuf::from(dir), key_map)));
            }
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds_single_env_member() {
        let chain = build_chain(&CredentialsChainConfig::default());
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn env_then_file_builds_two_members() {
        let mut key_map = BTreeMap::new();
        key_map.insert("k".to_string(), "f".to_string());
        let cfg = CredentialsChainConfig {
            chain: vec![
                ProviderConfig::Env,
                ProviderConfig::File {
                    dir: "/tmp".to_string(),
                    key_map,
                },
            ],
        };
        assert_eq!(build_chain(&cfg).len(), 2);
    }
}
