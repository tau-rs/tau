//! `BundleManifest` and its sub-structs. See spec §4 + §6.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tau_domain::{AgentId, CapabilityShape, PackageSource};
use tau_ports::target::TargetTriple;

use crate::bundle::error::BundleParseError;

/// IR payload carried in a v2 bundle.
///
/// Per the design spec D-5, v0 ships the IR as data inside the bundle;
/// the bundle's wasm component carries the interpreter as code and reads
/// this payload at startup. v1 (β.7) keeps the payload field but its
/// semantics change: `canonical_ir_bytes_hex` becomes the input to AOT
/// lowering rather than to runtime interpretation.
///
/// Both `canonical_ir_hash` and `canonical_ir_bytes_hex` are hex strings
/// for TOML round-trip compatibility (`Vec<u8>` serializes to integer
/// arrays in TOML, which is inefficient for large payloads).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrPayload {
    /// IR format version (D-6 — semver-shaped, e.g. "v1.0.0").
    pub ir_format: String,
    /// SHA-256 of the canonical IR bytes, lowercase hex (64 chars).
    /// Redundant with the bytes themselves but cheap; lets `tau verify`
    /// short-circuit on a hash mismatch before re-deserializing.
    pub canonical_ir_hash: String,
    /// The canonical IR bytes encoded as lowercase hex. Hashed into the
    /// bundle's self-hash via the canonical TOML, per D-6.
    pub canonical_ir_bytes_hex: String,
}

/// Error from hex decoding of IrPayload fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexDecodeError(pub String);

impl std::fmt::Display for HexDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hex decode error: {}", self.0)
    }
}

impl IrPayload {
    /// Decode `canonical_ir_bytes_hex` back to raw bytes.
    pub fn canonical_ir_bytes(&self) -> Result<Vec<u8>, HexDecodeError> {
        hex_decode(&self.canonical_ir_bytes_hex)
    }

    /// Parse the stored `canonical_ir_hash` hex to a `[u8; 32]` array.
    pub fn canonical_ir_hash_bytes(&self) -> Result<[u8; 32], HexDecodeError> {
        let v = hex_decode(&self.canonical_ir_hash)?;
        v.try_into()
            .map_err(|_| HexDecodeError("expected 32 bytes but got a different length".into()))
    }
}

/// Decode a lowercase hex string to bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, HexDecodeError> {
    if !s.len().is_multiple_of(2) {
        return Err(HexDecodeError("odd-length hex string".into()));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut chars = s.chars();
    loop {
        match (chars.next(), chars.next()) {
            (None, _) => break,
            (Some(h), Some(l)) => {
                let hi = h
                    .to_digit(16)
                    .ok_or_else(|| HexDecodeError(format!("invalid hex char '{h}'")))?;
                let lo = l
                    .to_digit(16)
                    .ok_or_else(|| HexDecodeError(format!("invalid hex char '{l}'")))?;
                out.push(((hi << 4) | lo) as u8);
            }
            _ => return Err(HexDecodeError("unexpected end of hex string".into())),
        }
    }
    Ok(out)
}

/// Top-level bundle manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleManifest {
    /// Major schema version. v1 is the legacy line; v2 adds `ir_payload`.
    /// v3+ would be a breaking change (consumer rejects loudly).
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
    /// IR payload for v2 bundles. `None` for v1 (legacy) bundles;
    /// `Some` when `tau build` successfully lowered the project IR.
    /// The canonical_ir_bytes are hashed into the bundle's self-hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_payload: Option<IrPayload>,
}

/// Bundle-level metadata.
///
/// NOTE: `to_canonical_toml` (bundle/canonical.rs) hand-rolls the TOML
/// emission and bypasses serde. Any field added here that should be
/// covered by the self-hash MUST also be emitted there, or it will be
/// silently excluded from the hash. See `canonical_emits_all_bundle_meta_fields`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleMeta {
    /// Self-hash. SHA-256 of the canonical TOML serialization with
    /// this field set to the empty string. See spec §5.
    pub sha256: String,
    /// RFC 3339 UTC timestamp. Informational; **excluded** from the
    /// self-hash (see `compute_self_hash`) so rebuilds at different
    /// times reproduce the same hash. §E (`tau verify --bundle`) relies
    /// on this.
    pub created_at: String,
    /// tau binary version that produced this bundle.
    pub tau_version: String,
    /// Deployment target.
    pub target: TargetTriple,
    /// Agent ids this bundle was sliced to (sorted), or absent for a
    /// full build. Drives `tau verify --bundle` reproduction: a sliced
    /// bundle records its `--agent` set here so the rebuild replays the
    /// same slice. Covered by the self-hash (deterministic build input);
    /// omitted from canonical TOML when `None`, so existing full bundles
    /// serialize identically and their self-hashes are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_agents: Option<Vec<String>>,
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
    /// skill.spawn allow-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_skill_spawn: Vec<String>,
    /// skill.spawn deny-list patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_skill_spawn: Vec<String>,
}

