//! Identifier newtypes used across the `tau-domain` surface.
//!
//! `PackageName` and `AgentId` are validating ASCII kebab-case identifiers.
//! `AgentInstanceId` and `MessageId` are UUID v7-based opaque identifiers.

use std::fmt;
use std::str::FromStr;

use crate::error::{AgentIdError, PackageNameError};

/// A package name. ASCII kebab-case, must start with a lowercase letter,
/// 1..=64 characters, character set `[a-z0-9-]`.
///
/// # Example
///
/// ```
/// use tau_domain::PackageName;
/// use std::str::FromStr;
///
/// let n = PackageName::from_str("fs-tools").unwrap();
/// assert_eq!(n.as_str(), "fs-tools");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageName(String);

impl PackageName {
    /// The maximum permitted length, in bytes (== chars, since ASCII-only).
    pub const MAX_LEN: usize = 64;

    /// View as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PackageName {
    type Err = PackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(PackageNameError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(PackageNameError::TooLong {
                max: Self::MAX_LEN,
                got: s.len(),
            });
        }
        let mut chars = s.char_indices();
        let (_, first) = chars.next().expect("length-checked above");
        if !first.is_ascii_lowercase() {
            return Err(PackageNameError::InvalidLeadingCharacter { ch: first });
        }
        for (pos, ch) in chars {
            if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
                return Err(PackageNameError::InvalidCharacter { ch, pos });
            }
        }
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        let max_len = "x".repeat(64);
        for name in ["a", "fs-tools", "abc-123", max_len.as_str()] {
            assert!(
                PackageName::from_str(name).is_ok(),
                "should accept {name:?}"
            );
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(PackageName::from_str(""), Err(PackageNameError::Empty));
    }

    #[test]
    fn rejects_too_long() {
        let s = "a".repeat(65);
        assert_eq!(
            PackageName::from_str(&s),
            Err(PackageNameError::TooLong { max: 64, got: 65 }),
        );
    }

    #[test]
    fn rejects_invalid_leading() {
        assert_eq!(
            PackageName::from_str("1abc"),
            Err(PackageNameError::InvalidLeadingCharacter { ch: '1' }),
        );
        assert_eq!(
            PackageName::from_str("-abc"),
            Err(PackageNameError::InvalidLeadingCharacter { ch: '-' }),
        );
        assert_eq!(
            PackageName::from_str("Abc"),
            Err(PackageNameError::InvalidLeadingCharacter { ch: 'A' }),
        );
    }

    #[test]
    fn rejects_invalid_mid_char() {
        assert!(matches!(
            PackageName::from_str("abc_def"),
            Err(PackageNameError::InvalidCharacter { ch: '_', pos: 3 }),
        ));
        assert!(matches!(
            PackageName::from_str("abcDef"),
            Err(PackageNameError::InvalidCharacter { ch: 'D', pos: 3 }),
        ));
    }

    #[test]
    fn display_round_trip() {
        let n = PackageName::from_str("fs-tools").unwrap();
        assert_eq!(n.to_string(), "fs-tools");
    }
}

/// An agent identifier. ASCII kebab-case, must start with a lowercase letter,
/// 1..=64 characters, character set `[a-z0-9-]`.
///
/// Same grammar as [`PackageName`]; separate type for clarity at call sites.
///
/// # Example
///
/// ```
/// use tau_domain::AgentId;
/// use std::str::FromStr;
///
/// let id = AgentId::from_str("researcher").unwrap();
/// assert_eq!(id.as_str(), "researcher");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(String);

impl AgentId {
    /// The maximum permitted length, in bytes.
    pub const MAX_LEN: usize = 64;

    /// View as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentId {
    type Err = AgentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(AgentIdError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(AgentIdError::TooLong {
                max: Self::MAX_LEN,
                got: s.len(),
            });
        }
        let mut chars = s.char_indices();
        let (_, first) = chars.next().expect("length-checked above");
        if !first.is_ascii_lowercase() {
            return Err(AgentIdError::InvalidLeadingCharacter { ch: first });
        }
        for (pos, ch) in chars {
            if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
                return Err(AgentIdError::InvalidCharacter { ch, pos });
            }
        }
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod agent_id_tests {
    use super::*;

    #[test]
    fn accepts_valid() {
        for name in ["a", "researcher", "agent-123"] {
            assert!(AgentId::from_str(name).is_ok());
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(AgentId::from_str(""), Err(AgentIdError::Empty));
    }

    #[test]
    fn rejects_too_long() {
        let s = "a".repeat(65);
        assert_eq!(
            AgentId::from_str(&s),
            Err(AgentIdError::TooLong { max: 64, got: 65 }),
        );
    }

    #[test]
    fn rejects_invalid_leading() {
        assert_eq!(
            AgentId::from_str("1agent"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: '1' }),
        );
    }

    #[test]
    fn rejects_invalid_mid_char() {
        assert!(matches!(
            AgentId::from_str("agent_x"),
            Err(AgentIdError::InvalidCharacter { ch: '_', pos: 5 }),
        ));
    }
}

