//! `tau build wasm <project>` — IR-to-wasm AOT compiler (β.7.5).
//!
//! Phase 1 (PR-1): CLI surface only. The lowering → IR-bake → cargo
//! (`wasm32-wasip2`) → `.wasm` pipeline lands in β.7.5 Phase 4.

use anyhow::Result;

use crate::cli::BuildWasmArgs;
use crate::output::Output;

/// CLI entry point for `tau build wasm`.
///
/// Phase 1: always returns a "not yet implemented" error. The actual
/// wasm compilation pipeline is wired in β.7.5 Phase 4.
pub async fn run(_args: &BuildWasmArgs, _output: &mut Output) -> Result<()> {
    anyhow::bail!("`tau build wasm` is not yet implemented (β.7.5 Phase 4)")
}