impl BundleEffectiveCapabilities {
    /// True when every list is empty (the table can be omitted entirely).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::bundle::manifest::BundleEffectiveCapabilities;
    ///
    /// let caps = BundleEffectiveCapabilities::default();
    /// assert!(caps.is_empty());
    ///
    /// let mut caps2 = BundleEffectiveCapabilities::default();
    /// caps2.allow_fs_read.push("/proj/**".to_string());
    /// assert!(!caps2.is_empty());
    /// ```
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
            && self.allow_skill_spawn.is_empty()
            && self.deny_skill_spawn.is_empty()
    }
}

impl BundleManifest {
    /// Parse a bundle manifest from a TOML string.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::bundle::manifest::BundleManifest;
    ///
    /// let toml = r#"
    /// schema_version = 1
    ///
    /// [bundle]
    /// sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
    /// created_at = "2026-01-01T00:00:00Z"
    /// tau_version = "0.1.0"
    /// target = "passthrough"
    ///
    /// [project]
    /// name = "my-bot"
    /// version = "0.1.0"
    /// tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    /// "#;
    /// let manifest = BundleManifest::parse_str(toml).expect("valid bundle TOML");
    /// assert_eq!(manifest.project.name, "my-bot");
    /// assert_eq!(manifest.schema_version, 1);
    /// ```
    pub fn parse_str(s: &str) -> Result<Self, BundleParseError> {
        let manifest: BundleManifest = toml::from_str(s)?;
        if manifest.schema_version != 1 && manifest.schema_version != 2 {
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
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::bundle::manifest::BundleManifest;
    ///
    /// # let toml = r#"
    /// # schema_version = 1
    /// # [bundle]
    /// # sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
    /// # created_at = "2026-01-01T00:00:00Z"
    /// # tau_version = "0.1.0"
    /// # target = "passthrough"
    /// # [project]
    /// # name = "bot"
    /// # version = "0.1.0"
    /// # tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    /// # "#;
    /// # let manifest = BundleManifest::parse_str(toml).unwrap();
    /// let canonical = manifest.to_canonical_toml();
    /// assert!(canonical.contains("[bundle]"));
    /// assert!(canonical.contains("[project]"));
    /// ```
    pub fn to_canonical_toml(&self) -> String {
        crate::bundle::canonical::to_canonical_toml(self)
    }

    /// Compute the canonical self-hash of this manifest. Does not mutate.
    /// See `crate::bundle::hash::compute_self_hash`.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::bundle::manifest::BundleManifest;
    ///
    /// # let toml = r#"
    /// # schema_version = 1
    /// # [bundle]
    /// # sha256 = ""
    /// # created_at = "2026-01-01T00:00:00Z"
    /// # tau_version = "0.1.0"
    /// # target = "passthrough"
    /// # [project]
    /// # name = "bot"
    /// # version = "0.1.0"
    /// # tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    /// # "#;
    /// # let manifest = BundleManifest::parse_str(toml).unwrap();
    /// let hash = manifest.compute_self_hash();
    /// assert_eq!(hash.len(), 64);
    /// assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    /// ```
    pub fn compute_self_hash(&self) -> String {
        crate::bundle::hash::compute_self_hash(self)
    }

    /// Verify that this manifest's `bundle.sha256` field equals the
    /// recomputed canonical self-hash. See
    /// `crate::bundle::hash::verify_self_hash`.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::bundle::manifest::BundleManifest;
    ///
    /// # let toml = r#"
    /// # schema_version = 1
    /// # [bundle]
    /// # sha256 = ""
    /// # created_at = "2026-01-01T00:00:00Z"
    /// # tau_version = "0.1.0"
    /// # target = "passthrough"
    /// # [project]
    /// # name = "bot"
    /// # version = "0.1.0"
    /// # tau_toml_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    /// # "#;
    /// # let mut manifest = BundleManifest::parse_str(toml).unwrap();
    /// // Fill the sha256 with the correct self-hash, then verify.
    /// manifest.bundle.sha256 = manifest.compute_self_hash();
    /// assert!(manifest.verify_self_hash().is_ok());
    ///
    /// // Tamper with the hash → mismatch.
    /// manifest.bundle.sha256 = "wrong".to_string();
    /// assert!(manifest.verify_self_hash().is_err());
    /// ```
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
            schema_version: 2,
            bundle: BundleMeta {
                sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
                created_at: "2026-05-19T13:42:11Z".into(),
                tau_version: "0.1.0".into(),
                target: "linux-native-strict".parse().unwrap(),
                selected_agents: None,
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
            ir_payload: None,
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
    fn parse_str_accepts_schema_version_2() {
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
        let m = BundleManifest::parse_str(toml_str).expect("v2 must parse");
        assert_eq!(m.schema_version, 2);
    }

    #[test]
    fn parse_str_accepts_schema_version_1_legacy() {
        // v1 bundles from before the schema_version bump must still parse.
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
"#;
        let m = BundleManifest::parse_str(toml_str).expect("legacy v1 must still parse");
        assert_eq!(m.schema_version, 1);
    }

    #[test]
    fn parse_str_rejects_schema_version_3() {
        let toml_str = r#"
schema_version = 3

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
        let err = BundleManifest::parse_str(toml_str).expect_err("should reject v3");
        match err {
            BundleParseError::UnsupportedSchemaVersion { found } => assert_eq!(found, 3),
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
    fn selected_agents_omitted_when_none() {
        let mut m = sample_manifest();
        m.bundle.selected_agents = None;
        let toml_str = toml::to_string(&m).expect("serialize");
        assert!(
            !toml_str.contains("selected_agents"),
            "selected_agents should be omitted when None: {toml_str}"
        );
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(parsed.bundle.selected_agents, None);
    }

    #[test]
    fn selected_agents_round_trips_when_some() {
        let mut m = sample_manifest();
        m.bundle.selected_agents = Some(vec!["alpha".to_string(), "beta".to_string()]);
        let toml_str = toml::to_string(&m).expect("serialize");
        let parsed = BundleManifest::parse_str(&toml_str).expect("parse");
        assert_eq!(
            parsed.bundle.selected_agents,
            Some(vec!["alpha".to_string(), "beta".to_string()])
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

    #[test]
    fn effective_caps_is_empty_includes_skill_spawn() {
        let mut caps = BundleEffectiveCapabilities::default();
        assert!(caps.is_empty());
        caps.allow_skill_spawn.push("critic".to_string());
        assert!(
            !caps.is_empty(),
            "allow_skill_spawn must count toward non-empty"
        );
    }

    /// A bundle emitted by a hypothetical future tau: unknown fields and
    /// tables at every level plus a future effective-cap shape. Today's
    /// parser MUST accept it and read the known fields intact.
    #[test]
    fn future_bundle_parses_with_all_tolerance_points() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "deadbeef"
created_at = "2026-01-01T00:00:00Z"
tau_version = "9.9.9"
target = "passthrough"
future_meta = "tolerated"

[project]
name = "fwd"
version = "0.1.0"
tau_toml_sha256 = "aaaa"

[[packages]]
name = "p"
version = "0.1.0"
source = "https://example.com/p.git"
tree_sha256 = "1111"
future_pkg_field = "tolerated"

[[agents]]
id = "r"
backend = { kind = "anthropic", future_backend_key = "tolerated" }
system_prompt_sha256 = "7777"
effective_capabilities = { allow_fs_read = ["/data/**"], allow_future_shape = ["/x/**"] }
future_agent_field = 1

[future_section]
future_key = "tolerated"
"#;
        let m = BundleManifest::parse_str(toml_str).expect("future bundle must parse");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.project.name, "fwd");
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.packages[0].name, "p");
        assert_eq!(m.agents.len(), 1);
        assert_eq!(m.agents[0].id.as_str(), "r");
        assert_eq!(
            m.agents[0].effective_capabilities.allow_fs_read,
            vec!["/data/**".to_string()]
        );
        assert!(m.agents[0].backend.extra.contains_key("future_backend_key"));
    }

    #[test]
    fn schema_version_ninety_nine_is_rejected() {
        let toml_str = r#"
schema_version = 99

[bundle]
sha256 = "x"
created_at = "2026-01-01T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "x"
"#;
        match BundleManifest::parse_str(toml_str) {
            Err(crate::bundle::error::BundleParseError::UnsupportedSchemaVersion { found }) => {
                assert_eq!(found, 99);
            }
            other => panic!("expected UnsupportedSchemaVersion(99), got {other:?}"),
        }
    }

    #[test]
    fn effective_capabilities_unknown_allow_field_is_ignored() {
        let toml_str = r#"
schema_version = 1

[bundle]
sha256 = "x"
created_at = "2026-01-01T00:00:00Z"
tau_version = "0.1.0"
target = "passthrough"

[project]
name = "x"
version = "0.1.0"
tau_toml_sha256 = "x"

[[agents]]
id = "r"
backend = { kind = "anthropic" }
system_prompt_sha256 = "7"
effective_capabilities = { allow_fs_read = ["/a/**"], allow_some_future_shape = ["/b/**"] }
"#;
        let m = BundleManifest::parse_str(toml_str).expect("must parse");
        assert_eq!(
            m.agents[0].effective_capabilities.allow_fs_read,
            vec!["/a/**".to_string()]
        );
    }
}
