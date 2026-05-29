//! `tau build` producer — gathers a fully-installed project's
//! resolved state into a §C.1 bundle artifact.
//!
//! See spec `2026-05-27-tau-build-design.md` and ADR-0035.

use std::path::PathBuf;
use std::str::FromStr;

use tau_ports::target::TargetTriple;

use crate::bundle::build_error::BuildError;
use crate::bundle::manifest::{
    BackendRef, BundleAgent, BundleEffectiveCapabilities, BundleManifest, BundleMeta,
    BundlePackage, ProjectInfo,
};
use crate::project::project::{PromptEntry, UncheckedProjectConfig};

/// Inputs to [`build`].
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Path to the project root (the directory containing `tau.toml`).
    pub project_root: PathBuf,
    /// Target triple to bake into the bundle. Use
    /// [`TargetTriple::host`] for the default.
    pub target: TargetTriple,
    /// Optional explicit output path. When `None`, defaults to
    /// `<project_root>/<project-name>-<project-version>.tau`.
    pub output_path: Option<PathBuf>,
    /// Restrict the bundle to these agents and prune packages they don't
    /// reference. `None` builds every agent and keeps every package (the
    /// §C.2 behavior). The CLI maps an empty `--agent` set to `None`.
    pub agent_filter: Option<Vec<tau_domain::AgentId>>,
}

/// Result of a successful build.
#[derive(Debug, Clone)]
pub struct BundleArtifact {
    /// Absolute path to the written bundle file.
    pub path: PathBuf,
    /// The bundle's self-hash (hex SHA-256).
    pub sha256: String,
    /// On-disk size of the bundle in bytes.
    pub size_bytes: u64,
}

