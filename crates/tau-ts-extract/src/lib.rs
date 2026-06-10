//! TypeScript source extractor — produces `ProjectConfig` from a
//! `project.ts` source via swc-based static AST analysis.
//!
//! See `docs/superpowers/specs/2026-06-10-beta-8-ts-authoring-design.md`
//! and ADR-0041 (forthcoming) for the design.
//!
//! β.8 v1 scope: declarations only. Tool bodies (`run: async () => ...`)
//! are rejected at parse time. δ.2 will add runtime JS execution via
//! QuickJS embed for inline tool bodies.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
#[allow(missing_docs)]
mod factory;
#[allow(missing_docs)]
mod lower;
#[allow(missing_docs)]
pub(crate) mod parse;
#[allow(missing_docs)]
pub(crate) mod scope;

pub use error::TsExtractError;
use std::path::Path;
use tau_pkg::project::project::ProjectConfig;

/// Extract a `ProjectConfig` from a TypeScript source string.
///
/// `source_path` is used only for error positioning (file:line:col).
/// The function does NOT read from disk — caller is responsible for
/// reading + UTF-8 validation.
pub fn extract_project(
    source: &str,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    let (module, _sm) = parse::parse_module(source, source_path)?;
    let names = scope::collect_top_level(&module);
    lower::build_project_config(&module, &names, source_path)
}
