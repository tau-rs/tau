//! Resolution from a project `[agents.<id>]` entry to a kernel-ready
//! [`tau_domain::AgentDefinition`] paired with the resolved package's
//! [`tau_domain::PackageManifest`].
//!
//! Per spec §3.3 and §3.4: walks the per-scope lockfile to find the
//! highest-version installed package matching the entry's `package`
//! reference, reads its manifest, and materializes the `system_prompt`
//! (inline or file). The backend is resolved from the project's `[models]`
//! table via the agent's `model` alias.
//! `requires.tools` resolve+install happens at the CLI call sites
//! (cmd/run.rs, cmd/chat.rs, cmd/resolve.rs) before this function runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::{ManifestReadError, RegistryError, Scope};
use tau_domain::{AgentDefinition, AgentId, PackageId, PackageManifest, PackageName, Value};

use super::project::{AgentEntry, ModelEntry, PromptEntry};

/// Errors from [`build_agent_definition`].
///
/// Per spec §3.4. All variants carry the offending `agent_id` so the
/// CLI can emit "agent X: ..." style messages without further
/// plumbing.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AgentResolutionError {
    /// The agent's `package` reference doesn't match any installed
    /// package by name in the resolved scope.
    #[error(
        "agent {agent_id:?} package {package:?} not installed in scope \
         (run `tau install <url>`)"
    )]
    PackageNotFound {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Raw `package = "..."` reference from the entry.
        package: String,
    },

    /// Installed packages with the requested name exist but none satisfy
    /// the version requirement.
    #[error(
        "agent {agent_id:?} package {package:?} matches no installed \
         version satisfying requirement {req:?}"
    )]
    PackageVersionUnsatisfied {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Raw `package = "..."` reference from the entry.
        package: String,
        /// Stringified semver requirement.
        req: String,
    },

    /// Failed to read the resolved package's `tau.toml` manifest.
    #[error("agent {agent_id:?}: failed to read manifest: {source}")]
    ManifestRead {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Underlying manifest read / validation error.
        #[source]
        source: ManifestReadError,
    },

    /// Failed to read a `prompt.system_file`.
    #[error("agent {agent_id:?}: prompt file {path:?} read failed: {source}")]
    PromptFileRead {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Resolved (possibly relative-joined-with-project-root) path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The `package = "name@^0.1"` string failed to parse into name +
    /// semver requirement.
    #[error("agent {agent_id:?}: package reference {package:?} could not be parsed: {message}")]
    PackageParse {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Raw `package = "..."` reference.
        package: String,
        /// Human-readable parse failure message.
        message: String,
    },

    /// Failed to call into tau-pkg's registry layer (filesystem error,
    /// schema mismatch, etc.).
    #[error("agent {agent_id:?}: registry error: {source}")]
    Registry {
        /// Agent id from `[agents.<id>]`.
        agent_id: String,
        /// Underlying tau-pkg registry error.
        #[source]
        source: RegistryError,
    },

    /// Failed to construct an `AgentId` or `PackageName` from a string
    /// the project config supplied. Usually means the ASCII kebab-case
    /// invariant is violated.
    #[error("agent {agent_id:?}: invalid identifier: {message}")]
    InvalidIdentifier {
        /// Agent id from `[agents.<id>]` (may itself be the invalid id).
        agent_id: String,
        /// Human-readable message describing what was invalid.
        message: String,
    },
}