/// Build a bundle from the project at [`BuildOptions::project_root`].
///
/// Strict mode: returns [`BuildError::MissingLockfile`] or
/// [`BuildError::PackageNotInstalled`] if the project isn't fully
/// installed. The function does NOT attempt to install anything.
pub fn build(opts: BuildOptions) -> Result<BundleArtifact, BuildError> {
    // Step 1: Load tau.toml. Parse via the typed UncheckedProjectConfig
    // and then validate — this is the same pipeline `tau run` uses, so
    // the bundle records exactly what the project config layer would
    // surface to the runtime.
    let tau_toml_path = opts.project_root.join("tau.toml");
    let tau_toml_bytes = std::fs::read(&tau_toml_path)
        .map_err(|e| BuildError::ProjectConfig(format!("read {tau_toml_path:?}: {e}")))?;
    let tau_toml_str = std::str::from_utf8(&tau_toml_bytes)
        .map_err(|e| BuildError::ProjectConfig(format!("tau.toml is not utf-8: {e}")))?;
    let unchecked: UncheckedProjectConfig = toml::from_str(tau_toml_str)
        .map_err(|e| BuildError::ProjectConfig(format!("parse {tau_toml_path:?}: {e}")))?;
    let project_config = unchecked
        .validate()
        .map_err(|e| BuildError::ProjectConfig(format!("validate {tau_toml_path:?}: {e}")))?;

    // Step 2: Load tau.lock. Distinguish missing (run `tau install`)
    // from present-but-invalid (config error).
    let lockfile_path = opts.project_root.join("tau.lock");
    if !lockfile_path.exists() {
        return Err(BuildError::MissingLockfile);
    }
    let _lockfile = crate::lockfile::LockFile::load(&lockfile_path)
        .map_err(|e| BuildError::LockfileLoad(e.to_string()))?;

    // Step 3: Verify every locked package is materialized on disk.
    //
    // Install layout (per `Scope::package_dir`, see `crates/tau-pkg/src/scope.rs`):
    //     <project_root>/.tau/packages/<name>/<version>/
    //
    // The build is per-project, so the canonical install root is the
    // project scope's `state_path` (= `<project_root>/.tau`). We
    // compute the path directly rather than calling
    // `Scope::new_project`, which would side-effect by creating
    // `.tau/` and writing a default config — undesirable for a
    // read-only build pipeline that's supposed to fail loudly if the
    // project isn't installed.
    let packages_root = opts.project_root.join(".tau").join("packages");
    for pkg in &_lockfile.packages {
        let pkg_dir = packages_root
            .join(pkg.name.as_str())
            .join(pkg.active_version.to_string());
        if !pkg_dir.exists() {
            return Err(BuildError::PackageNotInstalled {
                name: pkg.name.as_str().to_owned(),
                path: pkg_dir,
            });
        }
    }

    // Step 4: Gather package facts. One entry per LockedPackage.
    // Recompute `tree_sha256` from the installed directory (the
    // bundle's hash, not the install-time hash recorded in the
    // lockfile — they should agree, but `tau verify` is the gate that
    // enforces that; here we record the on-disk truth). Pull
    // `binary_sha256` + `required_shapes` from the optional
    // `LockedPackage.plugin` sub-struct; skill-only and data-only
    // packages get `None` / empty.
    let mut packages: Vec<crate::bundle::manifest::BundlePackage> = Vec::new();
    for pkg in &_lockfile.packages {
        let pkg_dir = packages_root
            .join(pkg.name.as_str())
            .join(pkg.active_version.to_string());
        let tree_sha256 =
            crate::tree_hash::tree_hash(&pkg_dir).map_err(|e| BuildError::TreeHashFailed {
                name: pkg.name.as_str().to_owned(),
                source: e,
            })?;
        // `LockedPlugin.binary_sha256` is a `String` (empty for v2
        // leftovers). `BundlePackage.binary_sha256` is `Option<String>`
        // — translate empty → None so the bundle JSON skips the field
        // for skill-only / unverified entries.
        let (binary_sha256, required_shapes) = match &pkg.plugin {
            Some(p) => {
                let bin = if p.binary_sha256.is_empty() {
                    None
                } else {
                    Some(p.binary_sha256.clone())
                };
                (bin, p.required_shapes.clone())
            }
            None => (None, Vec::new()),
        };
        packages.push(crate::bundle::manifest::BundlePackage {
            name: pkg.name.as_str().to_owned(),
            version: pkg.active_version.clone(),
            source: pkg.source.clone(),
            tree_sha256,
            binary_sha256,
            required_shapes,
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    // Step 5: Gather agent facts. One entry per validated AgentEntry in
    // the project's `[agents.<id>]` table. For each agent:
    //
    // - Resolve `[agents.<id>.prompt]` to bytes (inline string or file
    //   contents from `prompt.system_file` relative to project_root)
    //   and hash to SHA-256.
    // - Carry `requires.tools` as a sorted list of tool names — these
    //   are the typed deps the project config layer already resolved.
    // - When the agent has project-level capability overrides, compute
    //   the effective grant set by intersecting against the package's
    //   manifest grants. v1 happy path has no overrides, so the
    //   `package_caps` union is a stub. See `collect_package_caps`.
    let mut agents: Vec<BundleAgent> = Vec::new();
    for (id, entry) in &project_config.agents {
        let agent_id = tau_domain::AgentId::from_str(id).map_err(|e| {
            BuildError::ManifestInvalid(format!("agent `{id}` has invalid id: {e}"))
        })?;

        // Resolve the system prompt to bytes via the shared helper that
        // `tau verify` (bundle/verify.rs step 8) ALSO calls — keeping the
        // two byte-for-byte identical so a clean verify never spuriously
        // fails on a prompt.
        let prompt_bytes: Vec<u8> = resolve_agent_prompt_bytes(&entry.prompt, &opts.project_root)
            .map_err(|source| BuildError::PromptResolveFailed {
            id: id.clone(),
            source,
        })?;
        let system_prompt_sha256 = sha256_hex(&prompt_bytes);

        // List required tool names. BTreeMap iteration above is sorted
        // by agent id; sort tool names here for stable output.
        let mut required_tools: Vec<String> = entry
            .requires
            .tools
            .iter()
            .map(|t| t.name.as_str().to_owned())
            .collect();
        required_tools.sort();

        // Compute effective capabilities. v1 happy path: no overrides ⇒
        // skip entirely (leaves BundleEffectiveCapabilities::default()).
        // When an override IS present, collect the package-manifest
        // grant union (currently a stub — see collect_package_caps) and
        // call compute_effective.
        let effective_capabilities = if entry.capability_overrides.is_empty() {
            BundleEffectiveCapabilities::default()
        } else {
            let package_caps = collect_package_caps(&packages, &required_tools)?;
            let eff = crate::capability_override::compute_effective(
                &package_caps,
                &entry.capability_overrides,
            )
            .map_err(|source| BuildError::CapabilityOverrideFailed {
                id: id.clone(),
                source,
            })?;
            effective_to_bundle(&eff)
        };

        agents.push(BundleAgent {
            id: agent_id,
            backend: BackendRef {
                kind: entry.llm_backend.clone(),
                model: None,
                extra: std::collections::BTreeMap::new(),
            },
            system_prompt_sha256,
            required_tools,
            effective_capabilities,
        });
    }
    // BTreeMap iteration above is already sorted by key; defensive
    // resort guards against future refactors swapping the container.
    agents.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    // Step 5.5: per-agent slicing + package pruning (spec §C.2.2).
    //
    // When `agent_filter` is None this block is skipped entirely and the
    // full agent + package sets pass through (the §C.2 behavior). When
    // Some, keep only the named agents and prune packages to their
    // direct reference closure, then record the slice so `tau verify
    // --bundle` can replay it.
    let selected_agents: Option<Vec<String>> = match &opts.agent_filter {
        None => None,
        Some(wanted) => {
            // Validate every requested id against the project config so
            // the error fires even for agents a later step would reject.
            let mut available: Vec<String> = project_config.agents.keys().cloned().collect();
            available.sort();
            for id in wanted {
                if !project_config.agents.contains_key(id.as_str()) {
                    return Err(BuildError::UnknownAgent {
                        id: id.as_str().to_owned(),
                        available,
                    });
                }
            }

            let wanted_set: std::collections::BTreeSet<&str> =
                wanted.iter().map(|a| a.as_str()).collect();

            // Keep only the selected agents.
            agents.retain(|a| wanted_set.contains(a.id.as_str()));

            // Package keep-set: each kept agent's home package
            // (parsed from `[agents.<id>].package`) ∪ its required tools.
            // The flat lockfile has no inter-package deps, so direct
            // reference closure is complete (spec §4.2 assumption).
            let mut keep: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for a in &agents {
                if let Some(entry) = project_config.agents.get(a.id.as_str()) {
                    // Empty / unparseable package field contributes
                    // nothing — never a new failure vs. the full build.
                    if let Ok((name, _req)) =
                        crate::project::agent::parse_package_ref(&entry.package)
                    {
                        keep.insert(name);
                    }
                }
                for t in &a.required_tools {
                    keep.insert(t.clone());
                }
            }
            packages.retain(|p| keep.contains(&p.name));

            // Record the requested ids as the slice marker, sorted +
            // deduped so a caller passing `--agent a --agent a` yields a
            // stable, canonical marker (and thus a stable self-hash).
            let mut sel: Vec<String> = wanted.iter().map(|a| a.as_str().to_owned()).collect();
            sel.sort();
            sel.dedup();
            Some(sel)
        }
    };

    // Step 6: Assemble the manifest.
    //
    // The `tau.toml` schema does not currently carry a `[project]
    // version` field — `UncheckedProject` only has `name` and
    // `description`. The bundle manifest requires `project.version` as a
    // `semver::Version`, so we look for an optional `[project].version`
    // value in the raw TOML and fall back to `0.0.0` when absent. This
    // is intentional: the bundle records what's there; project authors
    // who want a meaningful version can add the field and it round-trips
    // through serde's unknown-field-tolerant deserialization.
    let project_version = extract_project_version(tau_toml_str)?;

    let tau_toml_sha256 = sha256_hex(&tau_toml_bytes);

    let mut manifest = BundleManifest {
        schema_version: 1,
        bundle: BundleMeta {
            // Placeholder — filled below after self-hash compute.
            sha256: String::new(),
            created_at: humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string(),
            tau_version: env!("CARGO_PKG_VERSION").to_string(),
            target: opts.target,
            selected_agents,
        },
        project: ProjectInfo {
            name: project_config.project_name.clone(),
            version: project_version,
            tau_toml_sha256,
        },
        packages,
        agents,
    };

    // Compute and fill the self-hash. `compute_self_hash` zeros out
    // both `bundle.sha256` and `bundle.created_at` in its canonical
    // serialization, so two builds of the same sources produce the
    // same hash (see T1 / spec §3.1).
    manifest.bundle.sha256 = crate::bundle::hash::compute_self_hash(&manifest);

    // Step 7: Canonical-TOML emit + write.
    let canonical = manifest.to_canonical_toml();
    let output_path = opts.output_path.unwrap_or_else(|| {
        opts.project_root.join(format!(
            "{}-{}.tau",
            manifest.project.name, manifest.project.version,
        ))
    });
    std::fs::write(&output_path, canonical.as_bytes()).map_err(|source| {
        BuildError::WriteFailed {
            path: output_path.clone(),
            source,
        }
    })?;
    let size_bytes = std::fs::metadata(&output_path)
        .map_err(|source| BuildError::WriteFailed {
            path: output_path.clone(),
            source,
        })?
        .len();
    Ok(BundleArtifact {
        path: output_path,
        sha256: manifest.bundle.sha256,
        size_bytes,
    })
}

/// Pull a `[project].version` string out of the raw tau.toml, parse as
/// semver, and fall back to `0.0.0` when absent.
///
/// The validated `ProjectConfig` doesn't carry a version — see comment
/// in [`build`] step 6.
fn extract_project_version(tau_toml: &str) -> Result<semver::Version, BuildError> {
    let value: toml::Value = toml::from_str(tau_toml)
        .map_err(|e| BuildError::ProjectConfig(format!("re-parse tau.toml: {e}")))?;
    let version_str = value
        .get("project")
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str());
    match version_str {
        Some(s) => semver::Version::parse(s).map_err(|e| {
            BuildError::ManifestInvalid(format!("project.version {s:?} is not valid semver: {e}"))
        }),
        None => Ok(semver::Version::new(0, 0, 0)),
    }
}

/// Collect the union of package-manifest capability grants for the
/// tools listed in `required_tools`.
///
/// MVP stub: returns an empty `Vec`. The v1 happy path has no per-agent
/// `[[capabilities]]` overrides, so this path is unreachable in shipped
/// configurations. A complete implementation should load each required
/// tool's manifest from `.tau/packages/<name>/<version>/tau.toml` and
/// union the `[plugin]`/`[tool]` capabilities. Flag in PR description
/// as a known follow-up.
fn collect_package_caps(
    _packages: &[BundlePackage],
    _required_tools: &[String],
) -> Result<Vec<tau_domain::Capability>, BuildError> {
    Ok(Vec::new())
}

/// Flatten [`crate::capability_override::EffectiveCapability`] entries into
/// the per-shape allow/deny lists of [`BundleEffectiveCapabilities`].
///
/// For each entry: the allow list is the narrowed `allow_override` if present,
/// otherwise the source variant's own field. `deny` is always `e.deny`.
/// `Filesystem(Exec)` and `Process(Spawn)` both map to `allow_exec`/`deny_exec`.
/// Shapes with no bundle representation (skill.spawn, task_list, plan, custom)
/// are silently dropped via the catch-all arm.
fn effective_to_bundle(
    eff: &[crate::capability_override::EffectiveCapability],
) -> BundleEffectiveCapabilities {
    use tau_domain::{AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability};
    let mut out = BundleEffectiveCapabilities::default();
    for e in eff {
        match &e.source {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => {
                out.allow_fs_read
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_read.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Write { paths, .. }) => {
                out.allow_fs_write
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_fs_write.extend(e.deny.clone());
            }
            Capability::Filesystem(FsCapability::Exec { paths, .. }) => {
                out.allow_exec
                    .extend(e.allow_override.clone().unwrap_or_else(|| paths.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Process(ProcessCapability::Spawn { commands, .. }) => {
                out.allow_exec
                    .extend(e.allow_override.clone().unwrap_or_else(|| commands.clone()));
                out.deny_exec.extend(e.deny.clone());
            }
            Capability::Network(NetCapability::Http { hosts, .. }) => {
                out.allow_net_http
                    .extend(e.allow_override.clone().unwrap_or_else(|| hosts.clone()));
                out.deny_net_http.extend(e.deny.clone());
            }
            Capability::Agent(AgentCapability::Spawn { allowed_kinds, .. }) => {
                out.allow_agent_spawn
                    .extend(e.allow_override.clone().unwrap_or_else(|| allowed_kinds.clone()));
                out.deny_agent_spawn.extend(e.deny.clone());
            }
            // skill.spawn / task_list / plan / custom: no bundle field.
            _ => {}
        }
    }
    out
}

/// Resolve an agent's `[agents.<id>.prompt]` entry to the exact bytes
/// that get SHA-256-hashed into `BundleAgent.system_prompt_sha256`.
///
/// This is the SINGLE source of truth for prompt-byte resolution. Both
/// the build pipeline (step 5 above) and `tau verify` (bundle/verify.rs
/// step 8) call it, guaranteeing their hashes can never drift:
///
/// - [`PromptEntry::Inline`] → the inline string's UTF-8 bytes.
/// - [`PromptEntry::File`] → the file's raw bytes, resolved relative to
///   `project_root` when the path is relative.
/// - [`PromptEntry::None`] → empty bytes (hashes to the SHA-256 of the
///   empty string), matching §C.2's "no prompt ⇒ hash empty" rule.
pub(crate) fn resolve_agent_prompt_bytes(
    prompt: &PromptEntry,
    project_root: &std::path::Path,
) -> Result<Vec<u8>, std::io::Error> {
    match prompt {
        PromptEntry::Inline(s) => Ok(s.clone().into_bytes()),
        PromptEntry::File(rel) => {
            let abs = if rel.is_absolute() {
                rel.clone()
            } else {
                project_root.join(rel)
            };
            std::fs::read(&abs)
        }
        PromptEntry::None => Ok(Vec::new()),
    }
}

/// SHA-256 of `bytes` as lowercase hex. The single source of truth for
/// every bundle hash (tau_toml_sha256, system_prompt_sha256, and the
/// verify-side recomputations). Build and verify MUST use this same fn
/// so their hashes can never drift.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    crate::tree_hash::to_hex_lower(h.finalize().as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::target::TargetTriple;
    use tempfile::tempdir;

    fn opts(root: &std::path::Path) -> BuildOptions {
        BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
        }
    }

    #[test]
    fn build_fails_on_missing_project_toml() {
        let tmp = tempdir().unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        assert!(
            matches!(err, BuildError::ProjectConfig(_)),
            "expected ProjectConfig, got {err:?}",
        );
    }

    #[test]
    fn build_fails_on_missing_lockfile() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "0.1.0"
"#,
        )
        .unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        assert!(
            matches!(err, BuildError::MissingLockfile),
            "expected MissingLockfile, got {err:?}",
        );
    }

    #[test]
    fn build_fails_when_locked_package_dir_missing() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "0.1.0"
"#,
        )
        .unwrap();
        // Minimal lockfile (schema v6) naming one package whose
        // installed dir does not exist anywhere on disk.
        let lockfile_toml = r#"
schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "ghost-plugin"
active_version = "0.1.0"
source = "https://example.com/ghost.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#;
        std::fs::write(tmp.path().join("tau.lock"), lockfile_toml).unwrap();
        let err = build(opts(tmp.path())).unwrap_err();
        match err {
            BuildError::PackageNotInstalled { name, .. } => {
                assert_eq!(name, "ghost-plugin");
            }
            other => panic!("expected PackageNotInstalled, got {other:?}"),
        }
    }

    #[test]
    fn build_progresses_past_install_verification_with_sorted_packages() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "0.1.0"
