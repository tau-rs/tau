//! Validated network host/method value types shared by `net.http`
//! capabilities, the capability lattice, and the sandbox proxy.
//!
//! One semantic end-to-end: hosts are bare lowercase hostnames (optional
//! port) or the typed [`HostSet::Any`] sentinel — never a URL, scheme, glob,
//! or IP-with-brackets. Suffix wildcards and IPv6 literals are deliberately
//! deferred (additive later).

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
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

/// One of the 9 standard HTTP verbs. Obscure/extension verbs (PROPFIND, …)
/// are a deliberate not-yet — additive later, like suffix wildcards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    /// GET verb.
    Get,
    /// HEAD verb.
    Head,
    /// POST verb.
    Post,
    /// PUT verb.
    Put,
    /// DELETE verb.
    Delete,
    /// CONNECT verb.
    Connect,
    /// OPTIONS verb.
    Options,
    /// TRACE verb.
    Trace,
    /// PATCH verb.
    Patch,
}

/// An unrecognized HTTP method token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpMethodError(pub String);

impl fmt::Display for HttpMethodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown HTTP method {:?} (expected one of GET, HEAD, POST, PUT, DELETE, CONNECT, OPTIONS, TRACE, PATCH)",
            self.0
        )
    }
}

impl HttpMethod {
    /// Parse case-insensitively; canonical output is uppercase.
    pub fn parse(s: &str) -> Result<HttpMethod, HttpMethodError> {
        Ok(match s.to_ascii_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "HEAD" => HttpMethod::Head,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "DELETE" => HttpMethod::Delete,
            "CONNECT" => HttpMethod::Connect,
            "OPTIONS" => HttpMethod::Options,
            "TRACE" => HttpMethod::Trace,
            "PATCH" => HttpMethod::Patch,
            _ => return Err(HttpMethodError(s.to_string())),
        })
    }

    /// The canonical uppercase verb.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Head => "HEAD",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Connect => "CONNECT",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
            HttpMethod::Patch => "PATCH",
        }
    }
}

/// The host ceiling of a `net.http` capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSet {
    /// Any host (authored `hosts = "any"`). Widest ceiling.
    Any,
    /// Exactly these hosts (authored `hosts = ["a.com", …]`).
    Exact(BTreeSet<HostName>),
}

impl HostSet {
    /// Ceiling subsumption: `self ⊇ child`.
    /// `Any` ⊇ everything; `Exact(p)` ⊇ `Exact(c)` ⟺ `c ⊆ p`; `Exact` ⊉ `Any`.
    pub fn subsumes(&self, child: &HostSet) -> bool {
        match (self, child) {
            (HostSet::Any, _) => true,
            (HostSet::Exact(_), HostSet::Any) => false,
            (HostSet::Exact(p), HostSet::Exact(c)) => c.is_subset(p),
        }
    }

    /// True iff this is the `Any` sentinel.
    pub fn is_any(&self) -> bool {
        matches!(self, HostSet::Any)
    }

    /// Sorted canonical host strings; empty for `Any`.
    pub fn exact_hosts(&self) -> Vec<String> {
        match self {
            HostSet::Any => Vec::new(),
            HostSet::Exact(set) => set.iter().map(|h| h.as_str().to_string()).collect(),
        }
    }
}

#[cfg(test)]
mod host_set_tests {
    use super::*;

    fn exact(hosts: &[&str]) -> HostSet {
        HostSet::Exact(hosts.iter().map(|h| HostName::parse(h).unwrap()).collect())
    }

    #[test]
    fn subsumes_truth_table() {
        assert!(HostSet::Any.subsumes(&exact(&["a.com"])));
        assert!(HostSet::Any.subsumes(&HostSet::Any));
        assert!(!exact(&["a.com"]).subsumes(&HostSet::Any)); // Exact ⊉ Any
        assert!(exact(&["a.com", "b.com"]).subsumes(&exact(&["a.com"])));
        assert!(!exact(&["a.com"]).subsumes(&exact(&["a.com", "b.com"])));
    }

    #[test]
    fn exact_hosts_are_sorted_and_folded() {
        assert_eq!(
            exact(&["B.com", "a.com"]).exact_hosts(),
            vec!["a.com", "b.com"]
        );
        assert!(HostSet::Any.exact_hosts().is_empty());
    }
}

#[cfg(test)]
mod http_method_tests {
    use super::*;

    #[test]
    fn parses_case_insensitively_and_canonicalizes() {
        assert_eq!(HttpMethod::parse("get").unwrap(), HttpMethod::Get);
        assert_eq!(HttpMethod::parse("PoSt").unwrap().as_str(), "POST");
    }

    #[test]
    fn rejects_unknown_verb() {
        assert_eq!(HttpMethod::parse("GTE"), Err(HttpMethodError("GTE".into())));
    }
}

#[cfg(feature = "serde")]
mod host_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    impl Serialize for HostName {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(self.as_str())
        }
    }
    impl<'de> Deserialize<'de> for HostName {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            HostName::parse(&s).map_err(serde::de::Error::custom)
        }
    }

    impl Serialize for HttpMethod {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(self.as_str())
        }
    }
    impl<'de> Deserialize<'de> for HttpMethod {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            HttpMethod::parse(&s).map_err(serde::de::Error::custom)
        }
    }

    // HostSet derives via HostName's impls; a small manual impl keeps the
    // `Any`/`Exact` shape explicit for the vestigial NetCapability derive path.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum HostSetRepr {
        Any,
        Exact(BTreeSet<HostName>),
    }
    impl Serialize for HostSet {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match self {
                HostSet::Any => HostSetRepr::Any.serialize(s),
                HostSet::Exact(set) => HostSetRepr::Exact(set.clone()).serialize(s),
            }
        }
    }
    impl<'de> Deserialize<'de> for HostSet {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match HostSetRepr::deserialize(d)? {
                HostSetRepr::Any => HostSet::Any,
                HostSetRepr::Exact(set) => HostSet::Exact(set),
            })
        }
    }
}
