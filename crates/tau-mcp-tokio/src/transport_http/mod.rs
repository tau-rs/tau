//! Streamable HTTP transport for MCP servers.
//!
//! Scaffold only in PR-1. PR-3 fills this in with:
//!
//! - `connect(url, &CapabilityPlan)` using reqwest + SSE parsing.
//! - Per-call net.http cap enforcement via host-pinning middleware.
//! - `Transport` impl carrying the HTTP client + SSE stream.
//!
//! See the β.3 design doc §2 + §9.