"#,
        )
        .unwrap();
        // Two packages in non-alphabetical order: "zeta" then "alpha".
        // Both directories exist (empty trees → tree_hash returns a
        // known constant).
        let pkg_dir_zeta = tmp.path().join(".tau/packages/zeta/0.1.0");
        let pkg_dir_alpha = tmp.path().join(".tau/packages/alpha/0.1.0");
        std::fs::create_dir_all(&pkg_dir_zeta).unwrap();
        std::fs::create_dir_all(&pkg_dir_alpha).unwrap();

        // Write a v6 lockfile with both packages, "zeta" first then
        // "alpha", to prove that step 4 sorts them alphabetically.
        let lockfile_toml = r#"
schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "zeta"
active_version = "0.1.0"
source = "https://example.com/zeta.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000002"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "alpha"
active_version = "0.1.0"
source = "https://example.com/alpha.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#;
        std::fs::write(tmp.path().join("tau.lock"), lockfile_toml).unwrap();

        // With steps 6+7 implemented, build now succeeds. Read the
        // bundle back and confirm step 4 sorted the packages
        // alphabetically (alpha before zeta).
        let artifact = build(opts(tmp.path())).expect("build succeeds");
        let bundle_str = std::fs::read_to_string(&artifact.path).unwrap();
        let m =
            crate::bundle::manifest::BundleManifest::parse_str(&bundle_str).expect("bundle parses");
        assert_eq!(m.packages.len(), 2);
        assert_eq!(m.packages[0].name, "alpha");
        assert_eq!(m.packages[1].name, "zeta");
    }

    #[test]
    fn build_progresses_past_agent_gather_with_sorted_agents() {
        let tmp = tempdir().unwrap();
        // Two agents in non-alphabetical order: "zeta" then "alpha".
        // Step 5 must hash each prompt, carry the backend, and emit a
        // sorted Vec. The lockfile is empty (no packages) — step 4
        // builds an empty Vec, step 5 builds a 2-element Vec.
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"

[agents.zeta]
display_name = "Zeta"
package      = "p@^0.1"
llm_backend  = "anthropic"

[agents.zeta.prompt]
system = "you are zeta"

[agents.alpha]
display_name = "Alpha"
package      = "p@^0.1"
llm_backend  = "anthropic"

[agents.alpha.prompt]
system = "you are alpha"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();

        // With steps 6+7 implemented, build now succeeds. Read the
        // bundle back and confirm step 5 sorted the agents
        // alphabetically (alpha before zeta).
        let artifact = build(opts(tmp.path())).expect("build succeeds");
        let bundle_str = std::fs::read_to_string(&artifact.path).unwrap();
        let m =
            crate::bundle::manifest::BundleManifest::parse_str(&bundle_str).expect("bundle parses");
        assert_eq!(m.agents.len(), 2);
        assert_eq!(m.agents[0].id.as_str(), "alpha");
        assert_eq!(m.agents[1].id.as_str(), "zeta");
    }

    #[test]
    fn build_resolves_system_prompt_from_file() {
        // Agent uses `prompt.system_file` — step 5 must read the file
        // relative to project_root.
        let tmp = tempdir().unwrap();
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(prompts_dir.join("r.md"), b"file prompt body").unwrap();

        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"

[agents.r]
display_name = "R"
package      = "p@^0.1"
llm_backend  = "anthropic"

[agents.r.prompt]
system_file = "prompts/r.md"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();

        // With steps 6+7 implemented, build succeeds. Confirm the
        // file-resolved system prompt was hashed (not the empty hash)
        // and that step 5 produced one agent.
        let artifact = build(opts(tmp.path())).expect("build succeeds");
        let bundle_str = std::fs::read_to_string(&artifact.path).unwrap();
        let m =
            crate::bundle::manifest::BundleManifest::parse_str(&bundle_str).expect("bundle parses");
        assert_eq!(m.agents.len(), 1);
        // SHA-256 of "file prompt body" — deterministic check that
        // step 5 actually read the file and hashed its bytes.
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"file prompt body");
        let want = crate::tree_hash::to_hex_lower(h.finalize().as_slice());
        assert_eq!(m.agents[0].system_prompt_sha256, want);
    }

    /// Two-agent project: alpha→requires pkg-a, beta→requires pkg-b,
    /// both home package pkg-home. All three package dirs exist so
    /// step-3 install verification passes.
    fn two_agent_project(tmp: &std::path::Path) {
        std::fs::write(
            tmp.join("tau.toml"),
            r#"
[project]
name = "multi"
version = "0.1.0"

[agents.alpha]
display_name = "Alpha"
package = "pkg-home@^0.1"
llm_backend = "anthropic"

[agents.alpha.prompt]
system = "you are alpha"

[[agents.alpha.requires.tools]]
name = "pkg-a"
source = "https://example.com/pkg-a.git"

[agents.beta]
display_name = "Beta"
package = "pkg-home@^0.1"
llm_backend = "anthropic"

[agents.beta.prompt]
system = "you are beta"

[[agents.beta.requires.tools]]
name = "pkg-b"
source = "https://example.com/pkg-b.git"
"#,
        )
        .unwrap();
        for name in ["pkg-a", "pkg-b", "pkg-home"] {
            std::fs::create_dir_all(tmp.join(format!(".tau/packages/{name}/0.1.0"))).unwrap();
        }
        std::fs::write(
            tmp.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-a"
active_version = "0.1.0"
source = "https://example.com/pkg-a.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-b"
active_version = "0.1.0"
source = "https://example.com/pkg-b.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000002"
installed_at = "2024-01-01T00:00:00Z"

[[package]]
name = "pkg-home"
active_version = "0.1.0"
source = "https://example.com/pkg-home.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000003"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
    }

    fn opts_filtered(root: &std::path::Path, ids: &[&str]) -> BuildOptions {
        BuildOptions {
            project_root: root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: Some(ids.iter().map(|s| s.parse().unwrap()).collect()),
        }
    }

    fn read_bundle(path: &std::path::Path) -> crate::bundle::manifest::BundleManifest {
        let s = std::fs::read_to_string(path).unwrap();
        crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap()
    }

    #[test]
    fn build_agent_filter_none_keeps_all() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts(tmp.path())).unwrap().path);
        assert_eq!(m.agents.len(), 2);
        assert_eq!(m.packages.len(), 3);
        assert_eq!(m.bundle.selected_agents, None);
    }

    #[test]
    fn build_agent_filter_selects_single_agent() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha"])).unwrap().path);
        assert_eq!(m.agents.len(), 1);
        assert_eq!(m.agents[0].id.as_str(), "alpha");
        assert_eq!(m.bundle.selected_agents, Some(vec!["alpha".to_string()]));
    }

    #[test]
    fn build_agent_filter_prunes_unreferenced_packages() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha"])).unwrap().path);
        let names: Vec<&str> = m.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"pkg-a"), "got {names:?}");
        assert!(names.contains(&"pkg-home"), "got {names:?}");
        assert!(
            !names.contains(&"pkg-b"),
            "pkg-b must be pruned; got {names:?}"
        );
    }

    #[test]
    fn build_agent_filter_keeps_home_package() {
        // An agent's home package (from `[agents.<id>].package`) is kept
        // even though it is not a required tool. Spec §7.
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(&build(opts_filtered(tmp.path(), &["alpha"])).unwrap().path);
        let names: Vec<&str> = m.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"pkg-home"),
            "home package must be retained even without a required-tool reference; got {names:?}"
        );
    }

    #[test]
    fn build_agent_filter_multiple_agents_unions_packages() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let m = read_bundle(
            &build(opts_filtered(tmp.path(), &["alpha", "beta"]))
                .unwrap()
                .path,
        );
        assert_eq!(m.agents.len(), 2);
        let names: Vec<&str> = m.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"pkg-a") && names.contains(&"pkg-b") && names.contains(&"pkg-home"),
            "got {names:?}"
        );
        assert_eq!(
            m.bundle.selected_agents,
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
    }

    #[test]
    fn build_agent_filter_unknown_id_errors() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let err = build(opts_filtered(tmp.path(), &["ghost"])).unwrap_err();
        match err {
            BuildError::UnknownAgent { id, available } => {
                assert_eq!(id, "ghost");
                assert_eq!(available, vec!["alpha".to_string(), "beta".to_string()]);
            }
            other => panic!("expected UnknownAgent, got {other:?}"),
        }
    }

    #[test]
    fn build_agent_filter_is_reproducible() {
        let tmp = tempdir().unwrap();
        two_agent_project(tmp.path());
        let a = build(opts_filtered(tmp.path(), &["alpha"])).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = build(opts_filtered(tmp.path(), &["alpha"])).unwrap();
        assert_eq!(a.sha256, b.sha256, "sliced build self-hash must be stable");
    }

    /// Canonical project + lockfile pair: one agent, no packages.
    /// Used by the step-6/7 tests.
    fn happy_path_project(tmp: &std::path::Path) {
        std::fs::write(
            tmp.join("tau.toml"),
            r#"
[project]
name = "test-project"
version = "1.2.3"

[agents.alpha]
display_name = "Alpha"
package      = "p@^0.1"
llm_backend  = "anthropic"

[agents.alpha.prompt]
system = "you are alpha"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
    }

    #[test]
    fn build_writes_bundle_and_self_hash_verifies() {
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());

        let artifact = build(opts(tmp.path())).expect("build succeeded");
        assert!(artifact.path.exists(), "bundle file written");
        assert_eq!(artifact.sha256.len(), 64, "sha256 is 64 hex chars");
        assert!(artifact.size_bytes > 0, "bundle is non-empty");

        // Parse the bundle back and verify the self-hash.
        let bundle_str = std::fs::read_to_string(&artifact.path).unwrap();
        let m =
            crate::bundle::manifest::BundleManifest::parse_str(&bundle_str).expect("bundle parses");
        crate::bundle::hash::verify_self_hash(&m).expect("self-hash verifies");

        // Default output path is `<name>-<version>.tau` at project root.
        assert_eq!(artifact.path, tmp.path().join("test-project-1.2.3.tau"),);
    }

    #[test]
    fn build_excludes_created_at_from_bundle_self_hash() {
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());

        let a = build(opts(tmp.path())).expect("build 1");
        // Sleep so that any naive `created_at`-inclusive hash differs
        // between the two runs.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = build(opts(tmp.path())).expect("build 2");
        assert_eq!(
            a.sha256, b.sha256,
            "self-hash must be stable across builds (created_at is excluded)"
        );
    }

    #[test]
    fn build_writes_to_explicit_output_path_when_set() {
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());

        let explicit = tmp.path().join("custom.tau");
        let o = BuildOptions {
            project_root: tmp.path().to_path_buf(),
            target: TargetTriple::host(),
            output_path: Some(explicit.clone()),
            agent_filter: None,
        };
        let artifact = build(o).expect("build");
        assert_eq!(artifact.path, explicit);
        assert!(explicit.exists());
    }

    #[test]
    fn build_fails_when_system_prompt_file_missing() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "test-project"

