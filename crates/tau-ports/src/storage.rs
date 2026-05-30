//! Storage port — `kind = "storage"` plugin contracts.
//!
//! [`Namespace`] and [`Key`] are validating newtypes used by the
//! [`Storage`] trait. Per the G8 scope-handling intent,
//! tau-runtime is responsible for composing scope information into a
//! `Namespace`; storage plugins treat the namespace as opaque and never
//! parse or interpret it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::error::{KeyError, NamespaceError, StorageError};

/// Validated namespace identifier. Carries scope information composed
/// by tau-runtime; opaque to Storage plugins.
///
/// Validation rules:
/// - Non-empty.
/// - At most [`Namespace::MAX_LEN`] bytes.
/// - No NUL bytes (`\0`) or ASCII control characters
///   (U+0000..=U+001F, U+007F).
///
/// # Example
///
/// ```
/// use tau_ports::storage::Namespace;
///
/// let ns = Namespace::try_new("global:cache").unwrap();
/// assert_eq!(ns.as_str(), "global:cache");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "String", try_from = "String"))]
pub struct Namespace(String);

impl Namespace {
    /// The maximum permitted length, in bytes.
    pub const MAX_LEN: usize = 1024;

    /// Validate and wrap a string as a [`Namespace`].
    pub fn try_new(s: impl Into<String>) -> Result<Self, NamespaceError> {
        let s = s.into();
        if s.is_empty() {
            return Err(NamespaceError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(NamespaceError::TooLong {
                max: Self::MAX_LEN,
                got: s.len(),
            });
        }
        for (pos, byte) in s.bytes().enumerate() {
            if byte < 0x20 || byte == 0x7F {
                return Err(NamespaceError::InvalidByte { pos });
            }
        }
        Ok(Self(s))
    }

    /// View as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Namespace> for String {
    fn from(ns: Namespace) -> Self {
        ns.0
    }
}

impl TryFrom<String> for Namespace {
    type Error = NamespaceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

/// Validated storage key. Within a namespace, opaque content.
///
/// Validation rules:
/// - Non-empty.
/// - At most [`Key::MAX_LEN`] bytes.
/// - No NUL bytes (`\0`). Keys may contain control characters and
///   arbitrary UTF-8 (e.g., `"\n"`, `"foo:bar"`).
///
/// # Example
///
/// ```
/// use tau_ports::storage::Key;
///
/// let k = Key::try_new("agent:01890000-0000-7000-8000-000000000001").unwrap();
/// assert_eq!(k.as_str(), "agent:01890000-0000-7000-8000-000000000001");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "String", try_from = "String"))]
pub struct Key(String);

impl Key {
    /// The maximum permitted length, in bytes.
    pub const MAX_LEN: usize = 1024;

    /// Validate and wrap a string as a [`Key`].
    pub fn try_new(s: impl Into<String>) -> Result<Self, KeyError> {
        let s = s.into();
        if s.is_empty() {
            return Err(KeyError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(KeyError::TooLong {
                max: Self::MAX_LEN,
                got: s.len(),
            });
        }
        for (pos, byte) in s.bytes().enumerate() {
            if byte == 0 {
                return Err(KeyError::InvalidByte { pos });
            }
        }
        Ok(Self(s))
    }

    /// View as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Key> for String {
    fn from(k: Key) -> Self {
        k.0
    }
}

impl TryFrom<String> for Key {
    type Error = KeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

/// Trait implemented by `kind = "storage"` plugins.
///
/// v0.1 surface is KV-only. Per G8, the namespace carries scope
/// (e.g., global / project / agent-instance); tau-runtime composes
/// namespaces and plugins consume them opaquely.
///
/// `Send + Sync` so the runtime can store impls in a multi-task plugin
/// registry.
#[allow(async_fn_in_trait)]
pub trait Storage: Send + Sync {
    /// Plugin-visible name (matches the package name; for diagnostics).
    fn name(&self) -> &str;

    /// Fetch the value at `(namespace, key)`. Returns `Ok(None)` if absent.
    async fn get(&self, namespace: &Namespace, key: &Key) -> Result<Option<Vec<u8>>, StorageError>;

    /// Set the value at `(namespace, key)`. Overwrites any existing value.
    async fn put(&self, namespace: &Namespace, key: &Key, value: &[u8])
        -> Result<(), StorageError>;

    /// Delete the key. Returns `true` if a key was deleted, `false` if
    /// it wasn't present.
    async fn delete(&self, namespace: &Namespace, key: &Key) -> Result<bool, StorageError>;

    /// List all keys under `namespace` whose names begin with `prefix`.
    /// Use empty string `""` to list all keys in the namespace.
    /// Order is plugin-defined — callers must not rely on alphabetical.
    async fn list(&self, namespace: &Namespace, prefix: &str) -> Result<Vec<Key>, StorageError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_accepts_valid() {
        let max_len = "x".repeat(1024);
        for s in ["a", "global:cache", "agent:01-23-45", max_len.as_str()] {
            assert!(Namespace::try_new(s).is_ok(), "should accept {s:?}");
        }
    }

    #[test]
    fn namespace_rejects_empty() {
        assert_eq!(Namespace::try_new(""), Err(NamespaceError::Empty));
    }

    #[test]
    fn namespace_rejects_too_long() {
        let s = "a".repeat(1025);
        assert_eq!(
            Namespace::try_new(&s),
            Err(NamespaceError::TooLong {
                max: 1024,
                got: 1025
            }),
        );
    }

    #[test]
    fn namespace_rejects_nul() {
        assert_eq!(
            Namespace::try_new("foo\0bar"),
            Err(NamespaceError::InvalidByte { pos: 3 }),
        );
    }

    #[test]
    fn namespace_rejects_control_char() {
        assert_eq!(
            Namespace::try_new("foo\nbar"),
            Err(NamespaceError::InvalidByte { pos: 3 }),
        );
        assert_eq!(
            Namespace::try_new("foo\x7fbar"),
            Err(NamespaceError::InvalidByte { pos: 3 }),
        );
    }

    #[test]
    fn key_accepts_valid_including_control_chars() {
        for s in ["a", "agent:foo", "with\tnewlines\n", "with:colons:ok"] {
            assert!(Key::try_new(s).is_ok(), "should accept {s:?}");
        }
    }

    #[test]
    fn key_rejects_empty() {
        assert_eq!(Key::try_new(""), Err(KeyError::Empty));
    }

    #[test]
    fn key_rejects_too_long() {
        let s = "a".repeat(1025);
        assert_eq!(
            Key::try_new(&s),
            Err(KeyError::TooLong {
                max: 1024,
                got: 1025
            }),
        );
    }

    #[test]
    fn key_rejects_nul_only() {
        assert_eq!(
            Key::try_new("foo\0bar"),
            Err(KeyError::InvalidByte { pos: 3 }),
        );
    }

    #[test]
    fn display_round_trips() {
        let ns = Namespace::try_new("global:cache").unwrap();
        assert_eq!(ns.to_string(), "global:cache");
        let k = Key::try_new("foo").unwrap();
        assert_eq!(k.to_string(), "foo");
    }
}
