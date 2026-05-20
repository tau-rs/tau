//! Plugin manifest types declared in a package's `tau.toml` `[plugin]`
//! table.
//!
//! Mirrors the ADR-0005 pattern from
//! [`PackageSource`](crate::package::source::PackageSource): enums serialize
//! as canonical strings via `Display`/`FromStr` (not as adjacent-tagged
//! objects), so a TOML `provides = "llm_backend"` round-trips cleanly.

use std::fmt;
use std::str::FromStr;

use crate::error::{PluginKindError, PortKindError};

/// Which port a plugin provides.
///
/// Serialized form (when the `serde` feature is on) is the canonical
/// string `llm_backend` / `tool` / `storage` / `sandbox`.
///
/// # Example
///
/// ```
/// use std::str::FromStr;
/// use tau_domain::PortKind;
///
/// // PortKind is `#[non_exhaustive]` (cannot exhaustive-match outside
/// // the crate) but the unit variants are themselves public and
/// // FromStr / Display works the same as for any other type.
/// let kind = PortKind::from_str("llm_backend").unwrap();
/// assert_eq!(kind.to_string(), "llm_backend");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortKind {
    /// LlmBackend port: provides `llm.complete` and friends.
    LlmBackend,
    /// Tool port: provides `tool.call`.
    Tool,
    /// Storage port: provides `storage.get`/`put`/`list`/`delete`.
    Storage,
    /// Sandbox port: reserved (in-process only at v0.1; no wire methods).
    Sandbox,
}

impl fmt::Display for PortKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PortKind::LlmBackend => "llm_backend",
            PortKind::Tool => "tool",
            PortKind::Storage => "storage",
            PortKind::Sandbox => "sandbox",
        })
    }
}

impl FromStr for PortKind {
    type Err = PortKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "llm_backend" => Ok(PortKind::LlmBackend),
            "tool" => Ok(PortKind::Tool),
            "storage" => Ok(PortKind::Storage),
            "sandbox" => Ok(PortKind::Sandbox),
            other => Err(PortKindError::Unknown {
                input: other.to_owned(),
            }),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for PortKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PortKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PortKind::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// What kind of plugin distribution this package is.
///
/// v0.1: only `RustCargo` (a Rust crate built with `cargo build
/// --release --bin <bin>` at install time). Future variants
/// (`PythonPip`, `NodeNpm`, `Prebuilt`) are additive — see spec §2.1.
///
/// Serialized form: the kebab-case string `rust-cargo`.
///
/// # Example
///
/// ```ignore
/// use tau_domain::PluginKind;
/// use std::str::FromStr;
///
/// let kind = PluginKind::from_str("rust-cargo").unwrap();
/// assert_eq!(kind.to_string(), "rust-cargo");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginKind {
    /// A Rust crate built via `cargo build --release --bin <bin>`.
    RustCargo,
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PluginKind::RustCargo => "rust-cargo",
        })
    }
}

impl FromStr for PluginKind {
    type Err = PluginKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rust-cargo" => Ok(PluginKind::RustCargo),
            other => Err(PluginKindError::Unknown {
                input: other.to_owned(),
            }),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for PluginKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PluginKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PluginKind::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Plugin manifest declared in a package's `tau.toml` `[plugin]` table.
///
/// Read-only at runtime; `tau-pkg` parses it during install and `tau-runtime`
/// consumes it via `LockedPlugin` (see `tau-pkg::lockfile`).
///
/// # Example
///
/// ```ignore
/// // `PluginManifest` is `#[non_exhaustive]`; constructed by tau-pkg
/// // during install. External callers (notably tau-runtime integration
/// // tests that synthesize a lockfile) build it via `serde::from_str`.
/// use tau_domain::PluginManifest;
/// let toml = r#"
///     provides = "llm_backend"
///     kind     = "rust-cargo"
///     bin      = "anthropic-plugin"
/// "#;
/// let m: PluginManifest = toml::from_str(toml).unwrap();
/// assert_eq!(m.bin, "anthropic-plugin");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PluginManifest {
    /// Which port this plugin provides.
    pub provides: PortKind,
    /// Distribution kind (build orchestration).
    pub kind: PluginKind,
    /// Cargo `[[bin]]` target name (when `kind == RustCargo`).
    pub bin: String,
}

impl PluginManifest {
    /// Construct a `PluginManifest`. `#[non_exhaustive]`; external
    /// callers (e.g. tau-runtime tests) use this constructor.
    pub fn new(provides: PortKind, kind: PluginKind, bin: String) -> Self {
        Self {
            provides,
            kind,
            bin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_kind_round_trip_via_display_from_str() {
        for kind in [
            PortKind::LlmBackend,
            PortKind::Tool,
            PortKind::Storage,
            PortKind::Sandbox,
        ] {
            let s = kind.to_string();
            let parsed = PortKind::from_str(&s).unwrap();
            assert_eq!(kind, parsed);
        }
    }

    #[test]
    fn port_kind_unknown_input_errors() {
        let err = PortKind::from_str("nope").unwrap_err();
        match err {
            crate::error::PortKindError::Unknown { input } => assert_eq!(input, "nope"),
            #[allow(unreachable_patterns)]
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn plugin_kind_round_trip() {
        let s = PluginKind::RustCargo.to_string();
        assert_eq!(s, "rust-cargo");
        assert_eq!(PluginKind::from_str(&s).unwrap(), PluginKind::RustCargo);
    }

    #[test]
    fn plugin_kind_unknown_input_errors() {
        let err = PluginKind::from_str("python-pip").unwrap_err();
        match err {
            crate::error::PluginKindError::Unknown { input } => assert_eq!(input, "python-pip"),
            #[allow(unreachable_patterns)]
            _ => panic!("expected Unknown"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn plugin_manifest_round_trips_through_toml() {
        let m = PluginManifest::new(
            PortKind::LlmBackend,
            PluginKind::RustCargo,
            "anthropic-plugin".to_string(),
        );
        let s = toml::to_string(&m).unwrap();
        let back: PluginManifest = toml::from_str(&s).unwrap();
        assert_eq!(m, back);
    }
}
