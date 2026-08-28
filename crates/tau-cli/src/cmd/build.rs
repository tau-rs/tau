//! `tau build` — see spec `2026-05-27-tau-build-design.md`.
//!
//! Thin CLI shim over [`tau_pkg::bundle::build`](tau_pkg::bundle::build()). Resolves the project
//! root from the current directory, calls the bundle builder with the
//! host target + default output path, prints progress to stderr and
//! the bundle path to stdout, then exits with the appropriate code
//! per spec §6 (0 success, 2 config/parse, 3 install-state, 70 internal).

use std::collections::BTreeMap;

use anyhow::Result;

use tau_pkg::bundle::{
    build, BuildError, BuildOptions, BundleArtifact, GovernanceRecord, IrPayload,
};
use tau_pkg::lockfile::{LockedMcpEntry, LockedMcpExpandedTool};
use tau_ports::target::TargetTriple;

use crate::cli::BuildArgs;
use crate::output::Output;

/// CLI entry point for `tau build`. The function is async to match the
/// dispatcher's signature; MCP contract resolution requires async I/O.
pub async fn run(args: &BuildArgs, output: &mut Output) -> Result<()> {
    // Resolve the project path: prefer the CLI positional arg, fall back to cwd.
    let project_path = match &args.project {
        Some(p) => p.clone(),
        None => match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                let _ = output.error(format!("cannot determine current directory: {e}"));
                std::process::exit(70);
            }
        },
    };

    // Resolve the project root and optionally extract a ProjectConfig from a
    // `.ts` source (β.8). For TOML-based projects the root IS the project
    // directory; for `.ts` projects it is the parent directory.
    let (project_root, ts_project) = {
        let ext = project_path.extension().and_then(|s| s.to_str());
        if project_path.is_file() && ext == Some("ts") {
            match crate::cmd::project_load::load_project(&project_path) {
                Ok(loaded) => (loaded.project_root, Some(loaded.project)),
                Err(e) => {
                    let _ = output.error(format!("{e}"));
                    std::process::exit(2);
                }
            }
        } else {
            let root = if project_path.is_dir() {
                project_path.clone()
            } else {
                project_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| project_path.clone())
            };
            (root, None)
        }
    };

    let build_target = match resolve_target(args) {
        Ok(t) => t,
        Err(msg) => {
            let _ = output.error(msg);
            std::process::exit(2);
        }
    };
    // Artifact-kind targets (EPIC 5.1) branch to their own pipelines; the
    // bundle path below runs only for a hardware triple / default host.
    let target = match build_target {
        BuildTarget::Bundle(t) => t,
        BuildTarget::WasmGuest => return dispatch_wasm_guest(args, output).await,
        BuildTarget::RustLib => return dispatch_rust_lib(args, output).await,
    };

    // Governed-by-default gate (ADR-0057 / D2). Evaluate the `[allow]`
    // constitution BEFORE any network I/O (MCP resolution): an absent ceiling
    // is a hard error (GOV000) unless `--allow-ungoverned`, and a declared-
    // but-violated ceiling refuses the build. The verdict is stamped into the
    // bundle so `tau run --bundle` can enforce governed-by-default on its end.
    let governance = evaluate_build_governance(&project_path, &target, args, output).await;

    // Map the repeatable `--agent` flag to the builder's filter. Empty
    // → None (build all). Parse each id to AgentId; a malformed id is a
    // config-level input error (exit 2).
    let agent_filter = if args.agents.is_empty() {
        None
    } else {
        let mut parsed = Vec::with_capacity(args.agents.len());
        for raw in &args.agents {
            match raw.parse::<tau_domain::AgentId>() {
                Ok(id) => parsed.push(id),
                Err(e) => {
                    let _ = output.error(format!("invalid agent id '{raw}': {e}"));
                    std::process::exit(2);
                }
            }
        }
        Some(parsed)
    };

    // Resolve MCP contracts (pinned or live) before lowering the IR.
    // This is async; the result is passed into lower_ir as a sync cache.
    let (mcp_entries_meta, mcp_cache_ir) =
        match resolve_mcp_cache(&project_root, args.offline).await {
            Ok(v) => v,
            Err(e) => {
                let _ = output.error(format!("{e}"));
                std::process::exit(2);
            }
        };

    // Lower the project IR. Load the project config (same pipeline the
    // bundle builder uses), then call lower_project with a deterministic
    // SHA-256-of-name cache for native tools (see `lower_ir`'s doc-comment
    // for the forward-stability semantic). On IrError, render a human-
    // readable diagnostic and exit 2.
    //
    // For `.ts` projects `ts_project` is already parsed; pass it through so
    // `lower_ir` does not try to read a non-existent `tau.toml`.
    let LowerIrResult {
        payload: ir_payload,
        triggers: trigger_bindings,
        lower_error,
        assets: ir_assets,
    } = lower_ir(&project_root, &target, &mcp_cache_ir, ts_project.as_ref());

    // A typecheck/lowering error (e.g. an invalid context pipeline where
    // `fit_budget` is not the last step) is a hard build failure: surface
    // the diagnostic and exit 2 rather than silently dropping the IR payload.
    if let Some(e) = lower_error {
        let _ = output.error(format!("{e}"));
        std::process::exit(2);
    }

    let opts = BuildOptions {
        project_root: project_root.clone(),
        target,
        output_path: args.output.clone(),
        agent_filter,
        ir_payload,
        governance,
        assets: ir_assets_to_bundle(ir_assets),
    };

    let _ = output.status("Building bundle…");

    match build(opts) {
        Ok(artifact) => {
            // After a successful build, persist MCP entries to tau.lock.
            if !mcp_entries_meta.is_empty() {
                let lockfile_path = project_root.join("tau.lock");
                match tau_pkg::lockfile::LockFile::load(&lockfile_path) {
                    Ok(mut lf) => {
                        lf.mcp_entries = mcp_entries_meta;
                        if let Err(e) = lf.save(&lockfile_path) {
                            tracing::warn!("failed to write mcp_entries to tau.lock: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!("failed to reload tau.lock for mcp_entries: {e}");
                    }
                }
            }
            emit_artifact(&artifact, output);
            if let Some(adapter) = &args.emit_trigger {
                emit_trigger_descriptors(adapter, &trigger_bindings, &artifact, output);
            }
            Ok(())
        }
        Err(e) => {
            let _ = output.error(format!("{e}"));
            std::process::exit(exit_code_for(&e) as i32);
        }
    }
}

/// Run the governed-by-default gate (ADR-0057 / D2) for `tau build`.
///
/// Loads the project through the same `CheckCtx` the `tau check governance`
/// category uses, evaluates the `[allow]` constitution against the build
/// flags, and either returns the [`GovernanceRecord`] to stamp into the
/// bundle or terminates the process:
///
/// - `NoConstitution` → prints `GOV000` and exits 2.
/// - `Violations`     → prints the refused-build diagnostic and exits 2.
/// - malformed project → returns `None`; the bundle builder surfaces the
///   precise parse/validation error (also exit 2) rather than a vague GOV000.
async fn evaluate_build_governance(
    project_path: &std::path::Path,
    target: &TargetTriple,
    args: &BuildArgs,
    output: &mut Output,
) -> Option<GovernanceRecord> {
    use crate::cmd::check::{
        evaluate_governance, render_no_constitution, render_violations, CheckCtx, GovernanceFlags,
        GovernanceOutcome,
    };

    let flags = GovernanceFlags {
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    let ctx = match CheckCtx::load(project_path.to_path_buf(), false, Some(*target)).await {
        Ok(c) => c,
        Err(e) => {
            let _ = output.error(format!("cannot evaluate governance: {e}"));
            std::process::exit(2);
        }
    };
    let Some(project) = &ctx.project else {
        // Unparseable project — defer to the bundle builder's precise error.
        return None;
    };
    match evaluate_governance(project, &ctx, flags) {
        GovernanceOutcome::Proceed(verdict) => Some(GovernanceRecord { verdict }),
        GovernanceOutcome::NoConstitution => {
            let _ = output.diagnostic(render_no_constitution());
            std::process::exit(2);
        }
        GovernanceOutcome::Violations(findings) => {
            let _ = output.diagnostic(render_violations(&findings));
            std::process::exit(2);
        }
    }
}

/// Discover all MCP entries from tau.toml and resolve their contracts.
///
/// Returns:
/// - `Vec<McpEntryMeta>` — per-entry metadata for writing to `tau.lock`.
/// - `BTreeMap<String, tau_ir_lower::ResolvedMcpContract>` — URL-keyed
///   cache for `Caches::mcp_contract`.
///
/// On `--offline`, reads `.tau/mcp/<entry>.contract.json` (error if missing).
/// On the live path, performs MCP handshakes and writes pinned files.
async fn resolve_mcp_cache(
    project_root: &std::path::Path,
    offline: bool,
) -> anyhow::Result<(
    Vec<LockedMcpEntry>,
    BTreeMap<String, tau_ir_lower::ResolvedMcpContract>,
)> {
    use tau_pkg::project::project::ToolBody;

    // Load tau.toml to find MCP entries. Goes through the dirs-aware
    // `from_path` (ADR-0068) so a `tools/**/*.toml` MCP definition gets its
    // contract resolved and pinned exactly like an inline `[tools.X]` one.
    let tau_toml_path = project_root.join("tau.toml");
    let config = match tau_pkg::project::ProjectConfig::from_path(&tau_toml_path) {
        Ok(c) => c,
        Err(_) => {
            // Missing / unparseable / invalid tau.toml → no MCP entries.
            // lower_ir warns separately.
            return Ok((Vec::new(), BTreeMap::new()));
        }
    };

    // Collect all MCP entries.
    let mcp_entries: Vec<(String, String)> = config
        .tools
        .iter()
        .filter_map(|(name, t)| match &t.body {
            ToolBody::Mcp(url) => Some((name.clone(), url.clone())),
            _ => None,
        })
        .collect();

    if mcp_entries.is_empty() {
        return Ok((Vec::new(), BTreeMap::new()));
    }

    let pin_base = project_root.join(".tau").join("mcp");
    let rel_pin_base = std::path::Path::new(".tau/mcp");

    if offline {
        // Pinned path: read `.tau/mcp/<entry>.contract.json`.
        use tau_mcp::contract::McpContractResolver as _;
        let resolver = tau_mcp::contract::resolver::PinnedResolver::new(&pin_base);
        let mut ir_cache: BTreeMap<String, tau_ir_lower::ResolvedMcpContract> = BTreeMap::new();
        let mut locked_entries: Vec<LockedMcpEntry> = Vec::new();
        for (entry, url) in &mcp_entries {
            let resolved = resolver
                .resolve(entry, url)
                .map_err(|e| anyhow::anyhow!("MCP pin resolve failed for {entry:?}: {e}"))?;
            locked_entries.push(mcp_entry_to_locked(
                entry,
                url,
                &resolved,
                Some(contract_pin_path(rel_pin_base, entry).display().to_string()),
            ));
            ir_cache.insert(url.clone(), to_ir_shape(resolved));
        }
        Ok((locked_entries, ir_cache))
    } else {
        // Live path: perform MCP handshakes.
        let inputs: Vec<tau_mcp_tokio::resolver::McpEntryInput> = mcp_entries
            .iter()
            .map(|(entry, url)| tau_mcp_tokio::resolver::McpEntryInput {
                entry: entry.clone(),
                url: url.clone(),
                plan: tau_ports::CapabilityPlan::new(Vec::new(), None, None),
            })
            .collect();
        let live = tau_mcp_tokio::resolver::resolve_all(inputs)
            .await
            .map_err(|e| anyhow::anyhow!("MCP live resolve failed: {e}"))?;

        // Write pinned files for next-time --offline. Path-named entries
        // (`github/search`) nest, so the parent must be created per-entry
        // rather than just the flat `.tau/mcp/` base.
        for (entry, url) in &mcp_entries {
            if let Some(lr) = live.get(url) {
                let path = contract_pin_path(&pin_base, entry);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow::anyhow!("failed to create {}: {e}", parent.display())
                    })?;
                }
                let bytes = serde_json::to_vec_pretty(&lr.pinned)
                    .map_err(|e| anyhow::anyhow!("serialize pinned contract for {entry:?}: {e}"))?;
                std::fs::write(&path, bytes)
                    .map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
            }
        }

        let mut ir_cache: BTreeMap<String, tau_ir_lower::ResolvedMcpContract> = BTreeMap::new();
        let mut locked_entries: Vec<LockedMcpEntry> = Vec::new();
        for (entry, url) in &mcp_entries {
            if let Some(lr) = live.get(url) {
                locked_entries.push(mcp_entry_to_locked(
                    entry,
                    url,
                    &lr.resolved,
                    Some(contract_pin_path(rel_pin_base, entry).display().to_string()),
                ));
                ir_cache.insert(url.clone(), to_ir_shape(lr.resolved.clone()));
            }
        }
        Ok((locked_entries, ir_cache))
    }
}

