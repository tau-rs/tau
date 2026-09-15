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
//! # What is and is not frozen
//!
//! [`LogHeader`] and the [`Msg`] envelopes inside entries are ABI (see
//! `kernel/src/abi/`). The [`Entry`] enum around them is **not yet** frozen:
//! it is kernel-internal and may churn until the replay CLI lands (M2), at
//! which point it joins the frozen surface. Fixtures in the determinism corpus
//! pin the *state hash* of a fold, so a churn here shows up as a loud fixture
//! failure rather than a silent divergence.

use core::fmt;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

use crate::abi::{
    AgentId, BlobRef, Budget, Capability, DriverId, HookId, LogHeader, Msg, Namespace, Seq,
};
use crate::hook::{FailureMode, HookPoint, HookSource, Roll};

/// One effect, as recorded.
///
/// Each variant carries the kernel-allocated ids it introduces (`agent`, `cap`,
/// the `corr` inside a `msg`), so the reducer can *verify* an allocation on
/// replay rather than re-deriving it and hoping it matches. What a variant
/// does *not* carry is anything the reducer can derive: `Cancelled` names the
/// subtree's root, not its members, and a hard abort is not an entry at all
/// but a consequence of applying the `Tick` that reaches the deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum Entry {
    /// A driver endpoint was registered at boot and a capability minted for it.
    DriverRegistered {
        /// Log position.
        seq: Seq,
        /// The driver.
        driver: DriverId,
        /// The capability that names it.
        cap: Capability,
        /// The most one request to this driver may cost, as declared by the
        /// harness. Every `Sent` through `cap` reserves this much from the
        /// sender before delivery; the `Replied` settles against it.
        ceiling: Budget,
    },
    /// An agent was created.
    Spawned {
        /// Log position.
        seq: Seq,
        /// The spawning agent, or `None` for the root, which the harness spawns.
        parent: Option<AgentId>,
        /// The new agent.
        agent: AgentId,
        /// Its birth namespace — a subset of the parent's.
        ns: Namespace,
        /// Its grant — carved atomically from the parent's.
        budget: Budget,
    },
    /// An agent sent a request through a capability it holds.
    Sent {
        /// The envelope. `msg.seq` is this entry's position.
        msg: Msg,
        /// The capability the sender used; the reducer resolves it to an
        /// endpoint, and the log keeps the evidence of authority.
        via: Capability,
    },
    /// A driver answered a request.
    Replied {
        /// The envelope, carrying the driver's consumption report.
        msg: Msg,
        /// The owner of the correlation — where the reply is delivered.
        to: AgentId,
    },
    /// A `recv` resolved: one message left an agent's mailbox.
    ///
    /// The filter is not logged, only the outcome. Replay does not re-evaluate
    /// user intent; it re-applies what happened.
    Resolved {
        /// Log position.
        seq: Seq,
        /// The receiving agent.
        agent: AgentId,
        /// The position of the message that matched.
        matched: Seq,
    },
    /// An agent's last act.
    Exited {
        /// Log position.
        seq: Seq,
        /// The agent.
        agent: AgentId,
        /// Its result, held until claimed.
        result: BlobRef,
    },
    /// A stored outcome was claimed: by the parent through `wait`, or by the
    /// harness for a result the tree left behind.
    Claimed {
        /// Log position.
        seq: Seq,
        /// Whose outcome.
        agent: AgentId,
        /// The claimant, or `None` for the harness.
        by: Option<AgentId>,
    },
    /// An agent's subtree was cancelled: phase one of `cancel`, the atomic
    /// freeze. Applying this entry freezes every live agent under `agent`,
    /// delivers each a `Notice` from `by` carrying `reason`, and sets each one's
    /// deadline to the current clock reading plus `grace`.
    Cancelled {
        /// Log position.
        seq: Seq,
        /// The canceller, or `None` for the harness.
        by: Option<AgentId>,
        /// The root of the cancelled subtree.
        agent: AgentId,
        /// How many clock units the subtree has to exit on its own.
        grace: u64,
        /// The payload of the notice each frozen agent receives.
        reason: BlobRef,
    },
    /// The clock advanced. Time enters the system only this way (ADR-0003):
    /// applying a tick is when wall budgets are charged and cancel deadlines
    /// are enforced.
    Tick {
        /// Log position.
        seq: Seq,
        /// The clock reading. Non-decreasing across a log.
        now: u64,
    },
    /// A hook program was installed at boot, before any agent existed
    /// (ADR-0008 §4). The registry is these entries, at the head of the log.
    Attached {
        /// Log position.
        seq: Seq,
        /// The hook, allocated by the kernel at `attach`.
        hook: HookId,
        /// Where it is consulted.
        point: HookPoint,
        /// What a verdict it fails to produce counts as.
        failure: FailureMode,
        /// What it is: a native by name, a rule by source.
        program: HookSource,
    },
    /// The hooks at `point` were consulted about `subject`, and this is what
    /// each answered, in `HookId` order, stopping at the first `Deny`
    /// (ADR-0008 §3). At a pre point this precedes the governed entry — or
    /// nothing, if a hook denied it; at an on point it follows the entry that
    /// caused the moment. A point with no hook attached writes nothing.
    ///
    /// The fold confirms the roll call and never runs a program: the effect
    /// of a `Deny` is the absence of the next entry, and the effect of an
    /// `Emit` is the `Emitted` entry that follows.
    Verdicts {
        /// Log position.
        seq: Seq,
        /// The point consulted.
        point: HookPoint,
        /// The agent the moment was about.
        subject: AgentId,
        /// Who answered what.
        roll: Roll,
    },
    /// A hook's `Emit` verdict produced a notice. Its own entry, because a
    /// notice is a message and every message has its own position: two
    /// notices sharing a `seq` in one mailbox would make a `Resolved`
    /// ambiguous. Delivered to `to` if live; dead-lettered otherwise.
    Emitted {
        /// The hook that emitted it.
        hook: HookId,
        /// The recipient.
        to: AgentId,
        /// The envelope, from `Endpoint::Hook`. `msg.seq` is this entry's
        /// position.
        msg: Msg,
    },
}

impl Entry {
    /// This entry's position in the log.
    #[must_use]
    pub fn seq(&self) -> Seq {
        match self {
            Self::DriverRegistered { seq, .. }
            | Self::Spawned { seq, .. }
            | Self::Resolved { seq, .. }
            | Self::Exited { seq, .. }
            | Self::Claimed { seq, .. }
            | Self::Cancelled { seq, .. }
            | Self::Tick { seq, .. }
            | Self::Attached { seq, .. }
            | Self::Verdicts { seq, .. } => *seq,
            Self::Sent { msg, .. } | Self::Replied { msg, .. } | Self::Emitted { msg, .. } => {
                msg.seq
            }
        }
    }
}

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
pub struct Log {
    header: LogHeader,
    entries: Vec<Entry>,
    sink: Option<Box<dyn Write + Send>>,
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
        Self {
            header: LogHeader::current(),
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
        self.entries.push(entry);
        Ok(())
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
        let mut entries = Vec::new();
        for (idx, line) in lines.enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str(&line).map_err(|source| LogError::MalformedEntry {
                line: idx.saturating_add(2),
                source,
            })?;
            entries.push(entry);
        }
        Ok(Self {
            header,
            entries,
            sink: None,
        })
    }
}

fn write_line<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<(), LogError> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()?;
    Ok(())
}
