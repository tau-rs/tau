//! Package source location grammar.
//!
//! tau-domain v0.1 only models `Git` sources. Local paths, registry-style
//! sources, and tarball URLs land as additive `PackageSource` variants
//! later (see `docs/explanation/escape-hatches.md` for the broader
//! escape-hatch policy this is consistent with).

use std::fmt;
use std::str::FromStr;

use crate::error::PackageSourceError;

/// A package source location. v0.1: git only.
///
/// Canonical text form: `<location>` or `<location>#<rev>`.
///
/// # Example
///
/// ```
/// use tau_domain::PackageSource;
/// use std::str::FromStr;
///
/// let s = PackageSource::from_str("https://github.com/example/repo.git#main").unwrap();
/// assert_eq!(
///     s.to_string(),
///     "https://github.com/example/repo.git#main",
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    /// A git repository location, optionally pinned to a revision.
    Git {
        /// Where the repository lives.
        location: GitLocation,
        /// Branch, tag, or commit SHA. Opaque to tau-domain; tau-pkg
        /// disambiguates at clone time.
        rev: Option<String>,
    },
}

#[cfg(feature = "serde")]
impl serde::Serialize for PackageSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PackageSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PackageSource;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a PackageSource string in the form `<location>` or `<location>#<rev>`")
            }
            fn visit_str<E>(self, v: &str) -> Result<PackageSource, E>
            where
                E: serde::de::Error,
            {
                use std::str::FromStr;
                PackageSource::from_str(v).map_err(serde::de::Error::custom)
            }
            fn visit_string<E>(self, v: String) -> Result<PackageSource, E>
            where
                E: serde::de::Error,
            {
                self.visit_str(&v)
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Where a git repository lives. Two shapes because git itself accepts both.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitLocation {
    /// Standard URL (https / http / ssh / git / file scheme).
    Url(url::Url),
    /// scp-style address, e.g. `git@github.com:owner/repo.git`. Not a
    /// valid URL by RFC 3986; git accepts it natively.
    Scp {
        /// Optional user component, e.g. `git`.
        user: Option<String>,
        /// Hostname.
        host: String,
        /// Repository path on the host.
        path: String,
    },
}

impl FromStr for PackageSource {
    type Err = PackageSourceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(PackageSourceError::Empty);
        }
        let (loc_str, rev) = match s.split_once('#') {
            Some((_, "")) => return Err(PackageSourceError::EmptyRevision),
            Some((loc, rev)) => (loc, Some(rev.to_owned())),
            None => (s, None),
        };
        let location = GitLocation::from_str(loc_str)?;
        Ok(PackageSource::Git { location, rev })
    }
}

impl fmt::Display for PackageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let PackageSource::Git { location, rev } = self;
        write!(f, "{location}")?;
        if let Some(r) = rev {
            write!(f, "#{r}")?;
        }
        Ok(())
    }
}

impl FromStr for GitLocation {
    type Err = PackageSourceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(PackageSourceError::Empty);
        }
        // Try url::Url first. url::Url::parse accepts scp-like inputs such as
        // `github.com:owner/repo.git` as URLs with a custom scheme, so we use
        // `s.contains("://")` to discriminate true URLs from scp-style addresses
        // before deciding whether a non-allowed scheme is an error or a fall-through.
        match url::Url::parse(s) {
            Ok(url) => match url.scheme() {
                "https" | "http" | "ssh" | "git" | "file" => Ok(GitLocation::Url(url)),
                other if s.contains("://") => Err(PackageSourceError::UnsupportedScheme {
                    scheme: other.to_owned(),
                }),
                _ => parse_scp(s),
            },
            Err(parse_err) => {
                // Fall through to scp-style only if the URL wasn't recognized
                // as having a scheme. If parser thinks it has a scheme but
                // failed for another reason, surface the URL error.
                if !s.contains("://") {
                    parse_scp(s)
                } else {
                    Err(PackageSourceError::MalformedUrl {
                        reason: parse_err.to_string(),
                    })
                }
            }
        }
    }
}

impl fmt::Display for GitLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitLocation::Url(u) => write!(f, "{u}"),
            GitLocation::Scp { user, host, path } => {
                if let Some(u) = user {
                    write!(f, "{u}@{host}:{path}")
                } else {
                    write!(f, "{host}:{path}")
                }
            }
        }
    }
}

