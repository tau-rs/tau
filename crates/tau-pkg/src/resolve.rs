//! Resolve transitive `requires.tools` dependencies for a project's
//! agents into a concrete install plan.
//!
//! Three phases (see spec §5):
//!   1. Group input entries by `name`.
//!   2. Conflict checks per name: source equality + version-constraint
//!      compatibility.
//!   3. Pick a concrete `Version` per name: lockfile reuse if installed
//!      and compatible, else list available versions at source and pick
//!      the highest satisfying all constraints.
//!
//! One level deep at v0.1: we resolve agents' `requires.tools` only;
//! recursive package-level `dependencies` resolution stays deferred
//! per ADR-0004 §10.

use std::collections::HashMap;

use semver::{Version, VersionReq};
use tau_domain::{AgentId, PackageName, PackageSource};

use crate::registry;
use crate::scope::Scope;
use crate::source_list::{list_versions_at_source, SourceListError};

/// One required-tool declaration drawn from a project tau.toml.
///
/// Constructed by tau-cli during project-config validation; passed
/// into [`resolve_requires_tools`] as `(AgentId, RequiredTool)` pairs.
///
/// `#[non_exhaustive]`: external callers must use [`RequiredTool::new`].
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RequiredTool {
    /// Package name to resolve.
    pub name: PackageName,
    /// Source to fetch from.
    pub source: PackageSource,
    /// Semver requirement; defaults to `*` if absent.
    pub version_req: VersionReq,
}

impl RequiredTool {
    /// Construct a `RequiredTool`. `#[non_exhaustive]` blocks struct-literal
    /// construction outside this crate.
    pub fn new(name: PackageName, source: PackageSource, version_req: VersionReq) -> Self {
        Self {
            name,
            source,
            version_req,
        }
    }
}

/// Output of [`resolve_requires_tools`].
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ResolutionPlan {
    /// Packages that need to be fetched + installed.
    pub installs: Vec<PlannedInstall>,
    /// Packages already installed and reused (no fetch).
    pub reuses: Vec<ReusedInstall>,
}

/// One package the resolver decided needs fetching.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PlannedInstall {
    /// Package name.
    pub name: PackageName,
    /// Concrete version selected from the available set.
    pub version: Version,
    /// Source to fetch from.
    pub source: PackageSource,
    /// Agents that requested this package (for diagnostics).
    pub requested_by: Vec<AgentId>,
}

/// One package the resolver decided to reuse from the lockfile.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ReusedInstall {
    /// Package name.
    pub name: PackageName,
    /// Already-installed version.
    pub version: Version,
}

/// Errors returned by [`resolve_requires_tools`].
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// Two or more agents declared the same tool name with different
    /// `PackageSource` values.
    #[error("tool {name:?}: agents {agents:?} declared conflicting sources {sources:?}")]
    ConflictingSources {
        /// The conflicting tool name.
        name: PackageName,
        /// All distinct sources observed.
        sources: Vec<PackageSource>,
        /// Agents that contributed to the conflict.
        agents: Vec<AgentId>,
    },
    /// No `Version` at the source satisfies the intersected constraints.
    #[error(
        "tool {name:?} from {at_source}: no version satisfies all of {constraints:?}; available: {available:?}"
    )]
    NoCompatibleVersion {
        /// The tool name.
        name: PackageName,
        /// The source consulted.
        at_source: PackageSource,
        /// All `version_req` values across the group.
        constraints: Vec<VersionReq>,
        /// Versions returned by `list_versions_at_source`.
        available: Vec<Version>,
    },
    /// `list_versions_at_source` itself failed.
    #[error("listing versions at {at_source}: {source_err}")]
    SourceListing {
        /// The source we tried to list.
        at_source: PackageSource,
        /// Underlying source-listing error.
        #[source]
        source_err: SourceListError,
    },
    /// Reading the lockfile failed.
    #[error("reading lockfile: {0}")]
    Registry(#[from] crate::error::RegistryError),
}

