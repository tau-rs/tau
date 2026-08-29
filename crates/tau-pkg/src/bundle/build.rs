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
    BundlePackage, IrPayload, ProjectInfo,
};
use crate::project::project::{ProjectConfig, PromptEntry};

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
    /// Optional IR payload to embed in the bundle. When `Some`, the
    /// `canonical_ir_bytes` are hashed into the bundle's self-hash and
    /// the bundle schema_version is written as 2. When `None`, the
    /// bundle is schema_version 2 but with no IR payload field. The
    /// caller (tau-cli) is responsible for lowering the project IR via
    /// `tau_ir::lower::lower_project` and constructing the `IrPayload`
    /// before calling `build`.
    pub ir_payload: Option<IrPayload>,
    /// Governance record to stamp into the bundle (ADR-0057 / D2). When
    /// `Some`, the bundle's `schema_version` is written as `4` and the
    /// verdict is hashed into the self-hash. The caller (tau-cli) computes
    /// the verdict from the project's `[allow]` ceiling + build flags via
    /// the governance gate. `None` produces a legacy-shaped (v2/v3) bundle
    /// with no governance record — used by lower-level tests and by tooling
    /// that predates governance-by-default.
    pub governance: Option<crate::bundle::manifest::GovernanceRecord>,
    /// Content-addressed assets to embed in the bundle's `[[assets]]` store
    /// (D6-B). When non-empty the bundle's `schema_version` is written as `5`
    /// and each asset's bytes are hashed into the self-hash via the canonical
    /// TOML. The caller (tau-cli) supplies the blobs `tau_ir_lower` collected
    /// (the module's `PromptSource::Asset` references resolve into this set).
    pub assets: Vec<crate::bundle::manifest::BundleAsset>,
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
    // Step 1: Load tau.toml through `ProjectConfig::parse_str_at`, the
    // dirs-aware entry point `ProjectConfig::from_path` delegates to — this
    // is the same pipeline `tau run` / `tau check` use, so the bundle records
    // exactly what the project config layer would surface to the runtime,
    // *including* `[dirs]`-scanned agents and tools (ADR-0069). Reading the
    // raw bytes ourselves rather than calling `from_path` keeps them around
    // for `tau_toml_sha256` and `extract_project_version` below.
    let tau_toml_path = opts.project_root.join("tau.toml");
    let tau_toml_bytes = std::fs::read(&tau_toml_path)
        .map_err(|e| BuildError::ProjectConfig(format!("read {tau_toml_path:?}: {e}")))?;
    let tau_toml_str = std::str::from_utf8(&tau_toml_bytes)
        .map_err(|e| BuildError::ProjectConfig(format!("tau.toml is not utf-8: {e}")))?;
    let project_config = ProjectConfig::parse_str_at(tau_toml_str, &opts.project_root)
        .map_err(|e| BuildError::ProjectConfig(format!("load {tau_toml_path:?}: {e}")))?;

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
    //   the agent's effective grant is computed from its home-package
    //   manifest — see `agent_home_package_caps`.
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
        // home-package manifest grant set (see agent_home_package_caps) and
        // call compute_effective.
        let effective_capabilities = if entry.capability_overrides.is_empty() {
            BundleEffectiveCapabilities::default()
        } else {
            // Mirror the runtime (tau-runtime builder.rs): compute the
            // agent's effective grant from its HOME-package manifest +
            // the project overrides. Home package name = the `<name>`
            // half of `[agents.<id>].package`.
            let (home_pkg, _req) = crate::project::agent::parse_package_ref(&entry.package)
                .map_err(|_| BuildError::AgentHomePackageMissing {
                    id: id.clone(),
                    package: entry.package.clone(),
                })?;
            let package_caps = agent_home_package_caps(&home_pkg, &packages, &packages_root, id)?;
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

        // Derive the backend kind from the project [models] table via the
        // agent's model alias. Falls back to empty string when the alias is
        // absent; the real resolution happens at IR lowering.
        let backend_kind = project_config
            .models
            .get(&entry.model)
            .map(|m| m.backend.clone())
            .unwrap_or_default();
        agents.push(BundleAgent {
            id: agent_id,
            backend: BackendRef {
                kind: backend_kind,
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

    // Step 5.6: gather trigger bindings (slice 1). One entry per validated
    // [trigger.<name>]. BTreeMap iteration is sorted by name.
    let triggers: Vec<crate::bundle::manifest::BundleTrigger> = project_config
        .triggers
        .values()
        .map(|t| crate::bundle::manifest::BundleTrigger {
            name: t.name.clone(),
            kind: t.kind.clone(),
            agent: t.agent.clone(),
            schedule: t.schedule.clone(),
            timezone: if t.timezone.is_empty() {
                None
            } else {
                Some(t.timezone.clone())
            },
            retry: t
                .retry
                .as_ref()
                .map(|r| crate::bundle::manifest::BundleRetry {
                    max_attempts: r.max_attempts,
                    backoff_strategy: r.backoff_strategy.clone(),
                    backoff_base: r.backoff_base.clone(),
                    backoff_max: r.backoff_max.clone(),
                    dead_letter: r.dead_letter.clone(),
                }),
        })
        .collect();

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

    // Content-addressed asset store (D6-B), sorted by hash so container order
    // can never leak into the self-hash (same discipline as `agents`).
    let mut assets = opts.assets;
    assets.sort_by(|a, b| a.hash.cmp(&b.hash));

    // schema_version: an asset store forces v5 (highest); else a governance
    // record forces v4; else a trigger-bearing bundle is v3 and a plain
    // bundle is v2.
    let schema_version = if !assets.is_empty() {
        5
    } else if opts.governance.is_some() {
        4
    } else if triggers.is_empty() {
        2
    } else {
        3
    };

    let mut manifest = BundleManifest {
        schema_version,
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
        ir_payload: opts.ir_payload,
        triggers,
        governance: opts.governance,
        assets,
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

/// Load the agent's home-package manifest and return its declared
/// capability grants — the same source the runtime feeds to
/// `compute_effective` (see tau-runtime builder.rs). `home_package` is
/// the `<name>` half of `[agents.<id>].package`; its resolved version
/// comes from the gathered `packages` list. Fails loudly rather than
/// returning empty so the bundle never silently under-records grants.
fn agent_home_package_caps(
    home_package: &str,
    packages: &[BundlePackage],
    packages_root: &std::path::Path,
    agent_id: &str,
) -> Result<Vec<tau_domain::Capability>, BuildError> {
    let pkg = packages
        .iter()
        .find(|p| p.name == home_package)
        .ok_or_else(|| BuildError::AgentHomePackageMissing {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
        })?;
    let manifest_path = packages_root
        .join(&pkg.name)
        .join(pkg.version.to_string())
        .join("tau.toml");
    let manifest = crate::read_manifest(&manifest_path).map_err(|source| {
        BuildError::AgentHomePackageManifest {
            id: agent_id.to_owned(),
            package: home_package.to_owned(),
            source,
        }
    })?;
    Ok(manifest.capabilities().to_vec())
}

/// Flatten [`crate::capability_override::EffectiveCapability`] entries into
/// the per-shape allow/deny lists of [`BundleEffectiveCapabilities`].
///
/// For each entry: the allow list is the narrowed `allow_override` if present,
/// otherwise the source variant's own field. `deny` is always `e.deny`.
/// `Filesystem(Exec)` and `Process(Spawn)` both map to `allow_exec`/`deny_exec`.
/// Shapes with no bundle representation (task_list, plan, custom)
/// are silently dropped via the catch-all arm.
fn effective_to_bundle(
    eff: &[crate::capability_override::EffectiveCapability],
) -> BundleEffectiveCapabilities {
    use tau_domain::{
        AgentCapability, Capability, FsCapability, NetCapability, ProcessCapability,
        SkillCapability,
    };
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
                // An explicit allow-override always narrows to a concrete list.
                // Absent an override, `hosts = "any"` records the any-host
                // grant via the typed `any_net_http` flag; a list contributes
                // its exact hosts (D7-B).
                match &e.allow_override {
                    Some(ov) => out.allow_net_http.extend(ov.clone()),
                    None => {
                        if hosts.is_any() {
                            out.any_net_http = true;
                        } else {
                            out.allow_net_http.extend(hosts.exact_hosts());
                        }
                    }
                }
                out.deny_net_http.extend(e.deny.clone());
            }
            Capability::Agent(AgentCapability::Spawn { allowed_kinds, .. }) => {
                out.allow_agent_spawn.extend(
                    e.allow_override
                        .clone()
                        .unwrap_or_else(|| allowed_kinds.clone()),
                );
                out.deny_agent_spawn.extend(e.deny.clone());
            }
            Capability::Skill(SkillCapability::Spawn { allowed_skills, .. }) => {
                out.allow_skill_spawn.extend(
                    e.allow_override
                        .clone()
                        .unwrap_or_else(|| allowed_skills.clone()),
                );
                out.deny_skill_spawn.extend(e.deny.clone());
            }
            // task_list / plan / custom: no bundle field (still dropped).
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
/// - [`PromptEntry::File`] → the file's bytes with CRLF line endings
///   normalized to LF (see [`read_prompt_file`]), resolved relative to
///   `project_root` when the path is relative. *Not* the raw bytes: an
///   autocrlf checkout must hash the same as a LF checkout.
/// - [`PromptEntry::None`] → empty bytes (hashes to the SHA-256 of the
///   empty string), matching §C.2's "no prompt ⇒ hash empty" rule.
pub(crate) fn resolve_agent_prompt_bytes(
    prompt: &PromptEntry,
    project_root: &std::path::Path,
) -> Result<Vec<u8>, std::io::Error> {
    match prompt {
        PromptEntry::Inline(s) => Ok(s.clone().into_bytes()),
        PromptEntry::File(rel) => read_prompt_file(rel, project_root),
        PromptEntry::None => Ok(Vec::new()),
    }
}

/// Read an agent `system_file` prompt's bytes, resolving a relative path
/// against `project_root`.
///
/// The single source of truth for prompt-file *path resolution*, shared by
/// the bundle builder ([`resolve_agent_prompt_bytes`]) and the lowering
/// `prompt_file` closure (D6-B) so the IR asset hash and the bundle's
/// `system_prompt_sha256` are computed over identical bytes.
///
/// Containment guard: `rel` must be relative and must resolve (after
/// canonicalization) to a path inside `project_root`. Absolute paths and
/// `../` escapes are rejected with `ErrorKind::InvalidInput` — a prompt path
/// is authored data from `tau.toml`, not something that should be able to
/// read arbitrary files off the host.
///
/// Returns CRLF-normalized bytes, not raw bytes — see
/// [`crate::crlf::normalize_crlf_bytes`], which is idempotent precisely
/// because `tau_ir_lower` normalizes this function's output a second time.
pub fn read_prompt_file(
    rel: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<Vec<u8>, std::io::Error> {
    let deny = |msg: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.to_string());
    if rel.is_absolute() {
        return Err(deny(
            "prompt file path must be relative to the project root",
        ));
    }
    let abs = project_root.join(rel);
    let canon = abs.canonicalize()?;
    let root = project_root.canonicalize()?;
    if !canon.starts_with(&root) {
        return Err(deny("prompt file path escapes the project root"));
    }
    // Applied at the single read point `resolve_agent_prompt_bytes` and the
    // lowering `prompt_file` closure both go through, so an autocrlf Windows
    // checkout can't desync the bundle's `system_prompt_sha256` from the IR
    // asset hash: both are computed over these same normalized bytes.
    std::fs::read(&canon).map(crate::crlf::normalize_crlf_bytes)
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
            ir_payload: None,
            governance: None,
            assets: Vec::new(),
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

[models]
default = { backend = "p", model = "model-v1" }

[agents.zeta]
display_name = "Zeta"
package      = "p@^0.1"
model        = "default"

[agents.zeta.prompt]
system = "you are zeta"

[agents.alpha]
display_name = "Alpha"
package      = "p@^0.1"
model        = "default"

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

[models]
default = { backend = "p", model = "model-v1" }

[agents.r]
display_name = "R"
package      = "p@^0.1"
model        = "default"

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

[models]
default = { backend = "pkg-home", model = "model-v1" }

[agents.alpha]
display_name = "Alpha"
package = "pkg-home@^0.1"
model = "default"

[agents.alpha.prompt]
system = "you are alpha"

[[agents.alpha.requires.tools]]
name = "pkg-a"
source = "https://example.com/pkg-a.git"

[agents.beta]
display_name = "Beta"
package = "pkg-home@^0.1"
model = "default"

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
            ir_payload: None,
            governance: None,
            assets: Vec::new(),
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

[models]
default = { backend = "p", model = "model-v1" }

[agents.alpha]
display_name = "Alpha"
package      = "p@^0.1"
model        = "default"

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
            ir_payload: None,
            governance: None,
            assets: Vec::new(),
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

[models]
default = { backend = "p", model = "model-v1" }

[agents.r]
display_name = "R"
package      = "p@^0.1"
model        = "default"

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

    #[test]
    fn read_prompt_file_rejects_absolute_and_escape() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("p.md"), "ok").unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.md"), "no").unwrap();

        assert_eq!(
            read_prompt_file(std::path::Path::new("p.md"), t.path()).unwrap(),
            b"ok"
        );
        // absolute
        assert!(read_prompt_file(&outside.path().join("secret.md"), t.path()).is_err());
        // ../ escape
        let escape = std::path::Path::new("..")
            .join(outside.path().file_name().unwrap())
            .join("secret.md");
        assert!(read_prompt_file(&escape, t.path()).is_err());
    }

    #[test]
    fn read_prompt_file_normalizes_crlf() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("p.md"), b"line one\r\nline two\r\n").unwrap();
        let bytes = read_prompt_file(std::path::Path::new("p.md"), t.path()).unwrap();
        assert_eq!(bytes, b"line one\nline two\n");
    }

    #[test]
    fn read_prompt_file_crlf_and_lf_hash_equal() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("crlf.md"), b"line one\r\nline two\r\n").unwrap();
        std::fs::write(t.path().join("lf.md"), b"line one\nline two\n").unwrap();

        let crlf_bytes = read_prompt_file(std::path::Path::new("crlf.md"), t.path()).unwrap();
        let lf_bytes = read_prompt_file(std::path::Path::new("lf.md"), t.path()).unwrap();

        assert_eq!(
            sha256_hex(&crlf_bytes),
            sha256_hex(&lf_bytes),
            "system_prompt_sha256 must be CRLF-invariant, matching the IR asset hash"
        );
    }

    /// `tau_ir_lower` normalizes this function's output a *second* time (the
    /// CLI's `prompt_file` closure is `read_prompt_file`). So a single
    /// `read_prompt_file` pass must already be at the fixed point, or the
    /// bundle's `system_prompt_sha256` and the IR asset hash describe
    /// different bytes for the same file. `\r\r\n` is the input that broke
    /// the old non-idempotent rewrite.
    #[test]
    fn read_prompt_file_output_is_already_normalized() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("p.md"), b"a\r\r\nb\r\n").unwrap();
        let once = read_prompt_file(std::path::Path::new("p.md"), t.path()).unwrap();
        assert_eq!(once, b"a\nb\n");
        assert_eq!(
            crate::crlf::normalize_crlf_bytes(once.clone()),
            once,
            "a second normalization pass must not change the bytes",
        );
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
            cap(
                serde_json::json!({"kind": "net.http", "hosts": ["api.example.com"], "methods": ["GET"]}),
            ),
            None,
            &[],
        );
        let b = effective_to_bundle(&[e]);
        assert_eq!(b.allow_net_http, vec!["api.example.com".to_string()]);
        assert!(b.deny_net_http.is_empty());
    }

    #[test]
    fn effective_to_bundle_unions_fs_exec_and_process_spawn_into_exec() {
        let a = eff(
            cap(serde_json::json!({"kind": "fs.exec", "paths": ["/usr/bin/git"]})),
            None,
            &[],
        );
        let c = eff(
            cap(serde_json::json!({"kind": "process.spawn", "commands": ["ls"]})),
            None,
            &[],
        );
        let b = effective_to_bundle(&[a, c]);
        assert_eq!(
            b.allow_exec,
            vec!["/usr/bin/git".to_string(), "ls".to_string()]
        );
    }

    #[test]
    fn effective_to_bundle_maps_fs_write_and_agent_spawn() {
        let w = eff(
            cap(serde_json::json!({"kind": "fs.write", "paths": ["/out/**"], "max_bytes": 1024})),
            None,
            &["/out/locked/**"],
        );
        let s = eff(
            cap(serde_json::json!({"kind": "agent.spawn", "allowed_kinds": ["critic"]})),
            None,
            &[],
        );
        let b = effective_to_bundle(&[w, s]);
        assert_eq!(b.allow_fs_write, vec!["/out/**".to_string()]);
        assert_eq!(b.deny_fs_write, vec!["/out/locked/**".to_string()]);
        assert_eq!(b.allow_agent_spawn, vec!["critic".to_string()]);
    }

    #[test]
    fn effective_to_bundle_drops_unrepresentable_shapes() {
        // task_list has no BundleEffectiveCapabilities field and must be dropped.
        let e = eff(
            cap(serde_json::json!({"kind": "task_list", "mode": "read"})),
            None,
            &[],
        );
        let b = effective_to_bundle(&[e]);
        assert!(b.is_empty(), "task_list must be dropped: {b:?}");
    }

    /// Project with override-agent `r` whose home package `homepkg` is
    /// installed with a manifest granting fs.read + net.http + fs.exec +
    /// process.spawn + skill.spawn. The agent narrows fs.read.
    fn override_agent_project(tmp: &std::path::Path) {
        std::fs::write(
            tmp.join("tau.toml"),
            r#"
[project]
name = "capproj"
version = "0.1.0"

[models]
default = { backend = "homepkg", model = "model-v1" }

[agents.r]
display_name = "R"
package = "homepkg@^0.1"
model = "default"

[agents.r.prompt]
system = "you are r"

[[agents.r.capabilities]]
kind = "fs.read"
allow_paths = ["/data/**"]
deny_paths = ["/data/secret/**"]
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.join("tau.lock"),
            r#"schema_version = 6
generated_by_tau_version = "0.1.0"
generated_at = "2024-01-01T00:00:00Z"

[[package]]
name = "homepkg"
active_version = "0.1.0"
source = "https://example.com/homepkg.git"

[[package.versions]]
version = "0.1.0"
resolved_commit = "0000000000000000000000000000000000000001"
installed_at = "2024-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        let pkg_dir = tmp.join(".tau/packages/homepkg/0.1.0");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("tau.toml"),
            r#"name = "homepkg"
version = "0.1.0"
description = "home package"
authors = ["a <a@example.com>"]
source = "https://example.com/homepkg.git"
kind = "tool"
dependencies = []

[[capabilities]]
kind = "fs.read"
paths = ["/data/**", "/tmp/**"]

[[capabilities]]
kind = "net.http"
hosts = ["api.example.com"]
methods = ["GET"]

[[capabilities]]
kind = "fs.exec"
paths = ["/usr/bin/git"]

[[capabilities]]
kind = "process.spawn"
commands = ["ls"]

[[capabilities]]
kind = "skill.spawn"
allowed_skills = ["critic"]
"#,
        )
        .unwrap();
    }

    fn read_agent_caps(
        tmp: &std::path::Path,
    ) -> crate::bundle::manifest::BundleEffectiveCapabilities {
        let artifact = build(opts(tmp)).expect("build succeeds");
        let s = std::fs::read_to_string(&artifact.path).unwrap();
        let m = crate::bundle::manifest::BundleManifest::parse_str(&s).unwrap();
        assert_eq!(m.agents.len(), 1);
        m.agents[0].effective_capabilities.clone()
    }

    #[test]
    fn build_records_narrowed_fs_read_for_override_agent() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
        assert_eq!(caps.deny_fs_read, vec!["/data/secret/**".to_string()]);
    }

    #[test]
    fn build_records_unoverridden_source_caps() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_net_http, vec!["api.example.com".to_string()]);
        assert!(caps.allow_exec.contains(&"/usr/bin/git".to_string()));
        assert!(caps.allow_exec.contains(&"ls".to_string()));
    }

    #[test]
    fn build_records_skill_spawn_for_override_agent() {
        // skill.spawn is now a represented shape: the home package's
        // skill.spawn grant is recorded in the agent's effective caps.
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_skill_spawn, vec!["critic".to_string()]);
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
    }

    #[test]
    fn build_effective_caps_is_reproducible() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let a = build(opts(tmp.path())).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let b = build(opts(tmp.path())).unwrap();
        assert_eq!(
            a.sha256, b.sha256,
            "effective-caps build must be reproducible"
        );
    }

    #[test]
    fn build_errors_when_override_agent_home_package_missing() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "capproj"
