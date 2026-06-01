//! Subprocess stdio transport for MCP servers.
//!
//! Scaffold only in PR-1. PR-2 fills this in with:
//!
//! - `spawn(cmd, &CapabilityPlan)` that wraps `tokio::process::Command`
//!   via `tau_runtime_tokio::process_gate::Sandbox::wrap_spawn`.
//! - line-delimited JSON-RPC framing over child stdout / stdin.
//! - `Transport` impl carrying the spawned child + framers.
//!
//! See the β.3 design doc §2 (crate layout) and §9 (sandbox model).
