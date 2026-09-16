//! Snapshots: a fold you may skip, never one you must trust (ADR-0011).
//!
//! A [`Snapshot`] is the canonical reducer [`State`] after the first `k`
//! entries of a log, plus a [`SnapshotHeader`] a reader checks before it
//! deserializes the state. Two lines of JSON, framed like a log: line 1 the
//! header, line 2 the state as [`State::hash`] serializes it.
//!
//! ```text
//! {"magic":[84,65,85,83],"abi":2,"fold":1,"seq":5017,"prefix":"…","state":"…"}
//! {"next_seq":5017,"next_agent":…,"agents":{…},…}
//! ```
//!
//! # Two ways to refuse, and why they differ
//!
//! [`Snapshot::read_from`] refuses with a [`SnapshotError`] when the file is
//! not one *this build* can use: wrong magic, a newer `abi`, another
//! reducer's `fold`, a body it cannot parse. The remedy is another build, or
//! a refold from zero. [`Snapshot::join`] refuses with a [`JoinError`] when
//! the snapshot and the log do not belong together: the offset is past the
//! end, the prefix digest disagrees, the state does not re-hash to what the
//! header claims, or the body's own offset contradicts the header's. No build
//! will make those two fit. `tau replay` gives each its own exit code (7 and
//! 8) for exactly that reason.
//!
//! The join checks run cheapest-first and most-specific-first: the offset
//! before any hashing, the prefix digest over lines the reader skips anyway,
//! the state re-hash — which catches a canonical field added or dropped
//! without a [`FOLD`] bump, the policy violation made loud — and `next_seq`
//! last, because if the first three hold it can only disagree by a corrupted
//! body.
//!
//! A snapshot is never authoritative. Nothing here reads one without the log
//! it was taken from, and no check can be skipped.

use core::fmt;
use std::io::{self, BufRead, Write};

use crate::abi::{SnapshotHeader, ABI};
use crate::log::Log;
use crate::reducer::{State, StateHash, FOLD};

/// A snapshot in memory: a header and the state it describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    header: SnapshotHeader,
    state: State,
}

/// Why a snapshot file is not usable by this build (`tau replay` exit 7).
///
/// Every variant that names a mismatch names both values, so the message
/// tells a reader which build would accept the file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SnapshotError {
    /// The underlying writer or reader failed.
    #[error("snapshot i/o failed")]
    Io(#[from] io::Error),
    /// The input ended before a header.
    #[error("snapshot has no header")]
    MissingHeader,
    /// The first line was not a header.
    #[error("snapshot header is malformed: {0}")]
    MalformedHeader(#[source] serde_json::Error),
    /// The magic bytes are not a snapshot's.
    #[error("snapshot header has magic {found:?}, not a tau snapshot ({expected:?})")]
    Magic {
        /// What the file says.
        found: [u8; 4],
        /// What a snapshot says.
        expected: [u8; 4],
    },
    /// Written by a build with a newer ABI: the state may hold a shape this
    /// build cannot parse.
    #[error("snapshot header says abi {found}; this build reads abi <= {reads}")]
    Abi {
        /// The writer's `ABI`.
        found: u16,
        /// This build's `ABI`.
        reads: u16,
    },
    /// Written by a different reducer: the state is not the fold this build
    /// would have produced.
    #[error("snapshot header says fold {found}; this build folds at fold {expected}")]
    Fold {
        /// The writer's `FOLD`.
        found: u16,
        /// This build's `FOLD`.
        expected: u16,
    },
    /// The input ended after the header.
    #[error("snapshot has no state line")]
    MissingState,
    /// The second line did not parse as a state.
    #[error("snapshot state is malformed: {0}")]
    MalformedState(#[source] serde_json::Error),
}

/// Why a snapshot is not a snapshot of a given log at its offset (`tau
/// replay` exit 8). Each variant names the check and both values.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum JoinError {
    /// The header's offset is past the end of the log.
    #[error("seq: the snapshot is at {seq}, the log has {len} entries")]
    BeyondEnd {
        /// The header's `seq`.
        seq: u64,
        /// The log's length.
        len: usize,
    },
    /// The digest of the log's first `seq` lines is not the header's.
    #[error(
        "prefix: the snapshot says {expected}, the log's first {seq} entries digest to {found}"
    )]
    Prefix {
        /// The header's `seq`.
        seq: u64,
        /// The header's `prefix`.
        expected: String,
        /// The digest this build computed over the log.
        found: String,
    },
    /// The state on line two does not hash to what the header claims.
    #[error("state: the snapshot says {expected}, the state re-hashes to {found}")]
    StateHash {
        /// The header's `state`.
        expected: String,
        /// This build's hash of the deserialized state.
        found: StateHash,
    },
    /// The state's own offset is not the header's.
    #[error("next_seq: the snapshot header says seq {header}, the state says next_seq {body}")]
    NextSeq {
        /// The header's `seq`.
        header: u64,
        /// The state's `next_seq`.
        body: u64,
    },
}

