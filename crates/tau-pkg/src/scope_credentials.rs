//! `[credentials]` chain configuration (β.5), stored in scope/home
//! `config.toml`. Deployment-specific: the same bundle resolves
//! credentials from env locally, files in k8s, or (later) Vault in prod.
//!
//! Unchecked→validate discipline mirrors `project::project`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Unchecked `[credentials]` block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct UncheckedCredentialsConfig {
    /// Provider names in precedence order. Empty → implicit `["env"]`.
    #[serde(default)]
    pub chain: Vec<String>,
    /// Provider definitions keyed by name.
    #[serde(default)]
    pub providers: BTreeMap<String, UncheckedProvider>,
}

/// Unchecked provider definition (`[credentials.providers.<name>]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UncheckedProvider {
    /// Provider kind: `"env"` or `"file"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `file`: secrets directory.
    #[serde(default)]
    pub dir: Option<String>,
    /// `file`: credential-id → filename map.
    #[serde(default)]
    pub key_map: BTreeMap<String, String>,
}

/// Validated chain configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsChainConfig {
    /// Ordered, validated providers.
    pub chain: Vec<ProviderConfig>,
}

/// A validated provider configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfig {
    /// Environment-variable provider.
    Env,
    /// File provider with a secrets dir and id→filename map.
    File {
        /// Secrets directory.
        dir: String,
        /// Credential-id → filename.
        key_map: BTreeMap<String, String>,
    },
}

/// Errors validating a `[credentials]` block.
#[non_exhaustive]
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CredentialsConfigError {
    /// A name in `chain` has no matching provider definition.
    #[error("chain references undefined provider {0:?}")]
    UndefinedProvider(String),
    /// A provider has an unknown `type`.
    #[error("provider {name:?}: unknown type {kind:?}")]
    UnknownKind {
        /// Provider name.
        name: String,
        /// The unknown kind string.
        kind: String,
    },
    /// A `file` provider is missing `dir`.
    #[error("file provider {0:?}: missing `dir`")]
    FileMissingDir(String),
}

impl Default for CredentialsChainConfig {
    fn default() -> Self {
        // Zero-config default: env-only.
        Self {
            chain: vec![ProviderConfig::Env],
        }
    }
}

impl UncheckedCredentialsConfig {
    /// Validate into a [`CredentialsChainConfig`]. An empty `chain`
    /// defaults to `["env"]`. `"env"` needs no provider definition.
    pub fn validate(self) -> Result<CredentialsChainConfig, CredentialsConfigError> {
        let names = if self.chain.is_empty() {
            vec!["env".to_string()]
        } else {
            self.chain
        };

        let mut chain = Vec::with_capacity(names.len());
        for name in names {
            if name == "env" && !self.providers.contains_key("env") {
                chain.push(ProviderConfig::Env);
                continue;
            }
            let def = self
                .providers
                .get(&name)
                .ok_or_else(|| CredentialsConfigError::UndefinedProvider(name.clone()))?;
            match def.kind.as_str() {
                "env" => chain.push(ProviderConfig::Env),
                "file" => {
                    let dir = def
                        .dir
                        .clone()
                        .ok_or_else(|| CredentialsConfigError::FileMissingDir(name.clone()))?;
                    chain.push(ProviderConfig::File {
                        dir,
                        key_map: def.key_map.clone(),
                    });
                }
                other => {
                    return Err(CredentialsConfigError::UnknownKind {
                        name: name.clone(),
                        kind: other.to_string(),
                    });
                }
            }
        }
        Ok(CredentialsChainConfig { chain })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_defaults_to_env_only() {
        let cfg = UncheckedCredentialsConfig::default().validate().unwrap();
        assert_eq!(cfg.chain, vec![ProviderConfig::Env]);
    }

    #[test]
    fn explicit_env_chain_without_provider_def_short_circuits() {
        let toml = r#"chain = ["env"]"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        let cfg = unchecked.validate().unwrap();
        assert_eq!(cfg.chain, vec![ProviderConfig::Env]);
    }

    #[test]
    fn env_then_file_validates() {
        let toml = r#"
chain = ["env", "file"]
[providers.file]
type = "file"
dir = "/var/run/secrets"
key_map = { anthropic_api_key = "anthropic-key" }
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        let cfg = unchecked.validate().unwrap();
        assert_eq!(cfg.chain.len(), 2);
        assert_eq!(cfg.chain[0], ProviderConfig::Env);
        match &cfg.chain[1] {
            ProviderConfig::File { dir, key_map } => {
                assert_eq!(dir, "/var/run/secrets");
                assert_eq!(key_map.get("anthropic_api_key").unwrap(), "anthropic-key");
            }
            _ => panic!("expected file provider"),
        }
    }

    #[test]
    fn undefined_provider_rejected() {
        let toml = r#"chain = ["vault"]"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::UndefinedProvider("vault".to_string())
        );
    }

    #[test]
    fn file_without_dir_rejected() {
        let toml = r#"
chain = ["file"]
[providers.file]
type = "file"
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::FileMissingDir("file".to_string())
        );
    }

    #[test]
    fn unknown_kind_rejected() {
        let toml = r#"
chain = ["weird"]
[providers.weird]
type = "smoke-signal"
"#;
        let unchecked: UncheckedCredentialsConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            unchecked.validate().unwrap_err(),
            CredentialsConfigError::UnknownKind { .. }
        ));
    }
}