/// Pin path for an MCP tool entry. Path-named tools (`github/search`) nest —
/// safe against a sibling `github.contract.json` because the file name always
/// carries the `.contract.json` suffix.
fn contract_pin_path(pin_base: &std::path::Path, entry: &str) -> std::path::PathBuf {
    pin_base.join(format!("{entry}.contract.json"))
}

/// Convert a `tau_mcp` resolver output to tau-ir's structurally-identical type.
fn to_ir_shape(
    r: tau_mcp::contract::resolver::ResolvedMcpContract,
) -> tau_ir_lower::ResolvedMcpContract {
    use tau_ir_lower::{ResolvedMcpContract as IrR, ResolvedServerTool as IrS};
    IrR {
        hash: r.hash,
        expanded_tools: r
            .expanded_tools
            .into_iter()
            .map(|t| IrS {
                name: t.name,
                // v0 lossy: caps is `CapabilityRequirements` in tau-ir (structured),
                // `Vec<String>` in tau-mcp (wire kind names). Use empty default;
                // the author's envelope is the source of truth at build time,
                // and runtime drift-check uses the lockfile.
                caps: tau_ir::capability::CapabilityRequirements::default(),
                input_schema: t.input_schema,
            })
            .collect(),
        requires_sampling: r.requires_sampling,
    }
}

