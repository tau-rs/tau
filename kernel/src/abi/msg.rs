//! The message envelope — the single frozen shape everything travels in.
//!
//! One uniform interface over heterogeneous things, the way everything in Unix
//! is a file. A model completion, a tool result, a cancellation notice, a clock
//! tick, and a hook verdict are all the same struct; what differs is the
//! payload, which the kernel never looks at.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Consumption, Corr, Endpoint, Seq, ABI};

/// A content-addressed reference to a payload.
///
/// # Why payloads are not in the log
///
/// A log entry that inlined a 200 KiB completion would make the log
/// prohibitively expensive to replay, impossible to dedupe, and — the part that
/// actually forces the decision — impossible to erase. With a hash in the
/// envelope and the bytes in a content-addressed store, deletion becomes
/// crypto-shredding: encrypt payloads per subtree, drop the key, and the
/// payloads are gone while the structural log, and therefore replay of
/// everything except the content, still works.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobRef([u8; 32]);

impl BlobRef {
    /// The reference to the empty payload: 32 zero bytes.
    ///
    /// Distinct from "no payload": every message has a payload, and a message
    /// that carries nothing carries the empty blob.
    pub const EMPTY: Self = Self([0; 32]);

    /// Wraps a 32-byte digest.
    ///
    /// Computing the digest is the blob store's job, not the ABI's — the ABI
    /// deliberately does not name a hash function, so changing it later is a
    /// store concern rather than a wire break.
    #[must_use]
    pub const fn from_bytes(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The lowercase hex form, which is also the wire form.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            // `write!` to a String is infallible, but the kernel denies
            // `unwrap`; a manual nibble push avoids the question entirely.
            out.push(hex_nibble(byte >> 4));
            out.push(hex_nibble(byte & 0x0f));
        }
        out
    }

    /// Parses a 64-character lowercase hex digest.
    ///
    /// # Errors
    ///
    /// Returns [`BlobRefError`] if the input is the wrong length or contains a
    /// non-hex character.
    pub fn from_hex(s: &str) -> Result<Self, BlobRefError> {
        if s.len() != 64 {
            return Err(BlobRefError::BadLength { len: s.len() });
        }
        let mut out = [0u8; 32];
        let bytes = s.as_bytes();
        for (i, slot) in out.iter_mut().enumerate() {
            // `bytes` is exactly 64 long and `i < 32`, so both indices are in
            // range; `get` keeps that fact checked rather than asserted.
            let (hi, lo) = match (bytes.get(i * 2), bytes.get(i * 2 + 1)) {
                (Some(&hi), Some(&lo)) => (hi, lo),
                _ => return Err(BlobRefError::BadLength { len: s.len() }),
            };
            *slot = (unhex(hi)? << 4) | unhex(lo)?;
        }
        Ok(Self(out))
    }
}

const fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

fn unhex(c: u8) -> Result<u8, BlobRefError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(BlobRefError::BadChar { found: c as char }),
    }
}

/// Why a string was rejected as a [`BlobRef`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobRefError {
    /// The input was not 64 characters.
    BadLength {
        /// The offending length.
        len: usize,
    },
    /// The input contained a character outside `[0-9a-f]`.
    BadChar {
        /// The offending character.
        found: char,
    },
}

impl fmt::Display for BlobRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLength { len } => write!(f, "blob ref is {len} characters, expected 64"),
            Self::BadChar { found } => write!(f, "blob ref contains {found:?}, expected [0-9a-f]"),
        }
    }
}

impl std::error::Error for BlobRefError {}

impl fmt::Debug for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobRef({})", self.to_hex())
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::from_hex(&raw).map_err(serde::de::Error::custom)
    }
}

/// What kind of traffic a message is.
///
/// Four kinds, and the set is `#[non_exhaustive]` so a fifth can be added
/// additively. `Partial` exists so streaming is an ordinary message rather than
/// a side channel: a streamed completion is a run of `Partial`s sharing a
/// correlation, terminated by a `Reply`. Streaming that bypassed the log would
/// be invisible to replay, which is the definition of a bug here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MsgKind {
    /// Asks for something. Allocates a [`Corr`].
    Request,
    /// The terminal answer to a request.
    Reply,
    /// A non-terminal fragment of an answer.
    Partial,
    /// Unsolicited: a cancellation notice, a hook `Emit`, a clock tick.
    Notice,
}

/// The frozen envelope.
///
/// Every effect in the system is one of these, appended to the log before it is
/// applied. If a behavior works without a log entry, it is a replay bug.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Msg {
    /// The ABI version this envelope was written against.
    ///
    /// Present from the first commit, because the one thing a versioning scheme
    /// cannot be is retrofitted: a log written before the field existed has no
    /// way to say what it is.
    pub abi: u16,
    /// This message's position in the log.
    pub seq: Seq,
    /// Who sent it.
    pub from: Endpoint,
    /// The correlation this message belongs to, if any.
    ///
    /// `None` for a `Notice`, which answers nothing.
    pub corr: Option<Corr>,
    /// What kind of traffic this is.
    pub kind: MsgKind,
    /// What the call cost, as reported by the driver. Present on replies.
    pub consumed: Option<Consumption>,
    /// A content-addressed reference to the payload.
    pub payload: BlobRef,
}

impl Msg {
    /// Builds an envelope stamped with the current [`ABI`].
    #[must_use]
    pub fn new(seq: Seq, from: Endpoint, kind: MsgKind, payload: BlobRef) -> Self {
        Self {
            abi: ABI,
            seq,
            from,
            corr: None,
            kind,
            consumed: None,
            payload,
        }
    }

    /// Attaches a correlation.
    #[must_use]
    pub fn with_corr(mut self, corr: Corr) -> Self {
        self.corr = Some(corr);
        self
    }

    /// Attaches a driver's consumption report.
    #[must_use]
    pub fn with_consumption(mut self, consumed: Consumption) -> Self {
        self.consumed = Some(consumed);
        self
    }
}

/// The header at the front of a log file.
///
/// Carries the ABI version independently of the envelopes inside it, so a
/// reader can refuse a log it cannot understand without parsing a single entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogHeader {
    /// Magic bytes identifying a tau log: `TAU\0`.
    pub magic: [u8; 4],
    /// The ABI version every entry in this log was written against.
    pub abi: u16,
}

impl LogHeader {
    /// The magic bytes at the start of every tau log.
    pub const MAGIC: [u8; 4] = *b"TAU\0";

    /// A header for a log being written now.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            magic: Self::MAGIC,
            abi: ABI,
        }
    }

    /// Whether this header is one this build can read.
    ///
    /// Deliberately not `==`: the whole point of an additive ABI is that a
    /// newer reader accepts an older log.
    #[must_use]
    // `ABI` is 0 today, so `<=` is trivially true and clippy says so. The
    // comparison is the *policy*, not an accident of the current value: at ABI
    // 1 it starts rejecting logs from the future while still accepting logs
    // from the past. Rewriting it to `==` to satisfy the lint would silently
    // make the reader version-exact, which is the opposite of the rule.
    #[allow(clippy::absurd_extreme_comparisons)]
    pub const fn is_readable(&self) -> bool {
        matches!(self.magic, Self::MAGIC) && self.abi <= ABI
    }
}

impl Default for LogHeader {
    fn default() -> Self {
        Self::current()
    }
}
