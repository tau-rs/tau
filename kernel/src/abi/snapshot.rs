//! The snapshot header — what a reader checks before it trusts a fold it
//! did not do (ADR-0011).
//!
//! A snapshot is two lines: this header, then the canonical reducer state
//! after the first `seq` entries of a log. The state is *not* frozen — it is
//! the reducer's to correct, versioned by the fold number — so the header is
//! the only part a reader from any build can rely on parsing. Its job is to
//! make every refusal legible: a reader that cannot use a snapshot says which
//! field and which two values, and refolds from zero.

use serde::{Deserialize, Serialize};

/// The first line of a snapshot file.
///
/// Every field exists for a check the reader makes before deserializing the
/// state on line two (ADR-0011 §1):
///
/// | field | the reader checks | or else |
/// |---|---|---|
/// | `magic` | it is [`MAGIC`](Self::MAGIC) | not a snapshot |
/// | `abi` | `<=` this build's `ABI` | the state may hold a shape this build cannot parse |
/// | `fold` | `==` this build's fold number | the state is another reducer's fold |
/// | `seq` | `<=` the log's length | the tail cannot start past the end |
/// | `prefix` | sha256 of the log as written, header line through entry `seq - 1` | a snapshot of another log whose ids happen to line up |
/// | `state` | the reducer's hash of line two | the writer's canonical serialization is not this build's |
///
/// `prefix` and `state` are lowercase hex sha256 digests, 64 characters each.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotHeader {
    /// Magic bytes identifying a tau snapshot: `TAUS`.
    pub magic: [u8; 4],
    /// The ABI version of the build that wrote the snapshot.
    pub abi: u16,
    /// The fold version of the build that wrote the snapshot: which reducer
    /// produced the state on line two.
    pub fold: u16,
    /// How many entries were folded into the state: where the tail begins.
    pub seq: u64,
    /// sha256, as lowercase hex, of the log as `Log::write_to` writes it —
    /// the header line, then entries `0..seq`, each followed by `\n`.
    pub prefix: String,
    /// The reducer's hash of the state on line two, as lowercase hex.
    pub state: String,
}

impl SnapshotHeader {
    /// The magic bytes at the start of every tau snapshot.
    pub const MAGIC: [u8; 4] = *b"TAUS";
}
