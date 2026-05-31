//! Lowering pass: `tau_pkg::ProjectConfig` → `IrModule`.
//!
//! The lowering pass is pure: it consumes an already-parsed
//! `ProjectConfig`, resolves external references against caches the
//! caller supplies (native tool registry, MCP contract cache, skill
//! content-hash table), runs the capability-fit check against the
//! target triple, and produces a typed `IrModule`. Any error short-
//! circuits with `IrError`.
//!
//! `tau build` is the caller; `tau dev` is also a caller (it lowers
//! once per source change to drive the interpreter against a fresh IR).

pub mod capability_fit;
pub mod parse;
pub mod resolve;
pub mod typecheck;

use tau_pkg::project::ProjectConfig;
use tau_ports::target::TargetTriple;

use crate::error::IrError;
use crate::module::IrModule;

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
/// use tau_ir::lower::{lower_project, Caches};
/// use tau_pkg::project::ProjectConfig;
/// use tau_ports::target::registry;
///
/// let toml = r#"
///     [project]
///     name = "demo"
/// "#;
/// let config = ProjectConfig::parse_str(toml).unwrap();
/// let target = registry::list_available().next().unwrap().triple().clone();
/// let caches = Caches {
///     native_tool: &|_| None,
///     mcp_contract: &|_| None,
///     skill: &|_| None,
/// };
/// let module = lower_project(&config, &target, &caches).unwrap();
/// assert_eq!(module.ir_format.0, tau_ir::IrFormatVersion::CURRENT);
/// ```
pub fn lower_project(
    config: &ProjectConfig,
    target: &TargetTriple,
    caches: &Caches,
) -> Result<IrModule, IrError> {
    let parsed = parse::parse(config)?;
    let resolved = resolve::resolve(parsed, caches)?;
    typecheck::typecheck(&resolved)?;
    capability_fit::check(&resolved, target)?;
    Ok(build_module(resolved, target))
}

/// Caches the caller supplies for resolution. Each is a closure over an
/// existing tau-pkg / tau-cli registry so the lowering pass stays pure.
pub struct Caches<'a> {
    /// Resolves a native tool symbolic name to its content hash.
    pub native_tool: &'a dyn Fn(&str) -> Option<[u8; 32]>,
    /// Resolves an MCP URL to (contract hash, declared capabilities).
    pub mcp_contract:
        &'a dyn Fn(&str) -> Option<([u8; 32], crate::capability::CapabilityRequirements)>,
    /// Resolves a skill name to its content hash (from Skills-2 lockfile).
    pub skill: &'a dyn Fn(&str) -> Option<[u8; 32]>,
}

fn build_module(parsed: crate::lower::parse::Parsed, target: &TargetTriple) -> IrModule {
    IrModule {
        ir_format: crate::IrFormatVersion::current(),
        tau_version: env!("CARGO_PKG_VERSION").into(),
        target: *target,
        workflow: parsed.workflow,
    }
}
