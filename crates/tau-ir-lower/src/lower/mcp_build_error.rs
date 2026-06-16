//! Build-time errors specific to MCP contract resolution + expansion
//! (per β.3 design doc §5 build-time invariants table).
//!
//! All variants surface through `LowerError::McpBuild(...)`; the `tau check`
//! aggregator renders them with exit code 64 (validation).

use thiserror::Error;

/// Per-spec §5 build-time invariant violations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum McpBuildError {
    /// Live resolver couldn't reach the MCP server (network / spawn /
    /// handshake failure). Re-raised from the resolver's typed error.
    #[error("MCP contract unreachable for entry {entry:?}: {reason}")]
    ContractUnreachable {
        /// `[tools.<entry>]` name from tau.toml.
        entry: String,
        /// Resolver-side reason (e.g. "handshake timeout after 30000ms").
        reason: String,
    },
    /// One or more server-tool capability requirements aren't covered by
    /// the author's envelope.
    #[error(
        "envelope does not cover server-tool {tool:?} capabilities for entry {entry:?}: \
         missing {missing:?}"
    )]
    EnvelopeCoversContract {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Server-side tool name.
        tool: String,
        /// Capabilities the contract declared that the envelope omits
        /// (rendered as `kind`/`host`/`path` keys for diagnostics).
        missing: Vec<String>,
    },
    /// `roots = [...]` declared in tau.toml are not all covered by the
    /// envelope's `fs.read` capabilities.
    #[error("roots {roots:?} for entry {entry:?} not covered by fs.read caps")]
    RootsExceedFsCaps {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Offending root paths.
        roots: Vec<String>,
    },
    /// The MCP server's contract requires sampling (any tool's caps
    /// include `sampling.*`) but the author left `sampling.models = []`.
    #[error("entry {entry:?} server contract requires sampling but sampling.models is empty")]
    SamplingRequiredByContract {
        /// `[tools.<entry>]` name.
        entry: String,
    },
    /// `--offline` was passed but `.tau/mcp/<entry>.contract.json` is missing.
    #[error(
        "entry {entry:?}: --offline requested but pinned contract file is missing at {path:?}"
    )]
    PinnedContractMissing {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Expected path on disk.
        path: String,
    },
    /// A server-tool name contains `.`, which would collide with the
    /// `<entry>.<server-tool>` ToolId convention.
    #[error("server-tool name {name:?} for entry {entry:?} contains '.', which is reserved as the ToolId separator")]
    ServerToolNameContainsDot {
        /// `[tools.<entry>]` name.
        entry: String,
        /// Server-side tool name that contains `.`.
        name: String,
    },
}
