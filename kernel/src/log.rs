//! The append-only log — the kernel's source of truth.
//!
//! The log is the kernel; everything else is cache (ADR-0003). Every effect in
//! the system is an [`Entry`] appended here *before* it is applied to state,
//! and state at any point is a pure fold over the entries up to it. A behavior
//! that works without an entry is a replay bug.
//!
//! # Framing
//!
//! A log is a [`LogHeader`] followed by entries, one JSON document per line.
//! Newline-delimited JSON is deliberately unclever: it is greppable, diffable,
//! appendable with nothing more than `>>`, and a truncated file is readable up
//! to the truncation. The header lets a reader refuse a log it cannot
//! understand without parsing a single entry.
//!
//! # What is frozen
//!
//! [`LogHeader`], the [`Msg`](crate::abi::Msg) envelopes inside entries, and
//! — since ADR-0010 — the [`Entry`] enum around them and the line framing
//! described above. All of them live under `kernel/src/abi/`, with a snapshot
//! per entry kind; `Entry` is re-exported here so `tau_kernel::log::Entry`
//! still resolves. What stays in this file is behaviour, not shape: the
//! [`Log`] that implements the framing may change, the framing may not.
//! Fixtures in the determinism corpus pin the *state hash* of a fold, which
//! is the second tripwire behind the snapshots.

use core::fmt;
use std::io::{self, BufRead, Write};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::abi::LogHeader;

pub use crate::abi::Entry;

/// Why a log could not be written or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LogError {
    /// The underlying writer or reader failed.
    #[error("log i/o failed")]
    Io(#[from] io::Error),
    /// The input ended before a header.
    #[error("log has no header")]
    MissingHeader,
    /// The first line was not a header.
    #[error("log header is malformed")]
    MalformedHeader(#[source] serde_json::Error),
    /// The header is one this build cannot read: wrong magic or a newer ABI.
    #[error("log header is not readable by this build: {header:?}")]
    Unreadable {
        /// The offending header.
        header: LogHeader,
    },
    /// An entry line did not parse.
    #[error("log entry on line {line} is malformed")]
    MalformedEntry {
        /// 1-based line number in the input.
        line: usize,
        /// The parse failure.
        #[source]
        source: serde_json::Error,
    },
}

/// An append-only sequence of entries, optionally mirrored to a sink.
///
/// Entries live in memory for the reducer and the `recv` path; when a sink is
/// attached, every entry is written through *before* [`Log::append`] returns,
/// which is what lets the kernel apply an entry only after it exists.
///
/// # The prefix digest
///
/// The log keeps a running sha256 over its own bytes as [`Log::write_to`]
/// writes them — the header line, then each entry, each followed by `\n` —
/// updated on every append and rebuilt on read. A snapshot carries the digest
/// at the offset it was taken (ADR-0011 §1), and that is what binds it to
/// *this* log rather than to another whose ids happen to line up. It is
/// defined over the re-serialization rather than the file so a log stored
/// with different whitespace still matches; for a log tau wrote the two are
/// the same bytes.
pub struct Log {
    header: LogHeader,
    entries: Vec<Entry>,
    sink: Option<Box<dyn Write + Send>>,
    /// sha256 over the header line and every entry line so far.
    prefix: Sha256,
}

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Log")
            .field("header", &self.header)
            .field("entries", &self.entries.len())
            .field("sink", &self.sink.is_some())
            .finish()
    }
}

impl Log {
    /// A log that lives only in memory.
    #[must_use]
    pub fn in_memory() -> Self {
        let header = LogHeader::current();
        Self {
            prefix: digest_of(&header),
            header,
            entries: Vec::new(),
            sink: None,
        }
    }