/// Build a `LockedMcpEntry` from a resolved contract + metadata.
fn mcp_entry_to_locked(
    entry: &str,
    url: &str,
    resolved: &tau_mcp::contract::resolver::ResolvedMcpContract,
    pinned_contract: Option<String>,
) -> LockedMcpEntry {
    let expanded_tools = resolved
        .expanded_tools
        .iter()
        .map(|st| {
            LockedMcpExpandedTool::new(
                st.name.clone(),
                st.caps.clone(),
                schema_hash_json(&st.input_schema),
            )
        })
        .collect();
    LockedMcpEntry::new(
        entry.to_owned(),
        url.to_owned(),
        hex_lower(&resolved.hash),
        pinned_contract,
        expanded_tools,
    )
}

/// SHA-256 of a JSON value's canonical bytes (compact serialization).
fn schema_hash_json(v: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&bytes);
    hex_lower(&h.finalize())
}

/// Result of [`lower_ir`]: the optional IR payload to embed in the bundle,
/// plus the lowered trigger bindings (consumed by `--emit-trigger`).
pub(crate) struct LowerIrResult {
    /// `Some` when lowering succeeded; `None` when it failed (the bundle is
    /// still built, just without an IR payload).
    pub payload: Option<IrPayload>,
    /// Trigger bindings lowered from the project config (empty when lowering
    /// failed or the project declares no triggers).
    pub triggers: Vec<tau_ir::trigger::TriggerBinding>,
    /// `Some` when `lower_project` returned a typecheck/lowering error. The
    /// payload is `None` in that case. Callers that treat lowering as a
    /// best-effort enrichment (verify, run, dev) ignore this; `tau build`
    /// inspects it and rejects the build (exit 2) so a typecheck failure —
    /// e.g. an invalid context pipeline — is surfaced at build time rather
    /// than silently dropped (see ADR: build-time enforcement discipline).
    pub lower_error: Option<tau_ir_lower::LowerError>,
    /// Content-addressed assets (currently `system_file` prompts) the module
    /// references, keyed by hash (`"sha256:" + 64 hex`). Empty when lowering
    /// failed or the project uses only inline prompts. `tau build` persists
    /// these into the bundle's asset store; verify/dev use them to resolve
    /// prompt references at run time (D6-B).
    pub assets: BTreeMap<String, tau_ir::asset::AssetBlob>,
}

