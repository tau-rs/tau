//! `McpBridge` — composable `ToolDispatcher` adapter.
//!
//! Scaffold only in PR-1. PR-5 fills this in with:
//!
//! - `BTreeMap<ToolId, (Arc<McpClient>, server_tool_name, caps)>`.
//! - `impl ToolDispatcher for McpBridge`.
//! - Outbound cap-gate enforcement.
//! - Composition with `tau-cli::ForwardingDispatcher`.
//!
//! See the β.3 design doc §8.
