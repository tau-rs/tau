//! `tau verify --bundle` reproducibility checker (Phase 2 §E).
//!
//! Rebuilds a fresh bundle from the local source tree and compares its
//! self-hash to a shipped bundle. See spec
//! `2026-05-28-tau-verify-bundle-design.md`.

use std::path::PathBuf;

use crate::bundle::manifest::BundleManifest;
use crate::bundle::reproduce_error::ReproError;

/// Inputs to [`verify_reproducible`].
#[derive(Debug, Clone)]
pub struct ReproOptions {
    /// Path to the shipped `.tau` bundle to reproduce.
    pub bundle_path: PathBuf,
    /// Local source tree to rebuild from (typically cwd).
    pub project_root: PathBuf,
}

/// Result of a reproducibility check.
#[derive(Debug, Clone)]
pub struct ReproReport {
    /// True when the rebuilt bundle's self-hash equals the shipped one's.
    pub reproducible: bool,
    /// The shipped bundle's self-hash.
    pub shipped_sha256: String,
    /// The rebuilt bundle's self-hash.
    pub rebuilt_sha256: String,
    /// Field-level divergences. Empty when `reproducible`.
    pub diffs: Vec<ManifestDiff>,
}

/// Which side of a comparison a one-sided item appears on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Side {
    /// Present only in the shipped bundle.
    ShippedOnly,
    /// Present only in the rebuilt bundle.
    RebuiltOnly,
}