/// Resolve a project `[agents.<id>]` entry to the kernel-ready
/// `(AgentDefinition, PackageManifest)` pair.
///
/// Per spec §3.3 steps 1–6:
///
/// 1. Parse `entry.package` into `(name, semver req)`.
/// 2. List installed packages in `scope`; pick the highest version of
///    `name` satisfying the requirement.
/// 3. Read the resolved package's manifest from
///    `scope.package_dir(name, version)/tau.toml`.
/// 4. Derive the LLM backend name from the project `[models]` table via
///    `entry.model` alias. Unresolvable → `"unresolved"` placeholder.
/// 5. (Removed) `requires.tools` is no longer verified here; resolve+install
///    happens at the CLI call sites before `build_agent_definition` is called.
/// 6. Read `prompt.system` / `prompt.system_file` (path relative to
///    `project_root`) into `Option<String>`.
/// 7. Construct `AgentDefinition::new(...)` chained with
///    `.with_system_prompt(...)` and `.with_config(...)`.
///
/// Returns the manifest alongside the definition because the kernel
/// needs the manifest's capability declarations and dependencies for
/// `Runtime::run` (per the tau-runtime amendments in Tasks 2 & 3).
pub fn build_agent_definition(
    entry: &AgentEntry,
    project_root: &Path,
    scope: &Scope,
    models: &BTreeMap<String, ModelEntry>,
) -> Result<(AgentDefinition, PackageManifest), AgentResolutionError> {
    // Step 1: parse `package = "name@^0.1"` into name + semver req.
    let (raw_name, version_req) = parse_package_ref(&entry.package).map_err(|message| {
        AgentResolutionError::PackageParse {
            agent_id: entry.id.clone(),
            package: entry.package.clone(),
            message,
        }
    })?;
    let pkg_name =
        PackageName::from_str(&raw_name).map_err(|e| AgentResolutionError::InvalidIdentifier {
            agent_id: entry.id.clone(),
            message: format!("package name {raw_name:?}: {e}"),
        })?;

    // Step 2: list installed packages.
    let installed = crate::list(scope).map_err(|source| AgentResolutionError::Registry {
        agent_id: entry.id.clone(),
        source,
    })?;

    // Filter to packages with the requested name. If none match, the
    // package isn't installed at all.
    let matching: Vec<&crate::LockedPackage> = installed
        .iter()
        .filter(|pkg| pkg.name == pkg_name)
        .collect();

    if matching.is_empty() {
        return Err(AgentResolutionError::PackageNotFound {
            agent_id: entry.id.clone(),
            package: entry.package.clone(),
        });
    }

    // Pick the highest installed version satisfying the requirement,
    // searching across every matching package's `installed_versions`.
    // (In practice there's exactly one matching package, since
    // `LockedPackage::name` is a primary key — but the iteration is
    // robust to a future schema change.)
    let resolved_version = matching
        .iter()
        .flat_map(|pkg| pkg.installed_versions.iter())
        .filter(|v| version_req.matches(&v.version))
        .map(|v| &v.version)
        .max();

    let resolved_version = resolved_version
        .ok_or_else(|| AgentResolutionError::PackageVersionUnsatisfied {
            agent_id: entry.id.clone(),
            package: entry.package.clone(),
            req: version_req.to_string(),
        })?
        .clone();

    // Step 3: read the manifest from `<scope>/packages/<name>/<version>/tau.toml`.
    let manifest_path = scope
        .package_dir(&pkg_name, &resolved_version)
        .join("tau.toml");
    let manifest = crate::read_manifest(&manifest_path).map_err(|source| {
        AgentResolutionError::ManifestRead {
            agent_id: entry.id.clone(),
            source,
        }
    })?;

    // Step 4: derive backend name from the project [models] table via the
    // agent's `model` alias. Real per-agent backend resolution happens at IR
    // lowering; here we use the value only to satisfy AgentDefinition::new's
    // PackageName parameter. Fall back to "unresolved" when the alias is
    // absent so the legacy path compiles without panicking.
    let backend_str = models
        .get(&entry.model)
        .map(|m| m.backend.clone())
        .unwrap_or_default();
    let llm_name = PackageName::from_str(if backend_str.is_empty() {
        "unresolved"
    } else {
        &backend_str
    })
    .map_err(|e| AgentResolutionError::InvalidIdentifier {
        agent_id: entry.id.clone(),
        message: format!("backend from models[{:?}]: {e}", entry.model),
    })?;

    // Step 5 (verify requires.tools) was removed in Tier 2 priority 5.
    // resolve+install for requires.tools now lives at the CLI call sites
    // (cmd/run.rs, cmd/chat.rs, cmd/resolve.rs). By the time
    // build_agent_definition runs, the lockfile is authoritative.

    // Step 6: read system prompt (inline / file / none).
    let system_prompt = match &entry.prompt {
        PromptEntry::None => None,
        PromptEntry::Inline(s) => Some(s.clone()),
        PromptEntry::File(rel_or_abs) => {
            let path = if rel_or_abs.is_absolute() {
                rel_or_abs.clone()
            } else {
                project_root.join(rel_or_abs)
            };
            let contents = std::fs::read_to_string(&path).map_err(|source| {
                AgentResolutionError::PromptFileRead {
                    agent_id: entry.id.clone(),
                    path: path.clone(),
                    source,
                }
            })?;
            Some(contents)
        }
    };

    // Step 7: construct AgentDefinition.
    let agent_id =
        AgentId::from_str(&entry.id).map_err(|e| AgentResolutionError::InvalidIdentifier {
            agent_id: entry.id.clone(),
            message: format!("agent id: {e}"),
        })?;

    let pkg_id = PackageId::new(manifest.name().clone(), manifest.version().clone());

    let mut def = AgentDefinition::new(agent_id, entry.display_name.clone(), pkg_id, llm_name);
    if let Some(prompt) = system_prompt {
        def = def.with_system_prompt(prompt);
    }
    if !entry.config.is_empty() {
        let domain_config = entry
            .config
            .iter()
            .map(|(k, v)| (k.clone(), toml_value_to_domain_value(v.clone())))
            .collect();
        def = def.with_config(domain_config);
    }

    Ok((def, manifest))
}