impl State {
    /// A snapshot of this state, bound to the log whose first
    /// [`next_seq`](State::next_seq) entries digest to `prefix`.
    ///
    /// The header is this build's: its `ABI`, its [`FOLD`], and the hash it
    /// computes for the state. `prefix` comes from [`Log::prefix`] or
    /// [`Log::prefix_at`] on the log the state was folded from.
    #[must_use]
    pub fn snapshot(&self, prefix: [u8; 32]) -> Snapshot {
        let header = SnapshotHeader {
            magic: SnapshotHeader::MAGIC,
            abi: ABI,
            fold: FOLD,
            seq: self.next_seq().get(),
            prefix: hex(&prefix),
            state: self.hash().to_string(),
        };
        Snapshot {
            header,
            state: self.clone(),
        }
    }
}

impl Snapshot {
    /// The header, as written or as read.
    #[must_use]
    pub fn header(&self) -> &SnapshotHeader {
        &self.header
    }

    /// The state on line two, before any join check. Prefer [`Snapshot::join`].
    #[must_use]
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Writes the two lines to `w`.
    ///
    /// # Errors
    ///
    /// [`SnapshotError::Io`] on a failed write.
    pub fn write_to<W: Write>(&self, mut w: W) -> Result<(), SnapshotError> {
        write_line(&mut w, &self.header)?;
        write_line(&mut w, &self.state)?;
        Ok(())
    }

    /// Reads a snapshot this build can use.
    ///
    /// Checks the header — magic, `abi <= ABI`, `fold == FOLD` — before
    /// touching the body, so a snapshot from another build is refused
    /// without parsing a state that may be megabytes.
    ///
    /// # Errors
    ///
    /// A [`SnapshotError`] naming the field and both values; the body is
    /// parsed only after every header check passes.
    pub fn read_from<R: BufRead>(r: R) -> Result<Self, SnapshotError> {
        let mut lines = r.lines();
        let first = lines.next().ok_or(SnapshotError::MissingHeader)??;
        let header: SnapshotHeader =
            serde_json::from_str(&first).map_err(SnapshotError::MalformedHeader)?;
        if header.magic != SnapshotHeader::MAGIC {
            return Err(SnapshotError::Magic {
                found: header.magic,
                expected: SnapshotHeader::MAGIC,
            });
        }
        if header.abi > ABI {
            return Err(SnapshotError::Abi {
                found: header.abi,
                reads: ABI,
            });
        }
        if header.fold != FOLD {
            return Err(SnapshotError::Fold {
                found: header.fold,
                expected: FOLD,
            });
        }
        let second = lines
            .find(|l| l.as_ref().is_ok_and(|l| !l.trim().is_empty()) || l.is_err())
            .ok_or(SnapshotError::MissingState)??;
        let state: State = serde_json::from_str(&second).map_err(SnapshotError::MalformedState)?;
        Ok(Self { header, state })
    }

    /// Checks that this is a snapshot of `log` at the header's offset, and
    /// hands back the state positioned to fold `log.entries()[seq..]`.
    ///
    /// # Errors
    ///
    /// The first [`JoinError`] in the order the module docs give: offset,
    /// prefix digest, state re-hash, `next_seq`.
    pub fn join(self, log: &Log) -> Result<State, JoinError> {
        let seq = self.header.seq;
        let k = usize::try_from(seq)
            .ok()
            .filter(|k| *k <= log.len())
            .ok_or(JoinError::BeyondEnd {
                seq,
                len: log.len(),
            })?;
        let found = log
            .prefix_at(k)
            .map(|d| hex(&d))
            .ok_or(JoinError::BeyondEnd {
                seq,
                len: log.len(),
            })?;
        if found != self.header.prefix {
            return Err(JoinError::Prefix {
                seq,
                expected: self.header.prefix,
                found,
            });
        }
        let rehash = self.state.hash();
        if rehash.to_string() != self.header.state {
            return Err(JoinError::StateHash {
                expected: self.header.state,
                found: rehash,
            });
        }
        let body = self.state.next_seq().get();
        if body != seq {
            return Err(JoinError::NextSeq { header: seq, body });
        }
        Ok(self.state)
    }
}

fn write_line<W: Write, T: serde::Serialize>(w: &mut W, value: &T) -> Result<(), SnapshotError> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()?;
    Ok(())
}

/// Lowercase hex, the form both digests take in the header.
fn hex(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in digest {
        // `write!` to a String cannot fail; the fallback keeps the kernel
        // free of `unwrap` for a branch that never runs.
        fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}")).unwrap_or_default();
    }
    out
}