/// Attempt to lower the project IR, returning `Some(IrPayload)` on
/// success or `None` if lowering fails (non-fatal — the bundle is still
/// built, but without an IR payload; a warning is logged).
///
/// Native-tool content hashes are derived from `sha2::Sha256(symbolic_name)`.
/// This is a deterministic, non-zero stand-in until an actual native-tool
/// registry lands in `tau-pkg`: when that registry exists, replace the
/// `native_tool` closure with the registry's source-content hash. Bundles
/// produced before the switch will rebuild on the next `tau build` because
/// their `canonical_ir_hash` will change — that's the honest forward-stability
/// semantic (D-6): a change in tool identity is a change in workflow identity.
///
/// `mcp_cache` is keyed by MCP URL and populated by `resolve_mcp_cache`.
///
/// `preloaded_config` — when `Some`, the config is used directly instead of
/// reading `tau.toml` from `project_root`. Used by the `.ts` source path
/// (β.8) where there is no `tau.toml`.
pub(crate) fn lower_ir(
    project_root: &std::path::Path,
    target: &TargetTriple,
    mcp_cache: &BTreeMap<String, tau_ir_lower::ResolvedMcpContract>,
    preloaded_config: Option<&tau_pkg::project::ProjectConfig>,
) -> LowerIrResult {
    let config_owned;
    let config = if let Some(c) = preloaded_config {
        c
    } else {
        // `from_path` → `parse_str_at`, the dirs-aware load (ADR-0068). It
        // MUST stay this way: `run::verify_bundle_against_source` and
        // `verify::run_reproducibility_check` re-lower through this very
        // function to recompute the source IR hash, so if this load and
        // `tau build`'s bundle load ever disagree about the agent set, every
        // `[dirs]` project fails with `IrSourceDivergence`.
        let tau_toml_path = project_root.join("tau.toml");
        config_owned = match tau_pkg::project::ProjectConfig::from_path(&tau_toml_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("IR lowering: failed to load tau.toml: {e}");
                return LowerIrResult {
                    payload: None,
                    triggers: Vec::new(),
                    lower_error: None,
                    assets: BTreeMap::new(),
                };
            }
        };
        &config_owned
    };

    // Deterministic stand-in cache (β.2.6.1): hash the symbolic name with
    // SHA-256. Two distinct names produce two distinct hashes (so future
    // drift detection actually works), the value is non-zero (typecheck's
    // sentinel check passes), and it's stable across builds (so the IR
    // module hash stays reproducible). When a real native-tool registry
    // lands in tau-pkg, replace this closure with the registry's
    // source-content hash — see this fn's doc-comment.
    let caches = tau_ir_lower::Caches {
        native_tool: &|name: &str| Some(sha256_name(name)),
        mcp_contract: &|url| mcp_cache.get(url).cloned(),
        skill: &|_name| None,
        // D6-B: read `system_file` prompts at build time (missing/unreadable
        // => LowerError::PromptFileUnreadable => build fails). Routed through
        // tau-pkg's `read_prompt_file` so the IR asset hash is computed over
        // the same bytes as the bundle's `system_prompt_sha256`.
        prompt_file: &|p: &std::path::Path| {
            tau_pkg::bundle::read_prompt_file(p, project_root)
                .map_err(|e| tau_ir_lower::PromptFileError(e.to_string()))
        },
    };

    match tau_ir_lower::lower_project(config, target, &caches) {
        Ok(out) => {
            let bytes = tau_ir::to_canonical_bytes(&out.module);
            let hash_bytes = tau_ir::compute_hash(&out.module);
            let payload = Some(IrPayload {
                ir_format: out.module.ir_format.0.clone(),
                canonical_ir_hash: hex_lower(&hash_bytes),
                canonical_ir_bytes_hex: hex_lower(&bytes),
            });
            LowerIrResult {
                payload,
                triggers: out.module.triggers,
                lower_error: None,
                assets: out.assets,
            }
        }
        Err(e) => {
            tracing::warn!("IR lowering failed: {e}");
            LowerIrResult {
                payload: None,
                triggers: Vec::new(),
                lower_error: Some(e),
                assets: BTreeMap::new(),
            }
        }
    }
}

