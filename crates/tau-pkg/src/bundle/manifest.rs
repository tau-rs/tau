//! `BundleManifest` and its sub-structs. See spec §4 + §6.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tau_domain::{AgentId, CapabilityShape, PackageSource};
use tau_ports::target::TargetTriple;

use crate::bundle::error::BundleParseError;

/// Top-level bundle manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Major schema version. v1.x is the current line; v2+ would be a
    /// breaking change (consumer rejects loudly).
    pub schema_version: u32,
    /// Bundle-level metadata (sha + timestamp + tau version + target).
    pub bundle: BundleMeta,
    /// Project identity (name, version, source tau.toml hash).
    pub project: ProjectInfo,
    /// Resolved packages (lockfile-equivalent set).
    #[serde(default)]
    pub packages: Vec<BundlePackage>,
    /// Per-agent compiled grant set + system prompt hash + tool list.
    #[serde(default)]
    pub agents: Vec<BundleAgent>,
}

/// Bundle-level metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMeta {
    /// Self-hash. SHA-256 of the canonical TOML serialization with
    /// this field set to the empty string. See spec §5.
    pub sha256: String,
    /// RFC 3339 UTC timestamp. Informational; in the hash. Reproducibility
    /// is §E's problem.
    pub created_at: String,
    /// tau binary version that produced this bundle.
    pub tau_version: String,
    /// Deployment target.
    pub target: TargetTriple,
}

/// Project identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Project name from `[project]` table in source tau.toml.
    pub name: String,
    /// Project version (semver).
    pub version: semver::Version,
    /// SHA-256 of the source tau.toml bytes (hex).
    pub tau_toml_sha256: String,
}

/// One resolved package in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundlePackage {
    /// Package name.
    pub name: String,
    /// Resolved version (semver).
    pub version: semver::Version,
    /// Source location (git URL + ref, local path, etc.).
    pub source: PackageSource,
    /// SHA-256 of the package tree (output of `tau-pkg::tree_hash`).
    pub tree_sha256: String,
    /// SHA-256 of the plugin binary, if this is a plugin package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha256: Option<String>,
    /// Capability shapes this package's plugin needs the host to enforce.
    #[serde(default)]
    pub required_shapes: Vec<CapabilityShape>,
}

/// One agent's compiled deployment record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleAgent {
    /// Agent identifier from project tau.toml.
    pub id: AgentId,
    /// LLM backend selection (kind + model + arbitrary backend-specific extras).
    pub backend: BackendRef,
    /// SHA-256 of the agent's system prompt text (hex).
    pub system_prompt_sha256: String,
    /// Plugin names this agent depends on.
    #[serde(default)]
    pub required_tools: Vec<String>,
    /// Per-shape allow/deny lists from `compute_effective`. Omitted
    /// entirely when the agent's grant set is empty.
    #[serde(default, skip_serializing_if = "BundleEffectiveCapabilities::is_empty")]
    pub effective_capabilities: BundleEffectiveCapabilities,
}

/// LLM backend reference carried in the bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendRef {
    /// Backend kind (e.g. "ollama", "anthropic", "openai", "stub").
    pub kind: String,
    /// Model identifier, if the backend requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Forward-compat catch-all for backend-specific keys.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// Serialized form of `compute_effective`'s output for one agent.
/// All ten lists hold glob patterns; empty lists are omitted from the
/// TOML output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BundleEffectiveCapabilities {
    /// fs.read allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_read: Vec<String>,
    /// fs.read deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_read: Vec<String>,
    /// fs.write allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_fs_write: Vec<String>,
    /// fs.write deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_fs_write: Vec<String>,
    /// exec allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_exec: Vec<String>,
    /// exec deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_exec: Vec<String>,
    /// net.http allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_net_http: Vec<String>,
    /// net.http deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_net_http: Vec<String>,
    /// agent.spawn allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_agent_spawn: Vec<String>,
    /// agent.spawn deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_agent_spawn: Vec<String>,
}

impl BundleEffectiveCapabilities {
    /// True when every list is empty (the table can be omitted entirely).
    pub fn is_empty(&self) -> bool {
        self.allow_fs_read.is_empty()
            && self.deny_fs_read.is_empty()
            && self.allow_fs_write.is_empty()
            && self.deny_fs_write.is_empty()
            && self.allow_exec.is_empty()
            && self.deny_exec.is_empty()
            && self.allow_net_http.is_empty()
            && self.deny_net_http.is_empty()
            && self.allow_agent_spawn.is_empty()
            && self.deny_agent_spawn.is_empty()
    }
}

impl BundleManifest {
    /// Parse a bundle manifest from a TOML string.
    pub fn parse_str(s: &str) -> Result<Self, BundleParseError> {
        let manifest: BundleManifest = toml::from_str(s)?;
        if manifest.schema_version != 1 {
            return Err(BundleParseError::UnsupportedSchemaVersion {
                found: manifest.schema_version,
            });
        }
        Ok(manifest)
    }

