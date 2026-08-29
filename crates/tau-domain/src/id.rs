//! Identifier newtypes used across the `tau-domain` surface.
//!
//! `PackageName` and `AgentId` are validating ASCII kebab-case identifiers.
//! `AgentInstanceId` and `MessageId` are UUID v7-based opaque identifiers.

use core::fmt;
use core::str::FromStr;

use alloc::borrow::ToOwned;
use alloc::string::String;

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
    use alloc::string::ToString;

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

/// An agent identifier: one or more `/`-separated ASCII segments, 1..=64
/// bytes in total.
///
/// ```text
/// AgentId  := segment ( '/' segment )*        1..=64 bytes total
/// segment  := [a-z0-9] [a-z0-9_-]*
/// ```
///
/// The `/` separator makes an agent's authored location its identity:
/// `[dirs]` names an agent by its path relative to the agents root, so
/// `agents/review/strict.md` is agent `review/strict` and
/// `agents/perf/strict.md` is `perf/strict` — distinct, never ambiguous
/// (ADR-0069). Sanitizing `/` away would collapse them, which is why this
/// grammar admits the separator rather than folding it (ADR-0070).
///
/// This is deliberately **not** [`PackageName`]'s grammar: a package name is
/// a registry identity, not a project-local namespace, so it admits neither
/// `/` nor `_` and still requires a leading letter.
///
/// # Example
///
/// ```
/// use tau_domain::AgentId;
/// use std::str::FromStr;
///
/// let id = AgentId::from_str("researcher").unwrap();
/// assert_eq!(id.as_str(), "researcher");
///
/// // Namespaced names round-trip verbatim.
/// let nested = AgentId::from_str("review/strict").unwrap();
/// assert_eq!(nested.as_str(), "review/strict");
///
/// assert!(AgentId::from_str("my_agent").is_ok());
/// assert!(AgentId::from_str("review//strict").is_err());
/// assert!(AgentId::from_str("-leading-dash").is_err());
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
        // Validate segment by segment. The total-length cap above is the
        // only bound on nesting depth: a second, independent depth limit
        // would be a third grammar to keep in sync, which is the failure
        // this type exists to avoid (ADR-0070).
        //
        // `offset` tracks the segment's start in the *whole* input so the
        // byte positions in `InvalidCharacter` / `EmptySegment` point into
        // the id the caller passed in, not into an interior slice of it.
        let mut offset = 0usize;
        for seg in s.split('/') {
            let mut chars = seg.char_indices();
            let Some((_, first)) = chars.next() else {
                return Err(AgentIdError::EmptySegment { pos: offset });
            };
            if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
                return Err(AgentIdError::InvalidLeadingCharacter { ch: first });
            }
            for (pos, ch) in chars {
                if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_') {
                    return Err(AgentIdError::InvalidCharacter {
                        ch,
                        pos: offset + pos,
                    });
                }
            }
            // + 1 for the `/` that followed this segment. Overshoots by one
            // past the final segment, which is never read.
            offset += seg.len() + 1;
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
            assert!(AgentId::from_str(name).is_ok(), "should accept {name:?}");
        }
    }

    /// The three shapes ADR-0070 added: `/`-separated namespaces (what
    /// `[dirs]` derives from a nested path), `_` inside a segment, and a
    /// digit-leading segment.
    #[test]
    fn accepts_widened_shapes() {
        let max_len = alloc::format!("a/{}", "b".repeat(62));
        assert_eq!(max_len.len(), 64);
        for name in [
            "review/strict",
            "perf/strict",
            "a/b/c",
            "my_agent",
            "review/strict_v2",
            "2fa",
            "2fa/check",
            "a1/b2-c3_d4",
            max_len.as_str(),
        ] {
            assert!(AgentId::from_str(name).is_ok(), "should accept {name:?}");
        }
    }

    /// Distinct paths must stay distinct names — the property that rules out
    /// sanitizing `/` into `-` for authored ids (ADR-0070).
    #[test]
    fn nested_names_do_not_collapse() {
        let a = AgentId::from_str("review/strict").unwrap();
        let b = AgentId::from_str("perf/strict").unwrap();
        let c = AgentId::from_str("review-strict").unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.as_str(), "review/strict");
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

    /// A leading `-` stays illegal so `tau build --agent <id>` can never be
    /// ambiguous with a flag. `1agent` is *accepted* now — the leading rule
    /// relaxed to `[a-z0-9]` so the domain grammar covers every segment the
    /// `[dirs]` scanner can produce (ADR-0070).
    #[test]
    fn rejects_invalid_leading() {
        assert_eq!(
            AgentId::from_str("-agent"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: '-' }),
        );
        assert_eq!(
            AgentId::from_str("_agent"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: '_' }),
        );
        assert_eq!(
            AgentId::from_str("Agent"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: 'A' }),
        );
        assert!(AgentId::from_str("1agent").is_ok());
    }

    /// The leading rule applies per segment, not just to the whole id.
    #[test]
    fn rejects_invalid_leading_in_a_later_segment() {
        assert_eq!(
            AgentId::from_str("review/-strict"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: '-' }),
        );
        assert_eq!(
            AgentId::from_str("review/Strict"),
            Err(AgentIdError::InvalidLeadingCharacter { ch: 'S' }),
        );
    }

    /// `_` is legal inside a segment now; the reported byte position is an
    /// offset into the whole id, not into the segment it was found in.
    #[test]
    fn rejects_invalid_mid_char() {
        assert!(AgentId::from_str("agent_x").is_ok());
        assert!(matches!(
            AgentId::from_str("agent.x"),
            Err(AgentIdError::InvalidCharacter { ch: '.', pos: 5 }),
        ));
        assert!(matches!(
            AgentId::from_str("agentX"),
            Err(AgentIdError::InvalidCharacter { ch: 'X', pos: 5 }),
        ));
        assert!(matches!(
            AgentId::from_str("review/str ict"),
            Err(AgentIdError::InvalidCharacter { ch: ' ', pos: 10 }),
        ));
    }

    /// A leading, trailing, or doubled `/` leaves an empty segment. `"/"`
    /// is two empty segments and reports the first.
    #[test]
    fn rejects_empty_segments() {
        assert_eq!(
            AgentId::from_str("/review"),
            Err(AgentIdError::EmptySegment { pos: 0 }),
        );
        assert_eq!(
            AgentId::from_str("review/"),
            Err(AgentIdError::EmptySegment { pos: 7 }),
        );
        assert_eq!(
            AgentId::from_str("review//strict"),
            Err(AgentIdError::EmptySegment { pos: 7 }),
        );
        assert_eq!(
            AgentId::from_str("/"),
            Err(AgentIdError::EmptySegment { pos: 0 }),
        );
    }

    /// The 64-byte cap is the only bound on nesting depth (ADR-0070), so a
    /// deep path fails as `TooLong` rather than via a separate depth limit.
    #[test]
    fn rejects_too_deep_via_the_length_cap() {
        let deep = alloc::vec!["seg"; 17].join("/");
        assert_eq!(deep.len(), 67);
        assert_eq!(
            AgentId::from_str(&deep),
            Err(AgentIdError::TooLong { max: 64, got: 67 }),
        );
    }

    /// `AgentId` and `PackageName` are no longer the same grammar. The pair
    /// is asserted together by `dynamic.rs::assert_legal`, so the divergence
    /// is pinned here rather than discovered there.
    #[test]
    fn diverges_from_package_name() {
        for widened in ["review/strict", "my_agent", "2fa"] {
            assert!(AgentId::from_str(widened).is_ok(), "{widened}");
            assert!(
                PackageName::from_str(widened).is_err(),
                "PackageName must stay narrow: {widened}"
            );
        }
        // Everything PackageName accepts, AgentId still accepts.
        for shared in ["a", "fs-tools", "abc-123"] {
            assert!(PackageName::from_str(shared).is_ok(), "{shared}");
            assert!(AgentId::from_str(shared).is_ok(), "{shared}");
        }
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
    /// Generate a fresh UUID v7 from the ambient system clock + RNG.
    ///
    /// Host-only (`std`). The no_std kernel mints instance ids
    /// deterministically via [`AgentInstanceId::from_parts`], fed by the
    /// `Clock`/`RandomSource` ports.
    #[cfg(feature = "std")]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    /// Mint a UUID v7 from an explicit Unix-millisecond timestamp and 10
    /// random bytes — the no_std-safe constructor the kernel feeds from
    /// its `Clock`/`RandomSource` ports.
    pub fn from_parts(unix_millis: u64, random: [u8; 10]) -> Self {
        Self(uuid::Builder::from_unix_timestamp_millis(unix_millis, &random).into_uuid())
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

#[cfg(feature = "std")]
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
    /// Generate a fresh UUID v7 from the ambient system clock + RNG.
    ///
    /// Host-only (`std`): reads `SystemTime` + `getrandom`. The no_std
    /// kernel mints ids deterministically via [`MessageId::from_parts`],
    /// fed by the `Clock`/`RandomSource` ports — see
    /// `tau_runtime_core::ids::message_id`.
    #[cfg(feature = "std")]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    /// Mint a UUID v7 from an explicit Unix-millisecond timestamp and 10
    /// random bytes — no ambient clock or RNG. This is the no_std-safe
    /// constructor the kernel routes through its `Clock`/`RandomSource`
    /// ports so ids are reproducible under conformance.
    pub fn from_parts(unix_millis: u64, random: [u8; 10]) -> Self {
        Self(uuid::Builder::from_unix_timestamp_millis(unix_millis, &random).into_uuid())
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

#[cfg(feature = "std")]
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

// `AgentInstanceId`/`MessageId` hand-roll `Serialize`/`Deserialize` as the
// inner `uuid::Uuid` (see `uuid_id_serde` above), which serializes as a
// hyphenated UUID string. `uuid::Uuid` is foreign, so we can't derive
// `JsonSchema` on it here (orphan rule) — mirror the wire format by hand
// instead of deriving on the newtype (which would need `uuid::Uuid:
// JsonSchema` anyway).
#[cfg(feature = "schema")]
mod uuid_id_schema {
    use super::{AgentInstanceId, MessageId};

    impl schemars::JsonSchema for AgentInstanceId {
        fn schema_name() -> alloc::borrow::Cow<'static, str> {
            "AgentInstanceId".into()
        }

        fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
            schemars::json_schema!({
                "type": "string",
                "format": "uuid"
            })
        }
    }

    impl schemars::JsonSchema for MessageId {
        fn schema_name() -> alloc::borrow::Cow<'static, str> {
            "MessageId".into()
        }

        fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
            schemars::json_schema!({
                "type": "string",
                "format": "uuid"
            })
        }
    }
}

#[cfg(test)]
mod uuid_id_tests {
    use super::*;
    use alloc::string::ToString;

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
