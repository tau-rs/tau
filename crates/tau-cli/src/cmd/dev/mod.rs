//! `tau dev <project>` — hot-reload REPL.
//!
//! See spec at `docs/superpowers/specs/2026-06-10-beta-7-tau-dev-design.md`
//! and ADR-0040 (forthcoming).

pub mod repl;
pub mod session;
pub mod watcher;

use anyhow::Result;

use crate::cli::DevArgs;
use crate::output::Output;
use session::DevSession;

/// Entry point for `tau dev`.
///
/// When `--prompt` (`-p`) is supplied, runs exactly one turn and exits
/// (single-shot mode). When omitted, enters the interactive REPL loop.
/// Phase 6 will layer `--watch` auto-reload on top.
pub async fn run(args: DevArgs, output: &mut Output) -> Result<()> {
    let mut session = DevSession::load(args.project, args.agent).await?;
    match args.prompt {
        Some(p) => session.run_turn(&p, output).await,
        None => repl::run_loop(&mut session, output).await,
    }
}