/// Convert the asset blobs `tau_ir_lower` collected into the bundle's
/// `[[assets]]` shape (bytes hex-encoded), sorted by hash for determinism.
/// Shared by `tau build` and `tau verify --bundle` so both derive an
/// identical asset store from the same source (D6-B).
pub(crate) fn ir_assets_to_bundle(
    ir_assets: BTreeMap<String, tau_ir::asset::AssetBlob>,
) -> Vec<tau_pkg::bundle::manifest::BundleAsset> {
    ir_assets
        .into_iter()
        .map(|(hash, blob)| tau_pkg::bundle::manifest::BundleAsset {
            hash,
            kind: blob.kind.as_str().to_string(),
            bytes_hex: hex_lower(&blob.bytes),
        })
        .collect()
}

/// `Caches::native_tool`-shaped stand-in: `Some(SHA-256(name))`.
///
/// Shared by [`lower_ir`] (bundle build) and `cmd::run`'s pipeline
/// lowering so both compute the SAME native-tool content hash — a drift
/// between the two would make a pipeline's runtime IR diverge from its
/// built bundle IR. Always `Some` (never the unknown-tool sentinel).
pub(crate) fn native_tool_hash(name: &str) -> Option<[u8; 32]> {
    Some(sha256_name(name))
}

/// Deterministic content-hash stand-in for a native tool's symbolic name.
///
/// Returns `SHA-256(name.as_bytes())`. Used by [`lower_ir`]'s `Caches::native_tool`
/// closure until a real native-tool registry lands in `tau-pkg`. Distinct
/// names always produce distinct hashes, and the value is non-zero so
/// `tau_ir_lower::typecheck` won't reject it as the unknown-tool sentinel.
fn sha256_name(name: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.finalize().into()
}

/// Encode a byte slice as lowercase hex.
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// The selected build artifact kind (EPIC 5.1). `--target` accepts two
/// artifact-kind keywords resolved ahead of hardware-triple parsing; any
/// other value is a hardware triple producing a `.tau` bundle.
#[derive(Debug)]
pub(crate) enum BuildTarget {
    /// Default / hardware triple → `.tau` bundle.
    Bundle(TargetTriple),
    /// `--target wasm-guest` → fully-linked wasm component.
    WasmGuest,
    /// `--target rust-lib` → generated no_std Rust library crate.
    RustLib,
}

/// Resolve the build target. Keywords `wasm-guest`/`rust-lib` select an
/// embedding artifact; `None` → host bundle; anything else is parsed as an
/// Available triple (ADR-0034). Returns a human-readable error on invalid input.
fn resolve_target(args: &BuildArgs) -> Result<BuildTarget, String> {
    match args.target.as_deref() {
        None => Ok(BuildTarget::Bundle(TargetTriple::host())),
        Some("wasm-guest") => Ok(BuildTarget::WasmGuest),
        Some("rust-lib") => Ok(BuildTarget::RustLib),
        Some(s) => {
            let triple: TargetTriple = s.parse().map_err(|e| {
                format!(
                    "invalid --target '{s}': {e}. Expected an artifact kind \
                     (wasm-guest, rust-lib) or an Available triple: {}",
                    available_triples_joined(),
                )
            })?;
            let available = tau_ports::target::lookup(&triple)
                .is_some_and(|e| matches!(e.status, tau_ports::target::TripleStatus::Available));
            if !available {
                return Err(format!(
                    "target '{triple}' is not an Available build target. Expected an \
                     artifact kind (wasm-guest, rust-lib) or an Available triple: {}",
                    available_triples_joined(),
                ));
            }
            Ok(BuildTarget::Bundle(triple))
        }
    }
}

/// Test seam: classify a `--target` value into its artifact-kind label without
/// running a build. Returns `"bundle" | "wasm-guest" | "rust-lib" | "invalid"`.
pub fn classify_target_for_test(target: Option<&str>) -> &'static str {
    let args = BuildArgs {
        project: None,
        target: target.map(|s| s.to_string()),
        output: None,
        agents: vec![],
        offline: false,
        emit_trigger: None,
        allow_ungoverned: false,
        no_governance: false,
    };
    match resolve_target(&args) {
        Ok(BuildTarget::Bundle(_)) => "bundle",
        Ok(BuildTarget::WasmGuest) => "wasm-guest",
        Ok(BuildTarget::RustLib) => "rust-lib",
        Err(_) => "invalid",
    }
}

