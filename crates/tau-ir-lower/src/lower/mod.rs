//! Lowering pass: `tau_pkg::ProjectConfig` → `IrModule`.
//!
//! The lowering pass is pure: it consumes an already-parsed
//! `ProjectConfig`, resolves external references against caches the
//! caller supplies (native tool registry, MCP contract cache, skill
//! content-hash table), runs the capability-fit check against the
//! target triple, and produces a typed `IrModule`. Any error short-
//! circuits with `LowerError`.
//!
//! `tau build` is the caller; `tau dev` is also a caller (it lowers
//! once per source change to drive the interpreter against a fresh IR).

pub mod capability_fit;
pub mod mcp_build_error;
pub mod parse;
pub mod resolve;
pub mod typecheck;

pub use mcp_build_error::McpBuildError;

use tau_pkg::project::ProjectConfig;
use tau_ports::target::TargetTriple;

use crate::error::LowerError;
use tau_ir::capability::CapabilityRequirements;
use tau_ir::module::IrModule;

/// Per-server-tool slice of a resolved MCP contract.
///
/// The resolve stage expands one `ToolImpl::Mcp` author entry into N IR
/// nodes (one per server-tool). Each node uses one of these structs to
/// fill in its `capability_subset` + `server_tool_name` + (separately)
/// the agent's `tool_refs` rewrite.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedServerTool {
    /// Server-side tool name (what tau-mcp-tokio sends on `tools/call`).
    pub name: alloc::string::String,
    /// Capability requirements the server declares for this tool.
    pub caps: CapabilityRequirements,
    /// JSON schema for the tool's input (passed through to IR; opaque to
    /// the resolver).
    pub input_schema: serde_json::Value,
}

/// Error returned by the injected `prompt_file` reader (see [`Caches`]).
///
/// Carries a human-readable reason (typically an `std::io::Error` rendered by
/// the caller); lowering wraps it into [`LowerError::PromptFileUnreadable`].
#[derive(Debug, Clone)]
pub struct PromptFileError(pub alloc::string::String);

/// Output of [`lower_project`]: the lowered module plus the content-addressed
/// asset blobs (currently agent prompts) the bundle must carry (D6-B).
///
/// Assets are keyed by their hash (`"sha256:" + 64 lowercase hex`), matching
/// the [`tau_ir::prompt::PromptSource::Asset`] references inside `module`.
/// Identical content across agents dedupes to one blob by construction.
#[derive(Debug)]
pub struct LowerOutput {
    /// The lowered IR module.
    pub module: IrModule,
    /// Content-addressed assets referenced by `module`, keyed by hash.
    pub assets: alloc::collections::BTreeMap<alloc::string::String, tau_ir::asset::AssetBlob>,
}

/// Resolved MCP contract for one author-side `[tools.<entry>]` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMcpContract {
    /// SHA-256 of the canonical contract — participates in the IR
    /// module hash so contract drift invalidates the bundle.
    pub hash: [u8; 32],
    /// All server-side tools the contract advertises (one ToolImpl::Mcp
    /// IR node will be emitted per entry).
    pub expanded_tools: alloc::vec::Vec<ResolvedServerTool>,
    /// Whether the contract advertises `sampling/*` capabilities. Used
    /// by resolve to enforce the `SamplingRequiredByContract` invariant.
    pub requires_sampling: bool,
}

/// Lower a parsed `ProjectConfig` into an `IrModule` for the given target.
///
/// Pipeline:
/// 1. `parse` — extract per-agent and per-tool declarations.
/// 2. `resolve` — resolve native tool content-hashes, MCP contract
///    hashes, skill content-hashes (caller-supplied caches).
/// 3. `typecheck` — agents' tool_refs exist, subflow targets exist,
///    cap_subset is a subset of parent grant.
/// 4. `capability_fit` — every required shape supported by `target`.
///
/// # Example
///
/// ```
/// use tau_ir_lower::{lower_project, Caches};
/// use tau_pkg::project::ProjectConfig;
/// use tau_ports::target::registry;
///
/// let toml = r#"
///     [project]
///     name = "demo"
/// "#;
/// let config = ProjectConfig::parse_str(toml).unwrap();
/// let target = registry::list_available().next().unwrap().triple;
/// let caches = Caches {
///     native_tool: &|_| None,
///     mcp_contract: &|_| None,
///     skill: &|_| None,
///     prompt_file: &|_| Ok(Vec::new()),
/// };
/// let out = lower_project(&config, &target, &caches).unwrap();
/// assert_eq!(out.module.ir_format.0, tau_ir::IrFormatVersion::CURRENT);
/// assert!(out.assets.is_empty());
/// ```
pub fn lower_project(
    config: &ProjectConfig,
    target: &TargetTriple,
    caches: &Caches,
) -> Result<LowerOutput, LowerError> {
    let parsed = parse::parse(config, caches.prompt_file)?;
    let resolved = resolve::resolve(parsed, caches)?;
    typecheck::typecheck(&resolved)?;
    capability_fit::check(&resolved, target)?;
    Ok(build_output(resolved, target))
}

/// Caches the caller supplies for resolution. Each is a closure over an
/// existing tau-pkg / tau-cli registry so the lowering pass stays pure.
pub struct Caches<'a> {
    /// Resolves a native tool symbolic name to its content hash.
    pub native_tool: &'a dyn Fn(&str) -> Option<[u8; 32]>,
    /// Resolves an MCP URL to its fully-expanded contract (per
    /// β.3 design doc §5). Returns `None` only if `tau build` did not
    /// pre-fetch this URL — the resolver typically errors instead.
    pub mcp_contract: &'a dyn Fn(&str) -> Option<ResolvedMcpContract>,
    /// Resolves a skill name to its content hash (from Skills-2 lockfile).
    ///
    /// **Reserved for a future `ToolImpl::Skill` variant.** No code path
    /// in v0 calls this closure; callers may supply a `|_| None` stub.
    /// The field stays in `Caches` to lock the resolver signature so
    /// adding `ToolImpl::Skill` later is non-breaking.
    pub skill: &'a dyn Fn(&str) -> Option<[u8; 32]>,
    /// Reads an agent `system_file` prompt's bytes at build time (D6-B).
    ///
    /// Lowering hashes the returned bytes and emits a
    /// [`tau_ir::prompt::PromptSource::Asset`] reference plus an asset blob
    /// (collected into [`LowerOutput::assets`]). The closure resolves the
    /// path (relative paths against the project root) and returns the
    /// content; a read failure becomes a
    /// [`LowerError::PromptFileUnreadable`] build error. Callers with no
    /// file prompts may supply a `|_| Ok(Vec::new())` stub.
    pub prompt_file: &'a dyn Fn(&std::path::Path) -> Result<alloc::vec::Vec<u8>, PromptFileError>,
}

fn build_output(parsed: crate::lower::parse::Parsed, target: &TargetTriple) -> LowerOutput {
    let crate::lower::parse::Parsed {
        workflow,
        triggers,
        assets,
    } = parsed;
    // Option B (ADR-0044 §D1): triggers do NOT bump ir_format. ir_format is set
    // to `IrFormatVersion::current()` (advanced since ADR-0044 for unrelated IR
    // changes); the point here is that adding the `triggers` field did not force
    // a bump. The `triggers` field's skip-empty serialization preserves
    // trigger-less hashes; the appended array differentiates trigger-bearing
    // hashes on its own.
    let module = IrModule {
        ir_format: tau_ir::IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target: *target,
        workflow,
        triggers,
    };
    LowerOutput { module, assets }
}