/// Resolve a flat list of required tools into a [`ResolutionPlan`].
// ResolveError carries diagnostic Vecs (constraints, available versions) that
// make it large; boxing the variants would force callers to dereference through
// Box. The diagnostic data is intentional — suppress the lint instead.
#[allow(clippy::result_large_err)]
pub fn resolve_requires_tools(
    requires: &[(AgentId, RequiredTool)],
    scope: &Scope,
) -> Result<ResolutionPlan, ResolveError> {
    // Phase 1: group by name.
    let groups = group_by_name(requires);

    // Phase 2: same-name conflict checks (source equality).
    for (name, entries) in &groups {
        check_source_equality(name, entries)?;
    }

    // Phase 3: pick concrete versions.
    // Sort groups by name string for deterministic resolution order.
    let mut sorted_groups: Vec<_> = groups.into_iter().collect();
    sorted_groups.sort_by_key(|(a, _)| a.to_string());

    let installed = registry::list(scope)?;
    let mut plan = ResolutionPlan::default();
    for (name, entries) in sorted_groups {
        let source = entries[0].1.source.clone();
        let constraints: Vec<VersionReq> =
            entries.iter().map(|(_, t)| t.version_req.clone()).collect();
        let requested_by: Vec<AgentId> = entries.iter().map(|(a, _)| a.clone()).collect();

        // Lockfile reuse: any installed version satisfying all constraints?
        if let Some(installed_pkg) = installed.iter().find(|p| p.name == name) {
            if installed_pkg.source == source
                && constraints
                    .iter()
                    .all(|c| c.matches(&installed_pkg.active_version))
            {
                plan.reuses.push(ReusedInstall {
                    name: name.clone(),
                    version: installed_pkg.active_version.clone(),
                });
                continue;
            }
        }

        // Otherwise: list available versions, pick highest satisfying all constraints.
        let available =
            list_versions_at_source(&source).map_err(|source_err| ResolveError::SourceListing {
                at_source: source.clone(),
                source_err,
            })?;
        let picked = available
            .iter()
            .rev() // highest first
            .find(|v| constraints.iter().all(|c| c.matches(v)))
            .cloned();
        match picked {
            Some(version) => plan.installs.push(PlannedInstall {
                name,
                version,
                source,
                requested_by,
            }),
            None => {
                return Err(ResolveError::NoCompatibleVersion {
                    name,
                    at_source: source,
                    constraints,
                    available,
                })
            }
        }
    }
    Ok(plan)
}

fn group_by_name(
    requires: &[(AgentId, RequiredTool)],
) -> HashMap<PackageName, Vec<&(AgentId, RequiredTool)>> {
    let mut groups: HashMap<PackageName, Vec<&(AgentId, RequiredTool)>> = HashMap::new();
    for entry in requires {
        groups.entry(entry.1.name.clone()).or_default().push(entry);
    }
    groups
}

