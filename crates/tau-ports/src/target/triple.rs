//! `TargetTriple` — Bazel-inspired 3-axis structural identifier for
//! tau deployment targets. See ADR-0034 + spec
//! `2026-05-19-target-triple-registry-design.md`.

#[cfg(feature = "serde")]
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;

use crate::capability_gate::CapabilityTier;
use crate::target::adapter_family::AdapterFamily;
use crate::target::parse::ParseError;
use crate::target::platform::Platform;

/// A tau deployment target.
///
/// Three orthogonal axes (`platform`, `adapter_family`, `tier`)
/// combined as a compact `<platform>-<adapter>-<tier>` canonical
/// name. The `passthrough` single-segment special is also accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TargetTriple {
    /// Platform axis (Linux, Darwin, Windows, Any).
    pub platform: Platform,
    /// Adapter family axis (Native, Container, Remote, Wasi, Passthrough).
    pub adapter_family: AdapterFamily,
    /// Sandbox tier axis (Strict, Light, None).
    pub tier: CapabilityTier,
}

impl TargetTriple {
    /// The `passthrough` single-segment special.
    pub const PASSTHROUGH: TargetTriple = TargetTriple {
        platform: Platform::Any,
        adapter_family: AdapterFamily::Passthrough,
        tier: CapabilityTier::None,
    };

    /// Is this the `passthrough` special?
    pub fn is_passthrough(&self) -> bool {
        matches!(
            self,
            TargetTriple {
                platform: Platform::Any,
                adapter_family: AdapterFamily::Passthrough,
                tier: CapabilityTier::None,
            }
        )
    }

    /// Returns the `native-<host-os>-strict` triple for the platform this
    /// binary runs on. Used by callers (e.g. `tau build`) that need a
    /// sensible default target when no `--target` flag is supplied.
    ///
    /// # Panics
    ///
    /// Panics on a platform tau doesn't have a `native` adapter for
    /// (currently anything other than Linux / Darwin / Windows). The
    /// resolver's Available registry intentionally has no entry for such
    /// platforms, so panicking here is the correct fail-loud behavior —
    /// adding a new host requires explicit ADR work, not silent fallback.
    pub fn host() -> TargetTriple {
        let platform = if cfg!(target_os = "linux") {
            crate::target::platform::Platform::Linux
        } else if cfg!(target_os = "macos") {
            crate::target::platform::Platform::Darwin
        } else if cfg!(target_os = "windows") {
            crate::target::platform::Platform::Windows
        } else {
            panic!(
                "tau has no native adapter for this host target_os; \
                 add an entry to the target registry first"
            );
        };
        TargetTriple {
            platform,
            adapter_family: crate::target::adapter_family::AdapterFamily::Native,
            tier: crate::capability_gate::CapabilityTier::Strict,
        }
    }
}

impl fmt::Display for TargetTriple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_passthrough() {
            return f.write_str("passthrough");
        }
        write!(
            f,
            "{}-{}-{}",
            self.platform.as_str(),
            self.adapter_family.as_str(),
            tier_as_str(self.tier),
        )
    }
}

