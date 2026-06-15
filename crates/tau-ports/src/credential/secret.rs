//! Resolved-secret value: redacts on `Debug`, zeroized on drop.

use alloc::vec::Vec;
use core::fmt;
use zeroize::Zeroizing;

/// A resolved credential value. Holds raw bytes (not `String`) because
/// device / secure-element keys are binary. The inner buffer is zeroized
/// on drop, and `Debug` never reveals the contents.
pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    /// Wrap raw bytes as a secret.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Borrow the raw secret bytes.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the secret as UTF-8, if it is valid UTF-8 (API keys are).
    pub fn expose_str(&self) -> Result<&str, core::str::Utf8Error> {
        core::str::from_utf8(&self.0)
    }

    /// Length of the secret in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the secret is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for Secret {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(bytes)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    #[test]
    fn debug_redacts() {
        let s = Secret::from_bytes(b"sk-ant-supersecret".to_vec());
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert!(!format!("{s:?}").contains("supersecret"));
    }

    #[test]
    fn expose_roundtrips() {
        let s = Secret::from_bytes(b"abc123".to_vec());
        assert_eq!(s.expose_bytes(), b"abc123");
        assert_eq!(s.expose_str().unwrap(), "abc123");
        assert_eq!(s.len(), 6);
        assert!(!s.is_empty());
    }

    #[test]
    fn non_utf8_is_rejected_by_expose_str() {
        let s = Secret::from_bytes(vec![0xff, 0xfe]);
        assert!(s.expose_str().is_err());
        assert_eq!(s.expose_bytes(), &[0xff, 0xfe]);
    }
}