fn parse_scp(s: &str) -> Result<GitLocation, PackageSourceError> {
    // scp grammar: [user@]host:path  where ':' is the first colon and is
    // NOT followed by '/' (which would be ambiguous with `host:port/path`).
    let colon_pos = s
        .find(':')
        .ok_or_else(|| PackageSourceError::MalformedScpAddress {
            reason: "missing ':' separator".to_owned(),
        })?;
    if s[colon_pos + 1..].starts_with('/') {
        return Err(PackageSourceError::MalformedScpAddress {
            reason: "':' followed by '/' (ambiguous with port form)".to_owned(),
        });
    }
    let (user_host, path) = s.split_at(colon_pos);
    let path = &path[1..]; // strip the ':'
    if path.is_empty() {
        return Err(PackageSourceError::MalformedScpAddress {
            reason: "path component empty".to_owned(),
        });
    }
    let (user, host) = match user_host.split_once('@') {
        Some((u, h)) if !u.is_empty() && !h.is_empty() => (Some(u.to_owned()), h.to_owned()),
        Some(_) => {
            return Err(PackageSourceError::MalformedScpAddress {
                reason: "empty user or host around '@'".to_owned(),
            })
        }
        None => {
            if user_host.is_empty() {
                return Err(PackageSourceError::MalformedScpAddress {
                    reason: "host component empty".to_owned(),
                });
            }
            (None, user_host.to_owned())
        }
    };
    Ok(GitLocation::Scp {
        user,
        host,
        path: path.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn parses_https_url() {
        let s = PackageSource::from_str("https://github.com/owner/repo.git").unwrap();
        assert_matches!(
            s,
            PackageSource::Git {
                location: GitLocation::Url(u),
                rev: None,
            } => {
                assert_eq!(u.scheme(), "https");
            }
        );
    }

    #[test]
    fn parses_https_with_rev() {
        let s = PackageSource::from_str("https://github.com/owner/repo.git#v1.2.3").unwrap();
        let PackageSource::Git { rev, .. } = s;
        assert_eq!(rev.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn parses_scp_form() {
        let s = PackageSource::from_str("git@github.com:owner/repo.git").unwrap();
        assert_matches!(
            s,
            PackageSource::Git {
                location: GitLocation::Scp { user, host, path },
                ..
            } => {
                assert_eq!(user.as_deref(), Some("git"));
                assert_eq!(host, "github.com");
                assert_eq!(path, "owner/repo.git");
            }
        );
    }

    #[test]
    fn parses_scp_without_user() {
        let s = PackageSource::from_str("github.com:owner/repo.git").unwrap();
        assert_matches!(
            s,
            PackageSource::Git {
                location:
                    GitLocation::Scp {
                        user: None,
                        host,
                        path,
                    },
                ..
            } => {
                assert_eq!(host, "github.com");
                assert_eq!(path, "owner/repo.git");
            }
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(PackageSource::from_str(""), Err(PackageSourceError::Empty));
    }

    #[test]
    fn rejects_empty_rev() {
        assert_eq!(
            PackageSource::from_str("https://x.com/r.git#"),
            Err(PackageSourceError::EmptyRevision),
        );
    }

    #[test]
    fn rejects_unsupported_scheme() {
        let err = PackageSource::from_str("ftp://x.com/r.git").unwrap_err();
        assert!(matches!(err, PackageSourceError::UnsupportedScheme { scheme } if scheme == "ftp"));
    }

    #[test]
    fn rejects_scp_colon_slash() {
        let err = PackageSource::from_str("github.com:/owner/repo.git").unwrap_err();
        assert!(matches!(
            err,
            PackageSourceError::MalformedScpAddress { .. }
        ));
    }

    #[test]
    fn display_round_trips_url() {
        for s in [
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo.git#main",
            "ssh://git@example.com/repo.git",
        ] {
            let parsed = PackageSource::from_str(s).unwrap();
            assert_eq!(parsed.to_string(), s, "mismatch on {s:?}");
        }
    }

    #[test]
    fn display_round_trips_scp() {
        for s in [
            "git@github.com:owner/repo.git",
            "github.com:owner/repo.git",
            "git@github.com:owner/repo.git#v1.0",
        ] {
            let parsed = PackageSource::from_str(s).unwrap();
            assert_eq!(parsed.to_string(), s, "mismatch on {s:?}");
        }
    }
}
