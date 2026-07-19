//! Validated network host/method value types shared by `net.http`
//! capabilities, the capability lattice, and the sandbox proxy.
//!
//! One semantic end-to-end: hosts are bare lowercase hostnames (optional
//! port) or the typed [`HostSet::Any`] sentinel — never a URL, scheme, glob,
//! or IP-with-brackets. Suffix wildcards and IPv6 literals are deliberately
//! deferred (additive later).

use alloc::string::String;
use core::fmt;

/// A validated bare hostname with an optional `:port`.
///
/// Invariant (guaranteed by [`HostName::parse`]): ASCII, lowercase, labels of
/// `[a-z0-9-]` separated by `.`, optional trailing `:<port>` (1..=65535); no
/// scheme, `@`, `/`, `[`, `]`, `*`, or whitespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostName(String);

/// Why a string is not a valid [`HostName`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostNameError {
    /// Contained a `*` (globs/`any` are not hostnames).
    Wildcard,
    /// Looked like a URL (scheme `://`, `@`, or `/`).
    UrlShaped,
    /// Contained `[` or `]` (IPv6 literal — not yet supported).
    BracketedIp,
    /// Empty host, empty label, or whitespace.
    Empty,
    /// A label held a character outside `[a-z0-9-]`.
    BadChar(char),
    /// The `:port` suffix was absent-digits or out of 1..=65535.
    BadPort,
}

impl fmt::Display for HostNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostNameError::Wildcard => {
                write!(f, "wildcards are not hostnames; write hosts = \"any\", or (suffix wildcards) enumerate the hosts")
            }
            HostNameError::UrlShaped => write!(f, "write the bare host, not a URL"),
            HostNameError::BracketedIp => write!(f, "IPv6 literal hosts are not yet supported"),
            HostNameError::Empty => write!(f, "empty host or label"),
            HostNameError::BadChar(c) => write!(f, "invalid character {c:?} in host label"),
            HostNameError::BadPort => write!(f, "port must be an integer in 1..=65535"),
        }
    }
}

impl HostName {
    /// Parse and case-fold. `A.COM` → `a.com` (accept-and-fold, never reject
    /// on case). See the module docs for the full accept/reject contract.
    pub fn parse(s: &str) -> Result<HostName, HostNameError> {
        if s.contains('*') {
            return Err(HostNameError::Wildcard);
        }
        if s.contains("://") || s.contains('@') || s.contains('/') {
            return Err(HostNameError::UrlShaped);
        }
        if s.contains('[') || s.contains(']') {
            return Err(HostNameError::BracketedIp);
        }
        if s.is_empty() || s.chars().any(|c| c.is_whitespace()) {
            return Err(HostNameError::Empty);
        }
        let folded = s.to_ascii_lowercase();
        // Split optional :port (at most one ':').
        let (host, port) = match folded.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (folded.as_str(), None),
        };
        if let Some(p) = port {
            match p.parse::<u32>() {
                Ok(n) if (1..=65535).contains(&n) => {}
                _ => return Err(HostNameError::BadPort),
            }
        }
        if host.is_empty() {
            return Err(HostNameError::Empty);
        }
        for label in host.split('.') {
            if label.is_empty() {
                return Err(HostNameError::Empty);
            }
            for c in label.chars() {
                if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                    return Err(HostNameError::BadChar(c));
                }
            }
        }
        Ok(HostName(folded))
    }

    /// The canonical (lowercase) host string, including any `:port`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod host_name_tests {
    use super::*;

    #[test]
    fn accepts_plain_and_port_and_punycode() {
        for ok in [
            "api.anthropic.com",
            "localhost:8080",
            "b.io:8080",
            "xn--nxasmq6b.com",
        ] {
            assert!(HostName::parse(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn folds_case() {
        assert_eq!(HostName::parse("A.COM").unwrap().as_str(), "a.com");
    }

    #[test]
    fn rejects_wildcards_urls_ipv6_paths_at_users() {
        assert_eq!(HostName::parse("*"), Err(HostNameError::Wildcard));
        assert_eq!(HostName::parse("*.a.com"), Err(HostNameError::Wildcard));
        assert_eq!(
            HostName::parse("https://a.com"),
            Err(HostNameError::UrlShaped)
        );
        assert_eq!(HostName::parse("a.com/path"), Err(HostNameError::UrlShaped));
        assert_eq!(HostName::parse("user@a.com"), Err(HostNameError::UrlShaped));
        assert_eq!(
            HostName::parse("[::1]:8080"),
            Err(HostNameError::BracketedIp)
        );
        assert_eq!(HostName::parse(""), Err(HostNameError::Empty));
        assert!(matches!(
            HostName::parse("bad_host"),
            Err(HostNameError::BadChar('_'))
        ));
        assert_eq!(HostName::parse("a.com:0"), Err(HostNameError::BadPort));
        assert_eq!(HostName::parse("a.com:99999"), Err(HostNameError::BadPort));
    }
}
