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
mod parse;
#[allow(missing_docs)]
mod scope;

pub use error::TsExtractError;
use std::path::Path;
use tau_pkg::project::project::ProjectConfig;

/// Extract a `ProjectConfig` from a TypeScript source string.
///
/// `source_path` is used only for error positioning (file:line:col).
/// The function does NOT read from disk — caller is responsible for
/// reading + UTF-8 validation.
///
/// Phase 1: stub. Phase 2+ fills this in.
pub fn extract_project(
    _source: &str,
    source_path: &Path,
) -> Result<ProjectConfig, TsExtractError> {
    Err(TsExtractError::ParseError {
        file: source_path.to_path_buf(),
        line: 0,
        col: 0,
        message: "not yet implemented (β.8 Phase 1 scaffold)".to_string(),
    })
}
