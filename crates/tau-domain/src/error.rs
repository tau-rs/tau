//! Per-concern error enums for `tau-domain`.
//!
//! Each error type is `#[non_exhaustive]` so additive variants are non-breaking.
//! All errors derive `Debug + Error + Clone + PartialEq + Eq`; tests with
//! free-form `String` fields use `matches!()` to avoid brittle wording
//! comparisons.

use alloc::string::String;

use thiserror::Error;

/// Validation errors for [`crate::id::PackageName`].
///
/// # Example
///
/// ```
/// use tau_domain::{PackageName, PackageNameError};
/// use std::str::FromStr;
///
/// let err = PackageName::from_str("").unwrap_err();
/// assert_eq!(err, PackageNameError::Empty);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageNameError {
    /// The input was empty.
    #[error("package name is empty")]
    Empty,
    /// The input exceeded the 64-character cap.
    #[error("package name exceeds {max} characters: got {got}")]
    TooLong {
        /// Maximum permitted length.
        max: usize,
        /// Actual length of the input.
        got: usize,
    },
    /// A character outside `[a-z0-9-]` was found mid-string.
    #[error("package name contains invalid character {ch:?} at byte {pos}")]
    InvalidCharacter {
        /// The offending character.
        ch: char,
        /// Byte position in the input string.
        pos: usize,
    },
    /// The leading character was not an ASCII lowercase letter.
    #[error("package name must start with a letter, got {ch:?}")]
    InvalidLeadingCharacter {
        /// The first character of the input.
        ch: char,
    },
}

/// Validation errors for [`crate::id::AgentId`].
///
/// # Example
///
/// ```
/// use tau_domain::{AgentId, AgentIdError};
/// use std::str::FromStr;
///
/// let err = AgentId::from_str("").unwrap_err();
/// assert_eq!(err, AgentIdError::Empty);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentIdError {
    /// The input was empty.
    #[error("agent id is empty")]
    Empty,
    /// The input exceeded the 64-character cap.
    #[error("agent id exceeds {max} characters: got {got}")]
    TooLong {
        /// Maximum permitted length.
        max: usize,
        /// Actual length of the input.
        got: usize,
    },
    /// A character outside `[a-z0-9-]` was found mid-string.
    #[error("agent id contains invalid character {ch:?} at byte {pos}")]
    InvalidCharacter {
        /// The offending character.
        ch: char,
        /// Byte position in the input string.
        pos: usize,
    },
    /// The leading character was not an ASCII lowercase letter.
    #[error("agent id must start with a letter, got {ch:?}")]
    InvalidLeadingCharacter {
        /// The first character of the input.
        ch: char,
    },
}

/// Parser/validation errors for [`crate::package::PackageSource`] and
/// [`crate::package::GitLocation`].
///
/// # Example
///
/// ```
/// use tau_domain::{PackageSource, PackageSourceError};
/// use std::str::FromStr;
///
/// let err = PackageSource::from_str("").unwrap_err();
/// assert_eq!(err, PackageSourceError::Empty);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageSourceError {
    /// The input was empty.
    #[error("package source is empty")]
    Empty,
    /// The URL had a scheme outside the allowed set.
    #[error("unsupported URL scheme {scheme:?}; expected https, http, ssh, or git")]
    UnsupportedScheme {
        /// The rejected scheme.
        scheme: String,
    },
    /// The URL did not parse as RFC 3986 *and* did not match scp-style.
    #[error("malformed URL: {reason}")]
    MalformedUrl {
        /// Upstream parser's diagnostic.
        reason: String,
    },
    /// The scp-style address could not be parsed.
    #[error("malformed scp-style address: {reason}")]
    MalformedScpAddress {
        /// Diagnostic from the scp-style parser.
        reason: String,
    },
    /// The fragment after `#` was empty.
    #[error("revision is empty after '#'")]
    EmptyRevision,
}