impl FromStr for TargetTriple {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseError::Empty);
        }
        if let Some(bad) = s.chars().find(|c| !(c.is_ascii_lowercase() || *c == '-')) {
            return Err(ParseError::InvalidChar(bad));
        }
        let segments: Vec<&str> = s.split('-').collect();
        match segments.as_slice() {
            [single] => match *single {
                "passthrough" => Ok(TargetTriple::PASSTHROUGH),
                other => Err(ParseError::UnknownSpecial(other.to_string())),
            },
            [p, a, t] => Ok(TargetTriple {
                platform: Platform::from_str(p)?,
                adapter_family: AdapterFamily::from_str(a)?,
                tier: tier_from_str(t)?,
            }),
            _ => Err(ParseError::WrongSegmentCount(segments.len())),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for TargetTriple {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for TargetTriple {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

fn tier_as_str(t: CapabilityTier) -> &'static str {
    match t {
        CapabilityTier::None => "none",
        CapabilityTier::Light => "light",
        CapabilityTier::Strict => "strict",
    }
}

fn tier_from_str(s: &str) -> Result<CapabilityTier, ParseError> {
    match s {
        "none" => Ok(CapabilityTier::None),
        "light" => Ok(CapabilityTier::Light),
        "strict" => Ok(CapabilityTier::Strict),
        other => Err(ParseError::UnknownTier(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_linux_native_strict() {
        let t: TargetTriple = "linux-native-strict".parse().unwrap();
        assert_eq!(t.platform, Platform::Linux);
        assert_eq!(t.adapter_family, AdapterFamily::Native);
        assert_eq!(t.tier, CapabilityTier::Strict);
    }

    #[test]
    fn parse_passthrough() {
        let t: TargetTriple = "passthrough".parse().unwrap();
        assert!(t.is_passthrough());
        assert_eq!(t, TargetTriple::PASSTHROUGH);
    }

    #[test]
    fn display_round_trips_for_three_segment() {
        let t = TargetTriple {
            platform: Platform::Darwin,
            adapter_family: AdapterFamily::Native,
            tier: CapabilityTier::Strict,
        };
        assert_eq!(t.to_string(), "darwin-native-strict");
        assert_eq!(t.to_string().parse::<TargetTriple>().unwrap(), t);
    }

    #[test]
    fn display_round_trips_for_passthrough() {
        assert_eq!(TargetTriple::PASSTHROUGH.to_string(), "passthrough");
        assert_eq!(
            "passthrough".parse::<TargetTriple>().unwrap(),
            TargetTriple::PASSTHROUGH
        );
    }

    #[test]
    fn empty_input_errors() {
        let e = "".parse::<TargetTriple>().unwrap_err();
        assert!(matches!(e, ParseError::Empty));
    }

    #[test]
    fn invalid_char_errors() {
        let e = "Linux-native-strict".parse::<TargetTriple>().unwrap_err();
        assert!(matches!(e, ParseError::InvalidChar('L')));
    }

    #[test]
    fn wrong_segment_count_errors() {
        let e = "linux-native".parse::<TargetTriple>().unwrap_err();
        assert!(matches!(e, ParseError::WrongSegmentCount(2)));
    }

    #[test]
    fn unknown_single_segment_errors() {
        let e = "bogus".parse::<TargetTriple>().unwrap_err();
        match e {
            ParseError::UnknownSpecial(s) => assert_eq!(s, "bogus"),
            other => panic!("expected UnknownSpecial, got {other:?}"),
        }
    }

    #[test]
    fn unknown_platform_errors() {
        let e = "bsd-native-strict".parse::<TargetTriple>().unwrap_err();
        match e {
            ParseError::UnknownPlatform(s) => assert_eq!(s, "bsd"),
            other => panic!("expected UnknownPlatform, got {other:?}"),
        }
    }

    #[test]
    fn unknown_adapter_family_errors() {
        let e = "linux-bogus-strict".parse::<TargetTriple>().unwrap_err();
        match e {
            ParseError::UnknownAdapterFamily(s) => assert_eq!(s, "bogus"),
            other => panic!("expected UnknownAdapterFamily, got {other:?}"),
        }
    }

    #[test]
    fn unknown_tier_errors() {
        let e = "linux-native-bogus".parse::<TargetTriple>().unwrap_err();
        match e {
            ParseError::UnknownTier(s) => assert_eq!(s, "bogus"),
            other => panic!("expected UnknownTier, got {other:?}"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trips_via_string() {
        let t = TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Container,
            tier: CapabilityTier::Strict,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"linux-container-strict\"");
        let parsed: TargetTriple = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, t);
    }

    #[test]
    fn host_returns_native_strict_for_current_platform() {
        let h = TargetTriple::host();
        assert_eq!(h.adapter_family.as_str(), "native");
        assert_eq!(h.tier, crate::capability_gate::CapabilityTier::Strict);
        let expected_platform = if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            panic!("unsupported host platform — extend host() before testing");
        };
        assert_eq!(h.platform.as_str(), expected_platform);
    }
}
