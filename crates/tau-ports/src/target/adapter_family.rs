//! AdapterFamily axis of `TargetTriple`.

use std::fmt;
use std::str::FromStr;

use crate::target::parse::ParseError;

/// Sandbox adapter family identified in a `TargetTriple`.
///
/// Mirrors `tau_runtime::sandbox::registry::RegistryKind` plus a `Wasi`
/// variant reserved for future WASI sandbox adapters (no impl in v1).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterFamily {
    /// OS-native sandbox adapter (landlock/seccomp on Linux, sandbox-exec on Darwin).
    Native,
    /// Container-based sandbox adapter (Podman/Docker).
    Container,
    /// Remote sandbox adapter (delegated execution).
    Remote,
    /// WASI sandbox adapter (reserved for future use).
    Wasi,
    /// Passthrough (no sandboxing); used in the single-segment `passthrough` special.
    Passthrough,
}

impl AdapterFamily {
    /// Returns the canonical lowercase string for this adapter family.
    pub fn as_str(&self) -> &'static str {
        match self {
            AdapterFamily::Native => "native",
            AdapterFamily::Container => "container",
            AdapterFamily::Remote => "remote",
            AdapterFamily::Wasi => "wasi",
            AdapterFamily::Passthrough => "passthrough",
        }
    }
}

impl fmt::Display for AdapterFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AdapterFamily {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "native" => Ok(AdapterFamily::Native),
            "container" => Ok(AdapterFamily::Container),
            "remote" => Ok(AdapterFamily::Remote),
            "wasi" => Ok(AdapterFamily::Wasi),
            "passthrough" => Ok(AdapterFamily::Passthrough),
            other => Err(ParseError::UnknownAdapterFamily(other.to_string())),
        }
    }
}