[agents.r]
display_name = "R"
package      = "p@^0.1"
llm_backend  = "anthropic"

[agents.r.prompt]
system_file = "prompts/missing.md"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();

        let err = build(opts(tmp.path())).unwrap_err();
        match err {
            BuildError::PromptResolveFailed { id, .. } => assert_eq!(id, "r"),
            other => panic!("expected PromptResolveFailed, got {other:?}"),
        }
    }

    // Helper: parse a Capability from its serialized kind/field JSON
    // (uses serde deserialization since Capability variants are #[non_exhaustive]
    // in tau-domain and cannot be constructed via struct-literal from tau-pkg tests).
    fn cap(json: serde_json::Value) -> tau_domain::Capability {
        serde_json::from_value(json).expect("capability parse")
    }

    // Helper: construct an EffectiveCapability.
    fn eff(
        source: tau_domain::Capability,
        allow_override: Option<Vec<&str>>,
        deny: &[&str],
    ) -> crate::capability_override::EffectiveCapability {
        crate::capability_override::EffectiveCapability {
            source,
            allow_override: allow_override.map(|v| v.iter().map(|s| s.to_string()).collect()),
            deny: deny.iter().map(|s| s.to_string()).collect(),
            max_bytes_override: None,
        }
    }

    #[test]
    fn effective_to_bundle_uses_override_allow_and_deny() {
        let e = eff(
            cap(serde_json::json!({"kind": "fs.read", "paths": ["/data/**", "/tmp/**"]})),
            Some(vec!["/data/**"]),
            &["/data/secret/**"],
        );
        let b = effective_to_bundle(&[e]);
        assert_eq!(b.allow_fs_read, vec!["/data/**".to_string()]);
        assert_eq!(b.deny_fs_read, vec!["/data/secret/**".to_string()]);
    }

    #[test]
    fn effective_to_bundle_falls_back_to_source_when_no_override() {
        let e = eff(
            cap(serde_json::json!({"kind": "net.http", "hosts": ["api.example.com"], "methods": ["GET"]})),
            None,
            &[],
        );
        let b = effective_to_bundle(&[e]);
        assert_eq!(b.allow_net_http, vec!["api.example.com".to_string()]);
        assert!(b.deny_net_http.is_empty());
    }

    #[test]
    fn effective_to_bundle_unions_fs_exec_and_process_spawn_into_exec() {
        let a = eff(cap(serde_json::json!({"kind": "fs.exec", "paths": ["/usr/bin/git"]})), None, &[]);
        let c = eff(cap(serde_json::json!({"kind": "process.spawn", "commands": ["ls"]})), None, &[]);
        let b = effective_to_bundle(&[a, c]);
        assert_eq!(b.allow_exec, vec!["/usr/bin/git".to_string(), "ls".to_string()]);
    }

    #[test]
    fn effective_to_bundle_maps_fs_write_and_agent_spawn() {
        let w = eff(cap(serde_json::json!({"kind": "fs.write", "paths": ["/out/**"], "max_bytes": 1024})), None, &["/out/locked/**"]);
        let s = eff(cap(serde_json::json!({"kind": "agent.spawn", "allowed_kinds": ["critic"]})), None, &[]);
        let b = effective_to_bundle(&[w, s]);
        assert_eq!(b.allow_fs_write, vec!["/out/**".to_string()]);
        assert_eq!(b.deny_fs_write, vec!["/out/locked/**".to_string()]);
        assert_eq!(b.allow_agent_spawn, vec!["critic".to_string()]);
    }

    #[test]
    fn effective_to_bundle_drops_unrepresentable_shapes() {
        let e = eff(cap(serde_json::json!({"kind": "skill.spawn", "allowed_skills": ["fact-checker"]})), None, &[]);
        let b = effective_to_bundle(&[e]);
        assert!(b.is_empty(), "skill.spawn must be dropped: {b:?}");
    }
}
