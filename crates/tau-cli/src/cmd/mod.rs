//! Subcommand handlers. Each subcommand has its own submodule with
//! a `pub async fn run(args: &<SubArgs>) -> anyhow::Result<()>`
//! entry point.
//!
//! Tasks 10-14 implement each handler. At Task 4 they are stubs
//! returning an "unimplemented" error.

pub mod chat;
pub mod check;
pub mod error_render;
pub mod init;
pub mod install;
pub mod list;
pub mod output_orchestration;
pub mod plugin;
pub(crate) mod plugin_loader;
pub mod resolve;
pub(crate) mod resolve_helpers;
pub mod run;
pub mod sandbox;
pub mod serve;
pub mod session;
pub mod skill;
pub mod target;
pub mod uninstall;
pub mod update;
pub mod verify;
pub mod workflow;
