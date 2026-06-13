#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! `fs-write` Tool plugin internals.
//!
//! The binary entrypoint at `src/main.rs` calls
//! `tau_plugin_sdk::run_tool_with_config::<FsWritePlugin>(...)`.
//!
//! Write-side mirror of the `fs-read` plugin. See
//! `docs/superpowers/specs/2026-06-13-fs-write-edit-plugin-design.md`.

pub mod config;
pub(crate) mod path_check;
pub mod plugin;
