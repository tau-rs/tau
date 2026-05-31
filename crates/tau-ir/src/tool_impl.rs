//! Tool implementation references.
//!
//! A [`Tool`] node carries a [`ToolImpl`] that distinguishes native
//! tools (statically linked Rust) from MCP-contracted tools (external
//! servers reached via the MCP wire). The lowering pass resolves
//! [`ToolImpl::Native::content_hash`] and [`ToolImpl::Mcp::contract_hash`]
//! at build time so every IR module is fully hashable per D-6.

use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::capability::CapabilityRequirements;

/// 32-byte content hash (SHA-256 output) used to pin tool implementations
/// and MCP contracts at build time.
pub type Hash256 = [u8; 32];

/// A reference to a statically linked native tool by symbolic name.
///
/// The symbolic name (e.g. `"ReadTemp"`) is the Rust identifier of the
/// `impl Tool for X` type. The lowering pass resolves it against the
/// project's native tool registry; AOT (β.7) lowers the call site
/// directly. v0's interpreter dispatches by name through a
/// `NativeFnRegistry` injected at runtime.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct NativeFnRef {
    /// Symbolic name of the Rust `Tool` impl.
    pub name: String,
}

/// How a [`crate::Tool`] node's behavior is provided at runtime.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum ToolImpl {
    /// Statically linked native tool.
    Native {
        /// Reference to the native Rust impl by symbolic name.
        fn_ref: NativeFnRef,
        /// Hash of the impl's source bytes (Rust source, dependencies'
        /// content hashes). Participates in the IR module hash.
        content_hash: Hash256,
    },
    /// MCP-contracted external server.
    Mcp {
        /// MCP server URL (e.g. `"https://mcp.weather.com"`).
        url: String,
        /// Content hash of the MCP contract (the cached schema + capability
        /// declaration the server advertises at handshake). Participates in
        /// the IR module hash so a contract drift invalidates the bundle.
        contract_hash: Hash256,
        /// The subset of capabilities this MCP server is bounded to (a
        /// subset of the contract's declared capabilities; narrowed by
        /// `tau.toml` overrides).
        capability_subset: CapabilityRequirements,
    },
}
