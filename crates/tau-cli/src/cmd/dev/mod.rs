//! `tau dev <project>` — hot-reload REPL.
//!
//! See spec at `docs/superpowers/specs/2026-06-10-beta-7-tau-dev-design.md`
//! and ADR-0040 (forthcoming).

pub mod session;

use anyhow::Result;

use crate::cli::DevArgs;
use crate::output::Output;

/// Entry point for `tau dev`. Phase 2+ fills this in; Phase 1 is a stub
/// so clap parses + smoke test passes.
pub async fn run(_args: DevArgs, _output: &mut Output) -> Result<()> {
    anyhow::bail!("not yet implemented (β.7 Phase 1 scaffold)")
}
