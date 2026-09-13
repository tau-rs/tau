//! A validated ASCII name, used wherever the ABI needs a stable human-readable
//! identifier (driver endpoints, custom budget dimensions).
//!
//! # Why a type and not a `String`
//!
//! tau v1 shipped `AgentId` with a validated grammar and two *other* crates
//! with an unvalidated one for the same values. The disagreement was invisible
//! until a feature made the build pipeline exercise it, at which point it
//! produced an exit-2 build failure, a panic on user input, and — worst — a
//! silent mis-attribution, where an id the grammar rejected collapsed into a
//! phantom entity instead of failing. The lesson is not "validate harder"; it
//! is *validate once, at the ABI, and make the invalid state unrepresentable
//! everywhere downstream*.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Maximum length of a [`Name`], in bytes.
///
/// Names appear in every log entry that mentions a driver; a bound keeps entry
/// size predictable and rules out unbounded allocation from a hostile manifest.
pub const NAME_MAX_LEN: usize = 63;

/// A validated ASCII name: `[a-z][a-z0-9_-]{0,62}`.
///
/// Lowercase-only by construction, so two names that differ in case cannot
/// exist and no part of the system has to decide whether comparison folds case.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

/// Why a string was rejected as a [`Name`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NameError {
    /// The string was empty.
    Empty,
    /// The string exceeded [`NAME_MAX_LEN`] bytes.
    TooLong {
        /// The offending length, in bytes.
        len: usize,
    },
    /// The first character was not `[a-z]`.
    BadLeadingChar {
        /// The offending character.
        found: char,
    },
    /// A character outside `[a-z0-9_-]` appeared after the first.
    BadChar {
        /// The offending character.
        found: char,
        /// Its byte offset within the string.
        at: usize,
    },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "name is empty"),
            Self::TooLong { len } => {
                write!(f, "name is {len} bytes, maximum is {NAME_MAX_LEN}")
            }
            Self::BadLeadingChar { found } => {
                write!(f, "name must start with [a-z], found {found:?}")
            }
            Self::BadChar { found, at } => {
                write!(
                    f,
                    "name contains {found:?} at byte {at}, allowed: [a-z0-9_-]"
                )
            }
        }
    }
}

impl std::error::Error for NameError {}

impl Name {
    /// Validates `s` and constructs a [`Name`].
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] describing the first rule the input violates.
    pub fn new(s: &str) -> Result<Self, NameError> {
        if s.is_empty() {
            return Err(NameError::Empty);
        }
        if s.len() > NAME_MAX_LEN {
            return Err(NameError::TooLong { len: s.len() });
        }
        let mut chars = s.char_indices();
        // `s` is non-empty, so the first `next()` is always `Some`; the match
        // avoids an `unwrap` (denied workspace-wide in kernel code).
        match chars.next() {
            Some((_, c)) if c.is_ascii_lowercase() => {}
            Some((_, found)) => return Err(NameError::BadLeadingChar { found }),
            None => return Err(NameError::Empty),
        }
        for (at, found) in chars {
            let ok = found.is_ascii_lowercase()
                || found.is_ascii_digit()
                || found == '-'
                || found == '_';
            if !ok {
                return Err(NameError::BadChar { found, at });
            }
        }
        Ok(Self(s.to_owned()))
    }

    /// Borrows the name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Name {
    type Err = NameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}
