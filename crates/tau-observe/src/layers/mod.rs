//! Custom `tracing_subscriber::Layer` impls that materialize internal
//! events into on-disk JSONL artifacts. See sub-project D in the
//! 2026-05-17 logging upgrades design.

pub mod plugin_recording;
pub mod workflow_run_log;