    /// Read and parse a bundle manifest from a file.
    pub fn from_path(p: &std::path::Path) -> Result<Self, crate::bundle::error::BundleIoError> {
        let bytes = std::fs::read_to_string(p).map_err(|source| {
            crate::bundle::error::BundleIoError::Read {
                path: p.to_path_buf(),
                source,
            }
        })?;
        Ok(Self::parse_str(&bytes)?)
    }

    /// Emit the canonical-TOML serialization of this manifest. See
    /// `crate::bundle::canonical::to_canonical_toml` for the format
    /// specification.
    pub fn to_canonical_toml(&self) -> String {
        crate::bundle::canonical::to_canonical_toml(self)
    }

    /// Compute the canonical self-hash of this manifest. Does not mutate.
    /// See `crate::bundle::hash::compute_self_hash`.
    pub fn compute_self_hash(&self) -> String {
        crate::bundle::hash::compute_self_hash(self)
    }

    /// Verify that this manifest's `bundle.sha256` field equals the
    /// recomputed canonical self-hash. See
    /// `crate::bundle::hash::verify_self_hash`.
    pub fn verify_self_hash(&self) -> Result<(), crate::bundle::error::BundleIntegrityError> {
        crate::bundle::hash::verify_self_hash(self)
    }
}

#[cfg(test)]
pub(crate) mod tests_helpers {
    use super::*;
    use std::collections::BTreeMap;
    use tau_domain::GitLocation;

    /// Construct a fully-populated bundle manifest for tests. Shared
    /// across the bundle module's unit tests.
    pub fn sample_manifest() -> BundleManifest {
        BundleManifest {
            schema_version: 1,
            bundle: BundleMeta {
                sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
                created_at: "2026-05-19T13:42:11Z".into(),
                tau_version: "0.1.0".into(),
                target: "linux-native-strict".parse().unwrap(),
            },
            project: ProjectInfo {
                name: "support-bot".into(),
                version: semver::Version::parse("0.3.2").unwrap(),
                tau_toml_sha256: "a".repeat(64),
            },
            packages: vec![BundlePackage {
                name: "tau-plugin-fs-read".into(),
                version: semver::Version::parse("0.2.1").unwrap(),
                source: PackageSource::Git {
                    location: GitLocation::Url(
                        "https://github.com/example/fs-read.git".parse().unwrap(),
                    ),
                    rev: Some("v0.2.1".into()),
                },
                tree_sha256: "1".repeat(64),
                binary_sha256: Some("2".repeat(64)),
                required_shapes: vec![CapabilityShape::FilesystemRead],
            }],
            agents: vec![BundleAgent {
                id: "researcher".parse().unwrap(),
                backend: BackendRef {
                    kind: "ollama".into(),
                    model: Some("llama3.1:8b".into()),
                    extra: BTreeMap::new(),
                },
                system_prompt_sha256: "7".repeat(64),
                required_tools: vec!["tau-plugin-fs-read".into()],
                effective_capabilities: BundleEffectiveCapabilities {
                    allow_fs_read: vec!["/data/**".into(), "/etc/agent/**".into()],
                    deny_fs_read: vec!["/data/secrets/**".into()],
                    ..Default::default()
                },
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_helpers::sample_manifest;
    use super::*;

    #[test]
    fn manifest_round_trips_through_toml() {
        let original = sample_manifest();
        let toml_str = toml::to_string(&original).expect("serialize");
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_str_rejects_schema_version_2() {
        let toml_str = r#"
schema_version = 2

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
        let err = BundleManifest::parse_str(toml_str).expect_err("should reject v2");
        match err {
            BundleParseError::UnsupportedSchemaVersion { found } => assert_eq!(found, 2),
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn binary_sha256_is_optional() {
        let mut m = sample_manifest();
        m.packages[0].binary_sha256 = None;
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("binary_sha256"),
            "binary_sha256 should be omitted when None: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed.packages[0].binary_sha256, None);
    }

    #[test]
    fn effective_capabilities_omitted_when_empty() {
        let mut m = sample_manifest();
        m.agents[0].effective_capabilities = BundleEffectiveCapabilities::default();
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("effective_capabilities"),
            "table should be omitted entirely when empty: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(
            parsed.agents[0].effective_capabilities,
            BundleEffectiveCapabilities::default()
        );
    }

    #[test]
    fn backend_extra_captures_unknown_keys() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[agents]]
id = "demo"
backend = { kind = "anthropic", model = "claude-sonnet-4-6", api_base_url = "https://custom.example/" }
system_prompt_sha256 = "7777777777777777777777777777777777777777777777777777777777777777"
"#;
        let m = BundleManifest::parse_str(toml_str).expect("parse");
        let backend = &m.agents[0].backend;
        assert_eq!(backend.kind, "anthropic");
        assert_eq!(backend.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(
            backend.extra.get("api_base_url").map(|v| v.as_str()),
            Some(Some("https://custom.example/")),
        );
    }

    #[test]
    fn unknown_top_level_field_is_accepted() {
        // Forward-compat: a future schema may add a [binaries] table.
        // v1 consumers ignore unknown top-level keys gracefully.
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
created_at = "2026-05-19T13:42:11Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[binaries]
some_future_field = "value"
"#;
        BundleManifest::parse_str(toml_str).expect("future tables ignored");
    }
}