/// Validation errors for [`crate::package::PackageKind`].
///
/// # Example
///
/// ```
/// use tau_domain::PackageKindError;
///
/// // Empty is the only variant at v0.1.
/// let err = PackageKindError::Empty;
/// assert!(err.to_string().contains("empty"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageKindError {
    /// The kind string was empty.
    #[error("package kind is empty")]
    Empty,
}

/// Validation errors for [`crate::package::PackageManifest`].
///
/// Composes leaf errors (`PackageNameError`, `PackageSourceError`,
/// `PackageKindError`) via `#[from]` for the first occurrence and
/// `#[source]` for repeated uses.
///
/// # Example
///
/// ```
/// use tau_domain::PackageManifestError;
///
/// // EmptyDescription is returned when the `description` field is blank.
/// let err = PackageManifestError::EmptyDescription;
/// assert!(err.to_string().contains("description"));
///
/// // CapabilityEmptyName is returned when a Custom capability has no name.
/// let err2 = PackageManifestError::CapabilityEmptyName { index: 0 };
/// assert!(err2.to_string().contains("capability"));
/// assert!(err2.to_string().contains('0'.to_string().as_str()));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageManifestError {
    /// The `name` field failed `PackageName` validation.
    #[error("manifest field 'name': {0}")]
    Name(#[from] PackageNameError),
    /// The `source` field failed parser/validator.
    #[error("manifest field 'source': {0}")]
    Source(#[from] PackageSourceError),
    /// The `kind` field failed validation.
    #[error("manifest field 'kind': {0}")]
    Kind(#[from] PackageKindError),
    /// The `description` field was empty.
    #[error("manifest field 'description' is empty")]
    EmptyDescription,
    /// A dependency entry's name failed validation.
    #[error("dependency #{index}: invalid name: {source}")]
    DependencyName {
        /// 0-based index of the offending dependency.
        index: usize,
        /// Underlying name validation error.
        #[source]
        source: PackageNameError,
    },
    /// A `Capability::Custom` entry had an empty `name`.
    #[error("capability #{index} has empty name")]
    CapabilityEmptyName {
        /// 0-based index of the offending capability.
        index: usize,
    },
    /// A package declaring `kind = "skill"` must not also carry a
    /// `[plugin]` table. Plugins and skills are mutually exclusive
    /// package kinds — a "tool that ships its own usage doc as a skill"
    /// is a future Skills-5 concern (composable manifests); v1 keeps
    /// the two kinds separate.
    #[error("kind = \"skill\" packages cannot have a [plugin] block")]
    SkillCannotHavePluginBlock,
}

/// Validation errors for `<crate::package::PortKind as std::str::FromStr>::from_str`.
///
/// # Example
///
/// ```
/// use tau_domain::PortKindError;
/// use tau_domain::PortKind;
/// use std::str::FromStr;
///
/// let err = PortKind::from_str("nonsense").unwrap_err();
/// assert!(matches!(err, PortKindError::Unknown { .. }));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortKindError {
    /// The input did not match any known port kind.
    #[error("unknown port kind {input:?}; expected one of: llm_backend, tool, storage, sandbox")]
    Unknown {
        /// The input that did not parse.
        input: String,
    },
}

/// Validation errors for `<crate::package::PluginKind as std::str::FromStr>::from_str`.
///
/// # Example
///
/// ```
/// use tau_domain::PluginKindError;
/// use tau_domain::PluginKind;
/// use std::str::FromStr;
///
/// let err = PluginKind::from_str("nonsense").unwrap_err();
/// assert!(matches!(err, PluginKindError::Unknown { .. }));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PluginKindError {
    /// The input did not match any known plugin kind.
    ///
    /// v0.1 only supports `rust-cargo`. Future kinds (`python-pip`,
    /// `node-npm`, `prebuilt`) are tracked in spec §2.1.
    #[error("unknown plugin kind {input:?}; expected: rust-cargo")]
    Unknown {
        /// The input that did not parse.
        input: String,
    },
}