#[allow(clippy::result_large_err)]
fn check_source_equality(
    name: &PackageName,
    entries: &[&(AgentId, RequiredTool)],
) -> Result<(), ResolveError> {
    let first = &entries[0].1.source;
    if entries.iter().all(|(_, t)| &t.source == first) {
        return Ok(());
    }
    let sources: Vec<PackageSource> = {
        let mut seen: Vec<PackageSource> = Vec::new();
        for (_, t) in entries {
            if !seen.contains(&t.source) {
                seen.push(t.source.clone());
            }
        }
        seen
    };
    let agents: Vec<AgentId> = entries.iter().map(|(a, _)| a.clone()).collect();
    Err(ResolveError::ConflictingSources {
        name: name.clone(),
        sources,
        agents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use tempfile::TempDir;

    fn agent_id(s: &str) -> AgentId {
        AgentId::from_str(s).unwrap()
    }

    fn pkg_name(s: &str) -> PackageName {
        PackageName::from_str(s).unwrap()
    }

    fn ver_req(s: &str) -> VersionReq {
        VersionReq::parse(s).unwrap()
    }

    fn make_source(url: &str) -> PackageSource {
        PackageSource::from_str(url).unwrap()
    }

    fn make_scope() -> (TempDir, Scope) {
        let tmp = TempDir::new().unwrap();
        let scope = Scope::global_at(tmp.path().join("tau-home")).unwrap();
        (tmp, scope)
    }

    #[test]
    fn empty_input_yields_empty_plan() {
        let (_tmp, scope) = make_scope();
        let plan = resolve_requires_tools(&[], &scope).unwrap();
        assert!(plan.installs.is_empty());
        assert!(plan.reuses.is_empty());
    }

    #[test]
    fn group_by_name_unifies_same_name_across_agents() {
        let req = RequiredTool::new(
            pkg_name("fs-read"),
            make_source("https://example.com/fs-read.git"),
            ver_req("^0.1"),
        );
        let pairs = [(agent_id("a"), req.clone()), (agent_id("b"), req.clone())];
        let groups = group_by_name(&pairs);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.get(&pkg_name("fs-read")).unwrap().len(), 2);
    }

    #[test]
    fn conflicting_sources_rejected() {
        let entries = vec![
            (
                agent_id("a"),
                RequiredTool::new(
                    pkg_name("fs-read"),
                    make_source("https://example.com/fs-read.git"),
                    ver_req("*"),
                ),
            ),
            (
                agent_id("b"),
                RequiredTool::new(
                    pkg_name("fs-read"),
                    make_source("https://other.com/fs-read.git"),
                    ver_req("*"),
                ),
            ),
        ];
        let groups = group_by_name(&entries);
        let group = groups.get(&pkg_name("fs-read")).unwrap();
        let err = check_source_equality(&pkg_name("fs-read"), group).unwrap_err();
        // `ResolveError` is not Clone, so destructure via `match` rather
        // than the let-else + .clone() trick used elsewhere. The `other =>`
        // arm prints the unexpected variant on regression — the original
        // `panic!("expected ConflictingSources")` lost that information.
        let (sources, agents) = match err {
            ResolveError::ConflictingSources {
                sources, agents, ..
            } => (sources, agents),
            other => panic!("expected ConflictingSources, got: {other:?}"),
        };
        assert_eq!(sources.len(), 2);
        assert_eq!(agents, vec![agent_id("a"), agent_id("b")]);
    }

    #[test]
    fn matching_sources_accepted() {
        let src = make_source("https://example.com/fs-read.git");
        let entries = vec![
            (
                agent_id("a"),
                RequiredTool::new(pkg_name("fs-read"), src.clone(), ver_req("^0.1")),
            ),
            (
                agent_id("b"),
                RequiredTool::new(pkg_name("fs-read"), src.clone(), ver_req("^0.1")),
            ),
        ];
        let groups = group_by_name(&entries);
        let group = groups.get(&pkg_name("fs-read")).unwrap();
        check_source_equality(&pkg_name("fs-read"), group).unwrap();
    }

    fn make_local_git_fixture(name: &str, version: &str) -> (TempDir, PackageSource) {
        use std::process::Command;
        let tempdir = TempDir::new().unwrap();
        let repo = tempdir.path().join(name);
        std::fs::create_dir(&repo).unwrap();
        let manifest_body = format!(
            r#"
name = "{name}"
version = "{version}"
description = "fixture"
authors = []
source = "https://example.com/{name}.git"
kind = "tool"
dependencies = []
capabilities = []
"#
        );
        std::fs::write(repo.join("tau.toml"), manifest_body).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "fixture"]);
        run(&["tag", &format!("v{version}")]);

        let url = format!("file://{}", repo.display());
        let source = PackageSource::from_str(&url).unwrap();
        (tempdir, source)
    }

    #[test]
    fn fresh_install_picks_highest_compatible_version() {
        let (_tmp, source) = make_local_git_fixture("fs-read", "0.1.4");
        let req = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req("^0.1"));
        let (_scope_tmp, scope) = make_scope();
        let plan = resolve_requires_tools(&[(agent_id("a"), req)], &scope).unwrap();
        assert_eq!(plan.installs.len(), 1);
        assert_eq!(plan.reuses.len(), 0);
        assert_eq!(plan.installs[0].version, Version::parse("0.1.4").unwrap());
    }

    #[test]
    fn no_compatible_version_rejected() {
        let (_tmp, source) = make_local_git_fixture("fs-read", "0.1.0");
        let req = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req("^2"));
        let (_scope_tmp, scope) = make_scope();
        let err = resolve_requires_tools(&[(agent_id("a"), req)], &scope).unwrap_err();
        let ResolveError::NoCompatibleVersion {
            name, available, ..
        } = err
        else {
            panic!("expected NoCompatibleVersion, got: {err:?}");
        };
        assert_eq!(name, pkg_name("fs-read"));
        assert_eq!(available, vec![Version::parse("0.1.0").unwrap()]);
    }

    #[test]
    fn intersection_picks_highest_satisfying_all_constraints() {
        // Fixture has v0.1.0; agent A wants ^0.1 (matches), agent B wants
        // >=0.1.0 (matches). Picked = 0.1.0.
        let (_tmp, source) = make_local_git_fixture("fs-read", "0.1.0");
        let req_a = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req("^0.1"));
        let req_b = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req(">=0.1.0"));
        let (_scope_tmp, scope) = make_scope();
        let plan =
            resolve_requires_tools(&[(agent_id("a"), req_a), (agent_id("b"), req_b)], &scope)
                .unwrap();
        assert_eq!(plan.installs.len(), 1);
        assert_eq!(plan.installs[0].version, Version::parse("0.1.0").unwrap());
        assert_eq!(plan.installs[0].requested_by.len(), 2);
    }

    #[test]
    fn conflicting_sources_propagate_to_resolve() {
        // Two different file:// URLs for the same package name → ConflictingSources.
        let (_tmp_a, source_a) = make_local_git_fixture("fs-read", "0.1.0");
        let (_tmp_b, source_b) = make_local_git_fixture("fs-read", "0.1.0");
        let req_a = RequiredTool::new(pkg_name("fs-read"), source_a, ver_req("*"));
        let req_b = RequiredTool::new(pkg_name("fs-read"), source_b, ver_req("*"));
        let (_scope_tmp, scope) = make_scope();
        let err = resolve_requires_tools(&[(agent_id("a"), req_a), (agent_id("b"), req_b)], &scope)
            .unwrap_err();
        assert!(matches!(err, ResolveError::ConflictingSources { .. }));
    }

    #[test]
    fn empty_intersection_rejected_via_no_compatible_version() {
        // Fixture v0.1.0 — agent A wants ^0.1 (matches), agent B wants ^0.2
        // (doesn't match 0.1.0). Intersection is empty for the available set.
        let (_tmp, source) = make_local_git_fixture("fs-read", "0.1.0");
        let req_a = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req("^0.1"));
        let req_b = RequiredTool::new(pkg_name("fs-read"), source.clone(), ver_req("^0.2"));
        let (_scope_tmp, scope) = make_scope();
        let err = resolve_requires_tools(&[(agent_id("a"), req_a), (agent_id("b"), req_b)], &scope)
            .unwrap_err();
        assert!(matches!(err, ResolveError::NoCompatibleVersion { .. }));
    }
}
