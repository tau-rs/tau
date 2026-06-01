//! Host lifecycle for a contracted MCP server.
//!
//! Scaffold only in PR-1. PR-2 (stdio) + PR-3 (HTTP) wire:
//!
//! - `open(url, &CapabilityPlan)` discriminates transport and dials.
//! - handshake: `initialize` + `tools/list`.
//! - keepalive + shutdown.
//!
//! See the β.3 design doc §2 + §8.