/// A runtime instance identifier for a spawned agent. UUID v7 (monotonic,
/// time-ordered). Two instances of the same `AgentDefinition` share an
/// `AgentId` but differ in `AgentInstanceId`.
///
/// # Example
///
/// ```
/// use tau_domain::AgentInstanceId;
///
/// let a = AgentInstanceId::new();
/// let b = AgentInstanceId::new();
/// assert_ne!(a, b);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentInstanceId(uuid::Uuid);

impl AgentInstanceId {
    /// Generate a fresh UUID v7.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    /// Wrap an existing `Uuid`.
    ///
    /// Useful when deserializing a stored identifier back into the typed
    /// wrapper, or when composing identifiers in cross-crate tests.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{AgentInstanceId, Uuid};
    ///
    /// let u = Uuid::now_v7();
    /// let id = AgentInstanceId::from_uuid(u);
    /// assert_eq!(id.as_uuid(), u);
    /// ```
    pub fn from_uuid(u: uuid::Uuid) -> Self {
        Self(u)
    }

    /// Underlying `Uuid`.
    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for AgentInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AgentInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for AgentInstanceId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<uuid::Uuid>().map(Self)
    }
}

/// A message identifier. UUID v7 (monotonic, time-ordered). Acts as the
/// reply target for `Message.parent_id`.
///
/// # Example
///
/// ```
/// use tau_domain::MessageId;
///
/// let id = MessageId::new();
/// let parsed: MessageId = id.to_string().parse().unwrap();
/// assert_eq!(id, parsed);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageId(uuid::Uuid);

impl MessageId {
    /// Generate a fresh UUID v7.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    /// Wrap an existing `Uuid`.
    ///
    /// Useful when deserializing a stored identifier back into the typed
    /// wrapper, or when composing message chains in cross-crate tests.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_domain::{MessageId, Uuid};
    ///
    /// let u = Uuid::now_v7();
    /// let id = MessageId::from_uuid(u);
    /// assert_eq!(id.as_uuid(), u);
    /// ```
    pub fn from_uuid(u: uuid::Uuid) -> Self {
        Self(u)
    }

    /// Underlying `Uuid`.
    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for MessageId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<uuid::Uuid>().map(Self)
    }
}

#[cfg(feature = "serde")]
mod uuid_id_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for AgentInstanceId {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.0.serialize(s)
        }
    }
    impl<'de> Deserialize<'de> for AgentInstanceId {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            uuid::Uuid::deserialize(d).map(Self)
        }
    }
    impl Serialize for MessageId {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.0.serialize(s)
        }
    }
    impl<'de> Deserialize<'de> for MessageId {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            uuid::Uuid::deserialize(d).map(Self)
        }
    }

    impl Serialize for PackageName {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.0.serialize(s)
        }
    }
    impl<'de> Deserialize<'de> for PackageName {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let raw = String::deserialize(d)?;
            raw.parse::<PackageName>().map_err(serde::de::Error::custom)
        }
    }

    impl Serialize for AgentId {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            self.0.serialize(s)
        }
    }
    impl<'de> Deserialize<'de> for AgentId {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let raw = String::deserialize(d)?;
            raw.parse::<AgentId>().map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod uuid_id_tests {
    use super::*;

    #[test]
    fn agent_instance_round_trips() {
        let a = AgentInstanceId::new();
        let parsed: AgentInstanceId = a.to_string().parse().unwrap();
        assert_eq!(a, parsed);
    }

    #[test]
    fn message_id_round_trips() {
        let m = MessageId::new();
        let parsed: MessageId = m.to_string().parse().unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn fresh_ids_differ() {
        assert_ne!(MessageId::new(), MessageId::new());
        assert_ne!(AgentInstanceId::new(), AgentInstanceId::new());
    }
}
