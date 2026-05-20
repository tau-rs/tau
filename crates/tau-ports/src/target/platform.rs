//! Platform axis of `TargetTriple`.

use std::fmt;
use std::str::FromStr;

use crate::target::parse::ParseError;

/// Platform an adapter targets.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Linux platform.
    Linux,
    /// macOS / Darwin platform.
    Darwin,
    /// Windows platform.
    Windows,
    /// Platform-agnostic (used for the `passthrough` special).
    Any,
}

impl Platform {
    /// Returns the canonical lowercase string for this platform.
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Darwin => "darwin",
            Platform::Windows => "windows",
            Platform::Any => "any",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "linux" => Ok(Platform::Linux),
            "darwin" => Ok(Platform::Darwin),
            "windows" => Ok(Platform::Windows),
            "any" => Ok(Platform::Any),
            other => Err(ParseError::UnknownPlatform(other.to_string())),
        }
    }
}