    /// A log written through to `sink`. The header is written immediately.
    ///
    /// # Errors
    ///
    /// [`LogError::Io`] if the header cannot be written.
    pub fn with_sink<W: Write + Send + 'static>(mut sink: W) -> Result<Self, LogError> {
        let header = LogHeader::current();
        write_line(&mut sink, &header)?;
        Ok(Self {
            prefix: digest_of(&header),
            header,
            entries: Vec::new(),
            sink: Some(Box::new(sink)),
        })
    }

    /// The header this log was written under.
    #[must_use]
    pub fn header(&self) -> LogHeader {
        self.header
    }

    /// Every entry, in order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many entries have been appended.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been appended.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends an entry, writing it through to the sink first.
    ///
    /// # Errors
    ///
    /// [`LogError::Io`] if the sink rejects the write, in which case the entry
    /// is *not* retained in memory either: an entry the sink never saw would
    /// make the in-memory log and the durable one disagree.
    pub fn append(&mut self, entry: Entry) -> Result<(), LogError> {
        if let Some(sink) = self.sink.as_mut() {
            write_line(sink, &entry)?;
        }
        feed(&mut self.prefix, &entry);
        self.entries.push(entry);
        Ok(())
    }

    /// The prefix digest over everything appended so far: sha256 of what
    /// [`Log::write_to`] would write now.
    #[must_use]
    pub fn prefix(&self) -> [u8; 32] {
        self.prefix.clone().finalize().into()
    }

    /// The prefix digest as it stood after the first `k` entries, or `None`
    /// if the log is shorter than `k`.
    ///
    /// `k == len` is the running digest; anything shorter is rebuilt from
    /// the entries, which is what makes the digest reproducible from a parsed
    /// log rather than only from a written one.
    #[must_use]
    pub fn prefix_at(&self, k: usize) -> Option<[u8; 32]> {
        if k == self.entries.len() {
            return Some(self.prefix());
        }
        let head = self.entries.get(..k)?;
        let mut hasher = digest_of(&self.header);
        for entry in head {
            feed(&mut hasher, entry);
        }
        Some(hasher.finalize().into())
    }

    /// Writes the whole log — header and entries — to `w`.
    ///
    /// # Errors
    ///
    /// [`LogError::Io`] on a failed write.
    pub fn write_to<W: Write>(&self, mut w: W) -> Result<(), LogError> {
        write_line(&mut w, &self.header)?;
        for entry in &self.entries {
            write_line(&mut w, entry)?;
        }
        Ok(())
    }

    /// Reads a log back. The result has no sink.
    ///
    /// # Errors
    ///
    /// [`LogError::Unreadable`] if the header's magic is wrong or its ABI is
    /// newer than this build's; [`LogError::MalformedEntry`] on a bad line.
    pub fn read_from<R: BufRead>(r: R) -> Result<Self, LogError> {
        let mut lines = r.lines();
        let first = lines.next().ok_or(LogError::MissingHeader)??;
        let header: LogHeader = serde_json::from_str(&first).map_err(LogError::MalformedHeader)?;
        if !header.is_readable() {
            return Err(LogError::Unreadable { header });
        }
        let mut prefix = digest_of(&header);
        let mut entries = Vec::new();
        for (idx, line) in lines.enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry =
                serde_json::from_str(&line).map_err(|source| LogError::MalformedEntry {
                    line: idx.saturating_add(2),
                    source,
                })?;
            feed(&mut prefix, &entry);
            entries.push(entry);
        }
        Ok(Self {
            header,
            entries,
            sink: None,
            prefix,
        })
    }
}

/// One line as [`Log::write_to`] writes it: the JSON document and a `\n`.
fn line_of<T: Serialize>(value: &T) -> Result<Vec<u8>, io::Error> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    Ok(line)
}

fn write_line<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), LogError> {
    w.write_all(&line_of(value)?)?;
    w.flush()?;
    Ok(())
}

/// A hasher primed with the header line.
fn digest_of(header: &LogHeader) -> Sha256 {
    let mut hasher = Sha256::new();
    feed(&mut hasher, header);
    hasher
}

/// Adds one line to the running prefix digest.
fn feed<T: Serialize>(hasher: &mut Sha256, value: &T) {
    // Serializing a frozen type cannot fail; the fallback keeps the kernel
    // free of `unwrap`, and a line that could not be written would not
    // have reached a file either.
    hasher.update(line_of(value).unwrap_or_default());
}