/// Split `<name>@<semver-req>` into `(name, VersionReq)`. Bare `<name>`
/// with no `@` defaults to `*`.
pub(crate) fn parse_package_ref(package: &str) -> Result<(String, semver::VersionReq), String> {
    let (name, req_str) = match package.split_once('@') {
        Some((n, r)) => (n.trim().to_string(), r.trim().to_string()),
        None => (package.trim().to_string(), "*".to_string()),
    };
    if name.is_empty() {
        return Err("package name half is empty".to_string());
    }
    let req = semver::VersionReq::parse(&req_str)
        .map_err(|e| format!("bad semver requirement {req_str:?}: {e}"))?;
    Ok((name, req))
}

/// Convert a `toml::Value` to a `tau_domain::Value` for the
/// `[agents.<id>.config]` passthrough. Best-effort — at v0.1 the runtime
/// only validates these values per-plugin, so structural fidelity is
/// what matters.
fn toml_value_to_domain_value(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Integer(i),
        toml::Value::Float(f) => Value::Float(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            Value::Array(arr.into_iter().map(toml_value_to_domain_value).collect())
        }
        toml::Value::Table(t) => Value::Object(
            t.into_iter()
                .map(|(k, v)| (k, toml_value_to_domain_value(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::TempDir;

    use crate::project::project::{AgentEntry, PromptEntry, RequiresEntry};

    // ---- helpers ----

    /// Build an `AgentEntry` with sensible defaults that individual tests
    /// override via the `mutate` closure.
    fn entry(mutate: impl FnOnce(&mut AgentEntry)) -> AgentEntry {
        let mut e = AgentEntry {
            id: "reviewer".into(),
            display_name: "Code Reviewer".into(),
            package: "code-reviewer@^0.1".into(),
            requires: RequiresEntry::default(),
            config: BTreeMap::new(),
            prompt: PromptEntry::None,
            capability_overrides: Vec::new(),
            model: String::new(),
            tool_refs: Vec::new(),
            max_turns: None,
            max_tokens: None,
            produces: Vec::new(),
            context: Vec::new(),
            credentials: Vec::new(),
        };
        mutate(&mut e);
        e
    }

    /// Materialize an installed package (lockfile entry + on-disk
    /// `tau.toml`) at `scope.package_dir(name, version)`. Uses raw TOML
    /// I/O because `LockedPackage` / `LockedVersion` are
    /// `#[non_exhaustive]` and can't be struct-literal-constructed from
    /// outside `tau_pkg`.
    fn install_fixture(scope: &Scope, name: &str, version: &str, kind: &str, source_url: &str) {
        // 1. Write the package's tau.toml to the canonical install path.
        let pkg_name: PackageName = name.parse().unwrap();
        let ver: tau_domain::Version = version.parse().unwrap();
        let pkg_dir = scope.package_dir(&pkg_name, &ver);
        fs::create_dir_all(&pkg_dir).unwrap();
        let manifest = format!(
            r#"name = "{name}"
version = "{version}"
description = "fixture"
authors = ["tester <test@example.com>"]
source = "{source_url}"
kind = "{kind}"
dependencies = []
capabilities = []
"#
        );
        fs::write(pkg_dir.join("tau.toml"), manifest).unwrap();

        // 2. Append/upsert this version into the lockfile, preserving any
        //    existing entries (so multiple install_fixture calls compose).
        let lockfile_path = scope.lockfile_path();
        let existing = if lockfile_path.exists() {
            fs::read_to_string(&lockfile_path).unwrap()
        } else {
            String::new()
        };

        // Hand-rolled RFC3339 timestamp at second precision, anchored at
        // a fixed instant. Tests don't care about the actual time;
        // `humantime-serde` (used by tau_pkg's lockfile) accepts any
        // RFC3339 string. Using a fixed value also makes test output
        // deterministic in case anyone snapshots a lockfile.
        let now_rfc3339 = "2026-04-28T00:00:00Z";

        // We hand-author the TOML rather than going through `LockFile` /
        // `LockedPackage` because both are `#[non_exhaustive]` outside
        // tau_pkg. The schema is stable (Task 6) so this is safe.
        let resolved_commit = "0".repeat(40);
        let new_entry = format!(
            r#"
[[package]]
name = "{name}"
active_version = "{version}"
source = "{source_url}"

[[package.versions]]
version = "{version}"
resolved_commit = "{resolved_commit}"
sha256 = ""
installed_at = "{now_rfc3339}"
"#
        );

        let new_lockfile = if existing.is_empty() {
            format!(
                r#"schema_version = 1
generated_by_tau_version = "0.0.0"
generated_at = "{now_rfc3339}"
{new_entry}"#
            )
        } else {
            // Naive append: each [[package]] / [[package.versions]] block is
            // additive, so concatenating works for our test fixture needs.
            // If we add the same name twice (different versions), both
            // entries land in the lockfile — `tau_pkg::list` returns them
            // in lockfile order, and `build_agent_definition` walks all of
            // them when version-matching.
            format!("{existing}\n{new_entry}")
        };
        fs::write(&lockfile_path, new_lockfile).unwrap();
    }

    fn make_project_scope() -> (TempDir, std::path::PathBuf, Scope) {
        let tmp = TempDir::new().unwrap();
        let project_root = tmp.path().join("project");
        fs::create_dir_all(&project_root).unwrap();
        let scope = Scope::new_project(&project_root).unwrap();
        (tmp, project_root, scope)
    }

    // ---- parse_package_ref ----

    #[test]
    fn parse_package_ref_splits_name_and_req() {
        let (n, r) = parse_package_ref("code-reviewer@^0.1").unwrap();
        assert_eq!(n, "code-reviewer");
        assert!(r.matches(&"0.1.5".parse().unwrap()));
        assert!(!r.matches(&"0.2.0".parse().unwrap()));
    }

    #[test]
    fn parse_package_ref_defaults_to_star_when_no_at_sign() {
        let (n, r) = parse_package_ref("code-reviewer").unwrap();
        assert_eq!(n, "code-reviewer");
        assert!(r.matches(&"99.99.99".parse().unwrap()));
    }

    #[test]
    fn parse_package_ref_rejects_bad_semver() {
        let err = parse_package_ref("code-reviewer@not-a-semver").unwrap_err();
        assert!(err.contains("semver"), "got: {err}");
    }

    // ---- build_agent_definition: error paths (no fixture needed) ----

    #[test]
    fn build_agent_definition_returns_package_not_found_when_scope_empty() {
        let (_tmp, project_root, scope) = make_project_scope();
        let entry = entry(|_| {});

        let err = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap_err();
        match err {
            AgentResolutionError::PackageNotFound { agent_id, package } => {
                assert_eq!(agent_id, "reviewer");
                assert_eq!(package, "code-reviewer@^0.1");
            }
            other => panic!("expected PackageNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn build_agent_definition_returns_package_parse_for_bad_semver() {
        let (_tmp, project_root, scope) = make_project_scope();
        let entry = entry(|e| e.package = "code-reviewer@not-a-semver".into());

        let err = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap_err();
        assert!(
            matches!(err, AgentResolutionError::PackageParse { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn build_agent_definition_returns_invalid_identifier_for_bad_package_name() {
        let (_tmp, project_root, scope) = make_project_scope();
        // PackageName must be lowercase ASCII kebab-case; "Bad" is rejected.
        let entry = entry(|e| e.package = "Bad@^0.1".into());

        let err = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap_err();
        assert!(
            matches!(err, AgentResolutionError::InvalidIdentifier { .. }),
            "got: {err:?}"
        );
    }

    // ---- build_agent_definition: with installed packages ----

    #[test]
    fn build_agent_definition_resolves_package_at_correct_version() {
        let (_tmp, project_root, scope) = make_project_scope();
        // Install both 0.1.0 and 0.2.0; req `^0.1` must pick 0.1.0
        // (`^0.1` = >=0.1.0, <0.2.0 in semver caret-with-one-element semantics).
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );
        install_fixture(
            &scope,
            "code-reviewer",
            "0.2.0",
            "tool",
            "https://example.com/cr.git",
        );

        let entry = entry(|e| e.package = "code-reviewer@^0.1".into());
        // No models map → backend falls back to "unresolved".
        let (def, manifest) = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap();

        assert_eq!(def.id.as_str(), "reviewer");
        assert_eq!(def.display_name, "Code Reviewer");
        assert_eq!(def.package.name.as_str(), "code-reviewer");
        assert_eq!(def.package.version.to_string(), "0.1.0");
        assert_eq!(def.llm_backend.as_str(), "unresolved");
        assert_eq!(manifest.name().as_str(), "code-reviewer");
        assert_eq!(manifest.version().to_string(), "0.1.0");
    }

    #[test]
    fn build_agent_definition_returns_package_version_unsatisfied() {
        let (_tmp, project_root, scope) = make_project_scope();
        // Only 0.2.0 installed; req `^0.1` cannot be satisfied.
        install_fixture(
            &scope,
            "code-reviewer",
            "0.2.0",
            "tool",
            "https://example.com/cr.git",
        );

        let entry = entry(|e| e.package = "code-reviewer@^0.1".into());
        let err = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap_err();
        match err {
            AgentResolutionError::PackageVersionUnsatisfied { agent_id, req, .. } => {
                assert_eq!(agent_id, "reviewer");
                assert!(req.contains("0.1") || req.contains("^0.1"));
            }
            other => panic!("expected PackageVersionUnsatisfied, got: {other:?}"),
        }
    }

    /// Build a `crate::RequiredTool` for use in tests.
    ///
    /// Task 5 will lift this helper into `tests/common/mod.rs` for
    /// cross-test reuse. Placed here temporarily.
    fn make_required_tool(name: &str, source_url: &str, version: &str) -> crate::RequiredTool {
        use std::str::FromStr;
        crate::RequiredTool::new(
            tau_domain::PackageName::from_str(name).unwrap(),
            tau_domain::PackageSource::from_str(source_url).unwrap(),
            semver::VersionReq::parse(version).unwrap(),
        )
    }

    #[test]
    fn build_agent_definition_does_not_check_requires_tools() {
        // Step 5 verify was removed in Tier 2 priority 5 — the resolve+install
        // happens at cmd/run.rs / cmd/chat.rs / cmd/resolve.rs before
        // build_agent_definition is called. By the time we reach this code path,
        // the lockfile is authoritative; we no longer iterate requires.tools here.
        //
        // Confirm: a non-empty requires.tools list pointing at a package that is
        // NOT installed does NOT trip build_agent_definition — the function still
        // returns Ok.
        let (_tmp, project_root, scope) = make_project_scope();
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );
        // "fs-read" is intentionally NOT installed; Step 5 was deleted so this
        // must NOT cause an error.
        let tool = make_required_tool("fs-read", "https://example.com/fs-read.git", "^0.1");

        let entry = entry(|e| e.requires.tools = vec![tool]);
        // Should succeed: requires.tools is no longer checked at this layer.
        let (def, _manifest) = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap();
        assert_eq!(def.id.as_str(), "reviewer");
    }

    #[test]
    fn build_agent_definition_reads_system_prompt_from_file() {
        let (_tmp, project_root, scope) = make_project_scope();
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );

        // Drop a prompt file at <project_root>/prompts/reviewer.md.
        let prompt_dir = project_root.join("prompts");
        fs::create_dir_all(&prompt_dir).unwrap();
        let prompt_path = prompt_dir.join("reviewer.md");
        fs::write(&prompt_path, "You are a careful reviewer.").unwrap();

        let entry = entry(|e| {
            e.prompt = PromptEntry::File("prompts/reviewer.md".into());
        });
        let (def, _manifest) = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap();
        assert_eq!(
            def.system_prompt.as_deref(),
            Some("You are a careful reviewer.")
        );
    }

    #[test]
    fn build_agent_definition_returns_prompt_file_read_on_missing_file() {
        let (_tmp, project_root, scope) = make_project_scope();
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );

        let entry = entry(|e| {
            e.prompt = PromptEntry::File("prompts/missing.md".into());
        });
        let err = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap_err();
        match err {
            AgentResolutionError::PromptFileRead { agent_id, path, .. } => {
                assert_eq!(agent_id, "reviewer");
                assert!(path.ends_with("prompts/missing.md"));
            }
            other => panic!("expected PromptFileRead, got: {other:?}"),
        }
    }

    #[test]
    fn build_agent_definition_uses_inline_prompt_when_supplied() {
        let (_tmp, project_root, scope) = make_project_scope();
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );

        let entry = entry(|e| e.prompt = PromptEntry::Inline("be helpful".into()));
        let (def, _) = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap();
        assert_eq!(def.system_prompt.as_deref(), Some("be helpful"));
    }

    #[test]
    fn build_agent_definition_passes_through_config_table() {
        let (_tmp, project_root, scope) = make_project_scope();
        install_fixture(
            &scope,
            "code-reviewer",
            "0.1.0",
            "tool",
            "https://example.com/cr.git",
        );

        let entry = entry(|e| {
            let mut cfg = BTreeMap::new();
            cfg.insert(
                "model".to_string(),
                toml::Value::String("claude-3".to_string()),
            );
            cfg.insert("max_tokens".to_string(), toml::Value::Integer(4096));
            e.config = cfg;
        });

        let (def, _) = build_agent_definition(&entry, &project_root, &scope, &BTreeMap::new()).unwrap();
        assert_eq!(def.config.len(), 2);
        assert_eq!(
            def.config.get("model").and_then(Value::as_string),
            Some("claude-3")
        );
        assert_eq!(
            def.config.get("max_tokens").and_then(Value::as_integer),
            Some(4096)
        );
    }

    // ---- toml_value_to_domain_value ----

    #[test]
    fn toml_value_conversion_round_trips_scalars() {
        assert_eq!(
            toml_value_to_domain_value(toml::Value::String("x".into())),
            Value::String("x".into())
        );
        assert_eq!(
            toml_value_to_domain_value(toml::Value::Integer(42)),
            Value::Integer(42)
        );
        assert_eq!(
            toml_value_to_domain_value(toml::Value::Boolean(true)),
            Value::Bool(true)
        );
    }

    #[test]
    fn toml_value_conversion_handles_nested_table() {
        let mut table = toml::value::Table::new();
        table.insert("k".into(), toml::Value::String("v".into()));
        let converted = toml_value_to_domain_value(toml::Value::Table(table));
        match converted {
            Value::Object(map) => {
                assert_eq!(map.get("k"), Some(&Value::String("v".into())));
            }
            other => panic!("expected Object, got {other:?}"),
        }
    }
}