/// A single field-level divergence between two manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestDiff {
    /// A `[project]` field differs.
    ProjectField {
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// A package is present on only one side.
    PackageMissing {
        /// Package name.
        name: String,
        /// Which side it appears on.
        side: Side,
    },
    /// A package field differs.
    PackageField {
        /// Package name.
        name: String,
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// An agent is present on only one side.
    AgentMissing {
        /// Agent id.
        id: String,
        /// Which side it appears on.
        side: Side,
    },
    /// An agent field differs.
    AgentField {
        /// Agent id.
        id: String,
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// A `[bundle]` metadata field differs (target, tau_version).
    BundleMetaField {
        /// Field name.
        field: String,
        /// Shipped value.
        shipped: String,
        /// Rebuilt value.
        rebuilt: String,
    },
    /// schema_version differs.
    SchemaVersionMismatch {
        /// Shipped version.
        shipped: u32,
        /// Rebuilt version.
        rebuilt: u32,
    },
}

/// Rebuild from `opts.project_root` and compare to the shipped bundle.
pub fn verify_reproducible(_opts: ReproOptions) -> Result<ReproReport, ReproError> {
    unimplemented!("Task 3")
}

/// Field-level diff between two manifests (Task 2).
pub(crate) fn diff_manifests(
    shipped: &BundleManifest,
    rebuilt: &BundleManifest,
) -> Vec<ManifestDiff> {
    use std::collections::BTreeMap;
    let mut diffs = Vec::new();

    if shipped.schema_version != rebuilt.schema_version {
        diffs.push(ManifestDiff::SchemaVersionMismatch {
            shipped: shipped.schema_version,
            rebuilt: rebuilt.schema_version,
        });
    }

    // bundle meta — target + tau_version (NOT sha256, NOT created_at).
    if shipped.bundle.target != rebuilt.bundle.target {
        diffs.push(ManifestDiff::BundleMetaField {
            field: "target".into(),
            shipped: shipped.bundle.target.to_string(),
            rebuilt: rebuilt.bundle.target.to_string(),
        });
    }
    if shipped.bundle.tau_version != rebuilt.bundle.tau_version {
        diffs.push(ManifestDiff::BundleMetaField {
            field: "tau_version".into(),
            shipped: shipped.bundle.tau_version.clone(),
            rebuilt: rebuilt.bundle.tau_version.clone(),
        });
    }

    // project
    if shipped.project.name != rebuilt.project.name {
        diffs.push(ManifestDiff::ProjectField {
            field: "name".into(),
            shipped: shipped.project.name.clone(),
            rebuilt: rebuilt.project.name.clone(),
        });
    }
    if shipped.project.version != rebuilt.project.version {
        diffs.push(ManifestDiff::ProjectField {
            field: "version".into(),
            shipped: shipped.project.version.to_string(),
            rebuilt: rebuilt.project.version.to_string(),
        });
    }
    if shipped.project.tau_toml_sha256 != rebuilt.project.tau_toml_sha256 {
        diffs.push(ManifestDiff::ProjectField {
            field: "tau_toml_sha256".into(),
            shipped: shipped.project.tau_toml_sha256.clone(),
            rebuilt: rebuilt.project.tau_toml_sha256.clone(),
        });
    }

    // packages — index by name, stable order.
    let ship_pkgs: BTreeMap<&str, &_> =
        shipped.packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let reb_pkgs: BTreeMap<&str, &_> =
        rebuilt.packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut pkg_names: Vec<&str> =
        ship_pkgs.keys().chain(reb_pkgs.keys()).copied().collect();
    pkg_names.sort_unstable();
    pkg_names.dedup();
    for name in pkg_names {
        match (ship_pkgs.get(name), reb_pkgs.get(name)) {
            (Some(_), None) => diffs.push(ManifestDiff::PackageMissing {
                name: name.to_string(),
                side: Side::ShippedOnly,
            }),
            (None, Some(_)) => diffs.push(ManifestDiff::PackageMissing {
                name: name.to_string(),
                side: Side::RebuiltOnly,
            }),
            (Some(s), Some(r)) => {
                if s.version != r.version {
                    diffs.push(ManifestDiff::PackageField {
                        name: name.to_string(),
                        field: "version".into(),
                        shipped: s.version.to_string(),
                        rebuilt: r.version.to_string(),
                    });
                }
                if s.tree_sha256 != r.tree_sha256 {
                    diffs.push(ManifestDiff::PackageField {
                        name: name.to_string(),
                        field: "tree_sha256".into(),
                        shipped: s.tree_sha256.clone(),
                        rebuilt: r.tree_sha256.clone(),
                    });
                }
                if s.source != r.source {
                    diffs.push(ManifestDiff::PackageField {
                        name: name.to_string(),
                        field: "source".into(),
                        shipped: format!("{:?}", s.source),
                        rebuilt: format!("{:?}", r.source),
                    });
                }
                if s.binary_sha256 != r.binary_sha256 {
                    diffs.push(ManifestDiff::PackageField {
                        name: name.to_string(),
                        field: "binary_sha256".into(),
                        shipped: format!("{:?}", s.binary_sha256),
                        rebuilt: format!("{:?}", r.binary_sha256),
                    });
                }
                if s.required_shapes != r.required_shapes {
                    diffs.push(ManifestDiff::PackageField {
                        name: name.to_string(),
                        field: "required_shapes".into(),
                        shipped: format!("{:?}", s.required_shapes),
                        rebuilt: format!("{:?}", r.required_shapes),
                    });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    // agents — index by id, stable order.
    let ship_agents: BTreeMap<String, &_> = shipped
        .agents
        .iter()
        .map(|a| (a.id.as_str().to_string(), a))
        .collect();
    let reb_agents: BTreeMap<String, &_> = rebuilt
        .agents
        .iter()
        .map(|a| (a.id.as_str().to_string(), a))
        .collect();
    let mut agent_ids: Vec<String> =
        ship_agents.keys().chain(reb_agents.keys()).cloned().collect();
    agent_ids.sort_unstable();
    agent_ids.dedup();
    for id in agent_ids {
        match (ship_agents.get(&id), reb_agents.get(&id)) {
            (Some(_), None) => diffs.push(ManifestDiff::AgentMissing {
                id: id.clone(),
                side: Side::ShippedOnly,
            }),
            (None, Some(_)) => diffs.push(ManifestDiff::AgentMissing {
                id: id.clone(),
                side: Side::RebuiltOnly,
            }),
            (Some(s), Some(r)) => {
                if s.system_prompt_sha256 != r.system_prompt_sha256 {
                    diffs.push(ManifestDiff::AgentField {
                        id: id.clone(),
                        field: "system_prompt_sha256".into(),
                        shipped: s.system_prompt_sha256.clone(),
                        rebuilt: r.system_prompt_sha256.clone(),
                    });
                }
                if s.backend != r.backend {
                    diffs.push(ManifestDiff::AgentField {
                        id: id.clone(),
                        field: "backend".into(),
                        shipped: format!("{:?}", s.backend),
                        rebuilt: format!("{:?}", r.backend),
                    });
                }
                if s.required_tools != r.required_tools {
                    diffs.push(ManifestDiff::AgentField {
                        id: id.clone(),
                        field: "required_tools".into(),
                        shipped: format!("{:?}", s.required_tools),
                        rebuilt: format!("{:?}", r.required_tools),
                    });
                }
                if s.effective_capabilities != r.effective_capabilities {
                    diffs.push(ManifestDiff::AgentField {
                        id: id.clone(),
                        field: "effective_capabilities".into(),
                        shipped: format!("{:?}", s.effective_capabilities),
                        rebuilt: format!("{:?}", r.effective_capabilities),
                    });
                }
            }
            (None, None) => unreachable!(),
        }
    }

    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::build::{build, BuildOptions};
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    /// Build a minimal (zero-package, one inline-prompt agent) bundle
    /// and return its parsed manifest.
    fn sample_manifest() -> BundleManifest {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "diff-fixture"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "noop@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let artifact = build(BuildOptions {
            project_root: tmp.path().to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
        })
        .unwrap();
        let s = std::fs::read_to_string(&artifact.path).unwrap();
        BundleManifest::parse_str(&s).unwrap()
    }

    #[test]
    fn diff_ignores_sha256_and_created_at() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.bundle.sha256 = "different".into();
        b.bundle.created_at = "2099-01-01T00:00:00Z".into();
        assert!(
            diff_manifests(&a, &b).is_empty(),
            "sha256 + created_at must be excluded; got {:?}",
            diff_manifests(&a, &b)
        );
    }

    #[test]
    fn diff_reports_tau_version_skew() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.bundle.tau_version = "9.9.9".into();
        let diffs = diff_manifests(&a, &b);
        assert_eq!(diffs.len(), 1, "got {diffs:?}");
        assert!(
            matches!(&diffs[0], ManifestDiff::BundleMetaField { field, .. } if field == "tau_version"),
            "got {diffs:?}"
        );
    }

    #[test]
    fn diff_reports_project_tau_toml_sha256() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.project.tau_toml_sha256 = "ffff".into();
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::ProjectField { field, .. } if field == "tau_toml_sha256")),
            "got {diffs:?}",
        );
    }

    #[test]
    fn diff_detects_added_package() {
        use std::str::FromStr;
        let a = sample_manifest();
        let mut b = a.clone();
        let pkg = crate::bundle::manifest::BundlePackage {
            name: "newpkg".into(),
            version: semver::Version::new(0, 1, 0),
            source: tau_domain::PackageSource::from_str(
                "https://github.com/example/newpkg.git",
            )
            .unwrap(),
            tree_sha256: "0".repeat(64),
            binary_sha256: None,
            required_shapes: vec![],
        };
        b.packages.push(pkg);
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::PackageMissing { name, side } if name == "newpkg" && *side == Side::RebuiltOnly)),
            "got {diffs:?}",
        );
    }

    #[test]
    fn diff_detects_removed_agent() {
        let a = sample_manifest();
        let mut b = a.clone();
        b.agents.clear();
        let diffs = diff_manifests(&a, &b);
        assert!(
            diffs.iter().any(|d| matches!(d, ManifestDiff::AgentMissing { id, side } if id == "solo" && *side == Side::ShippedOnly)),
            "got {diffs:?}",
        );
    }
}