/// Emit the rust-lib embedding crate for `project` into `out_dir`. Test seam:
/// lowers for `any-wasi-strict` (cap-fit refuses ProcessExec/AgentSpawn), derives
/// the WIT world, renders the scaffold, and writes it. Does not run governance
/// or touch `Output` — the CLI dispatch wraps this.
pub fn emit_rust_lib_to(
    project: &std::path::Path,
    out_dir: &std::path::Path,
) -> Result<RustLibArtifact> {
    use crate::cmd::build_wasm::{lower_to_wasm_ir, world_from_module};
    let (module, bytes) = lower_to_wasm_ir(project)?;
    let ir_hash = hex_lower(&tau_ir::compute_hash(&module));
    let wit = world_from_module(&module)?;
    let stem = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workflow");
    let crate_name = sanitize_crate_name(stem);

    let files = tau_sdk_codegen::render_rust_lib(tau_sdk_codegen::RustLibInput {
        crate_name: &crate_name,
        ir_bytes: &bytes,
        ir_hash: &ir_hash,
        wit: &wit,
        tau_version: env!("CARGO_PKG_VERSION"),
    });

    let mut written = 0usize;
    for (rel, contents) in &files {
        let path = out_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
        written += 1;
    }
    Ok(RustLibArtifact {
        out_dir: out_dir.to_path_buf(),
        ir_hash,
        files: written,
    })
}

/// Result of a rust-lib emission (for human/JSON output).
pub struct RustLibArtifact {
    /// Directory the generated crate was written to.
    pub out_dir: std::path::PathBuf,
    /// Lowercase-hex IR module hash baked into the crate.
    pub ir_hash: String,
    /// Number of files written.
    pub files: usize,
}

/// Lowercase, replace non-alphanumeric with `_`, so the stem is a valid crate name.
pub(crate) fn sanitize_crate_name(stem: &str) -> String {
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "workflow".to_string()
    } else {
        name
    }
}

/// `--target rust-lib` dispatch: governed-by-default gate, then emit the crate.
async fn dispatch_rust_lib(args: &BuildArgs, output: &mut Output) -> Result<()> {
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("cwd is readable"));
    let flags = crate::cmd::check::GovernanceFlags {
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    if let Err(diag) = crate::cmd::build_wasm::wasm_governance_gate(&project, flags).await {
        let _ = output.diagnostic(diag);
        std::process::exit(2);
    }
    let stem = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workflow");
    let out_dir = args
        .output
        .clone()
        .unwrap_or_else(|| project.join(format!("{stem}-rust-lib")));
    let artifact = match emit_rust_lib_to(&project, &out_dir) {
        Ok(a) => a,
        Err(e) => {
            let _ = output.error(format!("{e}"));
            std::process::exit(2);
        }
    };
    if output.is_json() {
        let _ = output.json(&serde_json::json!({
            "kind": "rust-lib",
            "path": artifact.out_dir.display().to_string(),
            "ir_hash": artifact.ir_hash,
            "files": artifact.files,
        }));
    } else {
        let _ = output.human(&format!(
            "built rust-lib crate: {} ({} files, ir {})",
            artifact.out_dir.display(),
            artifact.files,
            artifact.ir_hash,
        ));
    }
    Ok(())
}

/// `--target wasm-guest` dispatch: map the bundle-shaped args onto the wasm
/// subcommand and reuse the existing β.7.5 pipeline verbatim (no duplicated
/// lowering/build).
async fn dispatch_wasm_guest(args: &BuildArgs, output: &mut Output) -> Result<()> {
    let wasm_args = crate::cli::BuildWasmArgs {
        project: args.project.clone(),
        output: args.output.clone(),
        allow_ungoverned: args.allow_ungoverned,
        no_governance: args.no_governance,
    };
    crate::cmd::build_wasm::run(&wasm_args, output).await
}

/// Comma-joined Display list of Available registry triples (sorted).
fn available_triples_joined() -> String {
    let mut v: Vec<String> = tau_ports::target::list_available()
        .map(|e| e.triple.to_string())
        .collect();
    v.sort();
    v.join(", ")
}

/// Artifact rendering with JSON support. Emits JSON under --json,
/// human-readable text otherwise.
fn emit_artifact(artifact: &BundleArtifact, output: &mut Output) {
    if output.is_json() {
        let obj = serde_json::json!({
            "path": artifact.path.display().to_string(),
            "sha256": artifact.sha256,
            "size_bytes": artifact.size_bytes,
        });
        let _ = output.json(&obj);
    } else {
        let sha = &artifact.sha256;
        let head = &sha[..sha.len().min(6)];
        let tail = &sha[sha.len().saturating_sub(6)..];
        let _ = output.status(format!(
            "Wrote bundle: {} (sha256: {head}…{tail}, {} bytes)",
            artifact.path.display(),
            artifact.size_bytes,
        ));
        let _ = output.human(&artifact.path.display().to_string());
    }
}

