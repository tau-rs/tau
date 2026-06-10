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

/// Entry point for `tau dev`. Loads the project session and enters the REPL.
/// Phase 6 will branch on `args.prompt.is_some()` for `-p` one-shot mode.
pub async fn run(args: DevArgs, output: &mut Output) -> Result<()> {
    let mut session = DevSession::load(args.project, args.agent).await?;
    repl::run_loop(&mut session, output).await?;
    Ok(())
}
