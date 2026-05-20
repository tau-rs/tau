//! Parse errors for `TargetTriple` and its sub-enums.

/// Error returned when parsing a [`super::triple::TargetTriple`] or one of its
/// sub-enum axes from a string.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The input string was empty.
    #[error("empty triple")]
    Empty,
    /// The triple had an unexpected number of hyphen-separated segments.
    #[error("triple has {0} segments; expected 1 or 3")]
    WrongSegmentCount(usize),
    /// A single-segment input was not a recognised special (e.g. `passthrough`).
    #[error("unknown single-segment triple `{0}`; expected one of: passthrough")]
    UnknownSpecial(String),
    /// The platform segment was not recognised.
    #[error("unknown platform `{0}`; expected one of: linux, darwin, windows, any")]
    UnknownPlatform(String),
    /// The adapter-family segment was not recognised.
    #[error("unknown adapter family `{0}`; expected one of: native, container, remote, wasi, passthrough")]
    UnknownAdapterFamily(String),
    /// The tier segment was not recognised.
    #[error("unknown tier `{0}`; expected one of: strict, light, none")]
    UnknownTier(String),
    /// The input contained a character outside `[a-z-]`.
    #[error("invalid character `{0}` in triple; only lowercase ASCII letters and hyphens allowed")]
    InvalidChar(char),
}