/// Write host-adapter descriptors for the lowered cron triggers next to the
/// bundle. Manual triggers and systemd-unconvertible cron schedules are noted
/// and skipped. Errors writing a descriptor are surfaced but non-fatal — the
/// bundle is already built.
fn emit_trigger_descriptors(
    adapter: &str,
    bindings: &[tau_ir::trigger::TriggerBinding],
    artifact: &BundleArtifact,
    output: &mut Output,
) {
    use tau_ir::trigger::{emit_k8s, emit_systemd, TriggerKind};

    let artifact_ref = artifact.path.display().to_string();
    let descriptors = match adapter {
        "systemd" => emit_systemd(bindings, &artifact_ref),
        "k8s" => emit_k8s(bindings, &artifact_ref),
        other => {
            let _ = output.error(format!("unknown --emit-trigger adapter: {other}"));
            return;
        }
    };

    if descriptors.is_empty() {
        let cron_count = bindings
            .iter()
            .filter(|b| b.kind == TriggerKind::Cron)
            .count();
        if cron_count == 0 {
            let _ = output.status("No cron triggers to emit (manual triggers need no scheduler).");
        } else {
            let _ = output.status(format!(
                "{cron_count} cron trigger(s) present but none were emittable for {adapter} \
                 (systemd's OnCalendar supports only `*` and plain-integer cron fields; \
                 ranges/lists/steps are skipped — use --emit-trigger=k8s for those)."
            ));
        }
        return;
    }

    let dir = artifact
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    for (filename, content) in descriptors {
        let path = dir.join(&filename);
        match std::fs::write(&path, content.as_bytes()) {
            Ok(()) => {
                let _ = output.status(format!("Wrote trigger descriptor: {}", path.display()));
            }
            Err(e) => {
                let _ = output.error(format!("failed to write {}: {e}", path.display()));
            }
        }
    }
}