version = "0.1.0"

[models]
default = { backend = "ghost", model = "model-v1" }

[agents.r]
display_name = "R"
package = "ghost@^0.1"
model = "default"

[agents.r.prompt]
system = "you are r"

[[agents.r.capabilities]]
kind = "fs.read"
allow_paths = ["/data/**"]
"#,
        )
        .unwrap();
        match build(opts(tmp.path())).unwrap_err() {
            BuildError::AgentHomePackageMissing { id, package } => {
                assert_eq!(id, "r");
                assert_eq!(package, "ghost");
            }
            other => panic!("expected AgentHomePackageMissing, got {other:?}"),
        }
    }

    #[test]
    fn build_errors_when_home_package_manifest_unreadable() {
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        std::fs::remove_file(tmp.path().join(".tau/packages/homepkg/0.1.0/tau.toml")).unwrap();
        match build(opts(tmp.path())).unwrap_err() {
            BuildError::AgentHomePackageManifest { id, package, .. } => {
                assert_eq!(id, "r");
                assert_eq!(package, "homepkg");
            }
            other => panic!("expected AgentHomePackageManifest, got {other:?}"),
        }
    }

    #[test]
    fn build_tolerates_custom_capability_kind_in_home_package() {
        // An OLD tau building against a package that grants a future
        // (Custom) capability kind must not crash; the Custom shape is
        // simply not represented in effective_capabilities.
        let tmp = tempdir().unwrap();
        override_agent_project(tmp.path());
        let pkg_manifest = tmp.path().join(".tau/packages/homepkg/0.1.0/tau.toml");
        let mut body = std::fs::read_to_string(&pkg_manifest).unwrap();
        body.push_str("\n[[capabilities]]\nkind = \"custom.mcp.tool.use\"\nendpoint = \"x\"\n");
        std::fs::write(&pkg_manifest, body).unwrap();
        let caps = read_agent_caps(tmp.path());
        assert_eq!(caps.allow_fs_read, vec!["/data/**".to_string()]);
    }

    #[test]
    fn build_emits_v3_bundle_with_trigger_section() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("tau.toml"),
            r#"
