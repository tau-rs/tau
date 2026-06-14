//! Logical credential identifier, e.g. `anthropic_api_key`.

use alloc::string::String;
use core::fmt;

/// A validated logical credential id. Charset: `[a-z0-9_.-]`, non-empty.
/// Used as the lookup key a provider resolves.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialId(String);

/// Error returned when a string is not a valid [`CredentialId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCredentialId {
    /// Human-readable reason.
    pub reason: &'static str,
}

impl CredentialId {
    /// Parse and validate a credential id.
    pub fn parse(s: impl Into<String>) -> Result<Self, InvalidCredentialId> {
        let s = s.into();
        if s.is_empty() {
            return Err(InvalidCredentialId {
                reason: "credential id must be non-empty",
            });
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
        {
            return Err(InvalidCredentialId {
                reason: "credential id must match [a-z0-9_.-]",
            });
        }
        Ok(Self(s))
    }

    /// Borrow the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialId({})", self.0)
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for id in ["anthropic_api_key", "openai.org", "a-b-c", "x1"] {
            assert!(CredentialId::parse(id).is_ok(), "{id} should parse");
        }
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in ["", "Upper", "has space", "tab\t", "UPPER_CASE"] {
            assert!(CredentialId::parse(id).is_err(), "{id} should be rejected");
        }
    }

    #[test]
    fn as_str_roundtrips() {
        let id = CredentialId::parse("anthropic_api_key").unwrap();
        assert_eq!(id.as_str(), "anthropic_api_key");
    }
}