/// Maps a [`BuildError`] to its CLI exit code per spec §6.
fn exit_code_for(err: &BuildError) -> u8 {
    match err {
        BuildError::MissingLockfile
        | BuildError::PackageNotInstalled { .. }
        | BuildError::AgentHomePackageMissing { .. } => 3,
        BuildError::ProjectConfig(_)
        | BuildError::LockfileLoad(_)
        | BuildError::ManifestInvalid(_)
        | BuildError::UnknownAgent { .. }
        | BuildError::AgentHomePackageManifest { .. } => 2,
        BuildError::TreeHashFailed { .. }
        | BuildError::PromptResolveFailed { .. }
        | BuildError::CapabilityOverrideFailed { .. }
        | BuildError::WriteFailed { .. } => 70,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BuildArgs;
    use tau_ports::target::TargetTriple;

    fn args_with_target(t: Option<&str>) -> BuildArgs {
        BuildArgs {
            project: None,
            target: t.map(|s| s.to_string()),
            output: None,
            agents: vec![],
            offline: false,
            emit_trigger: None,
            allow_ungoverned: false,
            no_governance: false,
        }
    }

    #[test]
    fn resolve_target_defaults_to_host() {
        assert!(matches!(
            resolve_target(&args_with_target(None)).unwrap(),
            BuildTarget::Bundle(t) if t == TargetTriple::host()
        ));
    }

    #[test]
    fn resolve_target_accepts_available_triple() {
        // `passthrough` is Available on every host, including Windows.
        let available = TargetTriple::PASSTHROUGH;
        assert!(matches!(
            resolve_target(&args_with_target(Some(&available.to_string()))).unwrap(),
            BuildTarget::Bundle(t) if t == available
        ));
    }

    #[test]
    fn resolve_target_accepts_windows_native_strict_now_available() {
        // Phase 2 graduated `windows-native-strict` Reserved -> Available,
        // closing the `host()` divergence gap: on Windows, `host()` returns
        // this exact triple, and it must now resolve successfully (it no
        // longer hits the Reserved rejection branch).
        let t: TargetTriple = "windows-native-strict".parse().unwrap();
        assert!(matches!(
            resolve_target(&args_with_target(Some(&t.to_string()))).unwrap(),
            BuildTarget::Bundle(bt) if bt == t
        ));
    }

    #[test]
    fn resolve_target_rejects_unparseable() {
        let err = resolve_target(&args_with_target(Some("not a triple!!!"))).unwrap_err();
        assert!(err.contains("invalid --target"), "got {err}");
    }

    #[test]
    fn resolve_target_maps_artifact_keywords() {
        assert!(matches!(
            resolve_target(&args_with_target(Some("wasm-guest"))).unwrap(),
            BuildTarget::WasmGuest
        ));
        assert!(matches!(
            resolve_target(&args_with_target(Some("rust-lib"))).unwrap(),
            BuildTarget::RustLib
        ));
    }

    #[test]
    fn resolve_target_invalid_names_both_value_spaces() {
        let err = resolve_target(&args_with_target(Some("not a triple!!!"))).unwrap_err();
        assert!(
            err.contains("wasm-guest") && err.contains("rust-lib"),
            "got {err}"
        );
    }

    #[test]
    fn resolve_target_rejects_unregistered_triple() {
        // "darwin-container-strict" parses the platform-adapter-tier grammar
        // but is not in the registry at all — exercises the lookup-None
        // branch of the Available check (no registered Reserved triples
        // remain post-Phase-2, so this is the only "not Available" case).
        let err = resolve_target(&args_with_target(Some("darwin-container-strict"))).unwrap_err();
        assert!(err.contains("not an Available"), "got {err}");
    }

    #[test]
    fn exit_code_mapping_per_spec() {
        // Install-state errors → 3.
        assert_eq!(exit_code_for(&BuildError::MissingLockfile), 3);
        assert_eq!(
            exit_code_for(&BuildError::PackageNotInstalled {
                name: "foo".into(),
                path: "/nowhere".into(),
            }),
            3,
        );

        // Config/parse/manifest errors → 2.
        assert_eq!(exit_code_for(&BuildError::ProjectConfig("x".into())), 2);
        assert_eq!(exit_code_for(&BuildError::LockfileLoad("x".into())), 2);
        assert_eq!(exit_code_for(&BuildError::ManifestInvalid("x".into())), 2);

        // Internal / IO errors → 70.
        assert_eq!(
            exit_code_for(&BuildError::WriteFailed {
                path: "/dev/null".into(),
                source: std::io::Error::other("x"),
            }),
            70,
        );
        assert_eq!(
            exit_code_for(&BuildError::PromptResolveFailed {
                id: "a".into(),
                source: std::io::Error::other("x"),
            }),
            70,
        );

        // Unknown agent (bad --agent input) → 2.
        assert_eq!(
            exit_code_for(&BuildError::UnknownAgent {
                id: "ghost".into(),
                available: vec!["alpha".into()],
            }),
            2,
        );

        // Override-agent home package missing -> install-state -> 3.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageMissing {
                id: "r".into(),
                package: "homepkg".into(),
            }),
            3,
        );
        // Home-package manifest unreadable -> config/parse -> 2.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageManifest {
                id: "r".into(),
                package: "homepkg".into(),
                source: tau_pkg::error::ManifestReadError::NotFound { path: "x".into() },
            }),
            2,
        );
    }

    /// `sha256_name` must return the same bytes for the same input — this
    /// is what keeps `canonical_ir_hash` reproducible across `tau build`
    /// invocations of the same source tree.
    #[test]
    fn sha256_name_is_deterministic_per_input() {
        assert_eq!(sha256_name("ReadTemp"), sha256_name("ReadTemp"));
        assert_eq!(sha256_name(""), sha256_name(""));
    }

    /// `sha256_name` must distinguish symbolic names — two distinct tools
    /// must produce two distinct content hashes so any future drift-
    /// detection layer can actually tell them apart.
    #[test]
    fn sha256_name_distinguishes_distinct_names() {
        assert_ne!(sha256_name("A"), sha256_name("B"));
        assert_ne!(sha256_name("ReadTemp"), sha256_name("ReadHumidity"));
    }

    /// `sha256_name` is never the zero sentinel — that's the
    /// `tau_ir_lower::typecheck` "unknown native tool" tripwire and
    /// would re-introduce the silent-IR-loss bug A.2 is fixing.
    #[test]
    fn sha256_name_is_never_zero_sentinel() {
        assert_ne!(sha256_name("ReadTemp"), [0u8; 32]);
        assert_ne!(sha256_name(""), [0u8; 32]);
    }

    /// End-to-end regression for A.2: a project with a `[tools.<x>] native = "…"`
    /// entry must lower to an `IrPayload` instead of falling through to
    /// the `None` warn-and-continue path. Before A.2, the zero-sentinel
    /// cache caused this to return `None`.
    #[test]
    fn lower_ir_yields_payload_for_native_tool_project() {
        let scratch = tempfile::tempdir().unwrap();
        let project = scratch.path();
        // Minimal native-tool project: one agent + one [tools.<x>] native
        // entry. The agent doesn't have to reference the tool — lowering
        // emits every project-level [tools.<x>] entry into the workflow.
        std::fs::write(
            project.join("tau.toml"),
            r#"
packages = ["anthropic"]

[project]
name = "native_smoke"
version = "0.1.0"

[models.default]
backend = "anthropic"
model = "claude-haiku-4-5"

[agents.solo]
display_name = "Solo"
package = "native_smoke@^0.1"
model = "default"

[agents.solo.prompt]
system = "hi"

[tools.read_temp]
native = "ReadTemp"
description = "reads the temperature"
capabilities = []
"#,
        )
        .unwrap();

        let target = TargetTriple::PASSTHROUGH;
        // No MCP entries → empty cache.
        let mcp_cache = std::collections::BTreeMap::new();
        let LowerIrResult { payload, .. } = lower_ir(project, &target, &mcp_cache, None);
        assert!(
            payload.is_some(),
            "lower_ir must return Some(IrPayload) for a project with a [tools.<x>] native = ... entry; \
             was None — did the native_tool cache regress to the zero sentinel?",
        );
        let payload = payload.unwrap();
        assert!(!payload.canonical_ir_hash.is_empty());
        assert!(!payload.canonical_ir_bytes_hex.is_empty());
    }

    #[test]
    fn contract_pin_path_nests_slash_names() {
        let base = std::path::Path::new(".tau/mcp");
        assert_eq!(
            contract_pin_path(base, "github/search"),
            std::path::Path::new(".tau/mcp/github/search.contract.json")
        );
        assert_eq!(
            contract_pin_path(base, "plain"),
            std::path::Path::new(".tau/mcp/plain.contract.json")
        );
    }
}