[project]
name = "trig"
version = "0.1.0"

[models]
default = { backend = "p", model = "model-v1" }

[agents.summarizer]
display_name = "S"
package = "p@^0.1"
model = "default"

[agents.summarizer.prompt]
system = "hi"

[trigger.nightly]
kind = "cron"
agent = "summarizer"
schedule = "0 3 * * *"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();

        let artifact = build(opts(tmp.path())).expect("build");
        let m = crate::bundle::manifest::BundleManifest::parse_str(
            &std::fs::read_to_string(&artifact.path).unwrap(),
        )
        .unwrap();
        assert_eq!(m.schema_version, 3);
        assert_eq!(m.triggers.len(), 1);
        assert_eq!(m.triggers[0].name, "nightly");
        crate::bundle::hash::verify_self_hash(&m).expect("self-hash verifies");
    }

    #[test]
    fn build_with_governance_emits_v4_and_records_verdict() {
        use crate::bundle::manifest::{GovernanceRecord, GovernanceVerdict};
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());
        let o = BuildOptions {
            governance: Some(GovernanceRecord {
                verdict: GovernanceVerdict::Governed,
            }),
            ..opts(tmp.path())
        };
        let artifact = build(o).expect("build");
        let m = crate::bundle::manifest::BundleManifest::parse_str(
            &std::fs::read_to_string(&artifact.path).unwrap(),
        )
        .unwrap();
        assert_eq!(m.schema_version, 4);
        assert_eq!(
            m.governance,
            Some(GovernanceRecord {
                verdict: GovernanceVerdict::Governed
            })
        );
        crate::bundle::hash::verify_self_hash(&m).expect("self-hash verifies");
    }

    #[test]
    fn build_trigger_less_stays_v2() {
        let tmp = tempdir().unwrap();
        happy_path_project(tmp.path());
        let artifact = build(opts(tmp.path())).expect("build");
        let m = crate::bundle::manifest::BundleManifest::parse_str(
            &std::fs::read_to_string(&artifact.path).unwrap(),
        )
        .unwrap();
        assert_eq!(m.schema_version, 2);
        assert!(m.triggers.is_empty());
    }
}
