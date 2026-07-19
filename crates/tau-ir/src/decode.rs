//! Version-gated decode of canonical IR bytes.
//!
//! Two phases: peek `ir_format` and apply the semver acceptance window
//! (accept ⟺ major == CURRENT.major ∧ minor ≤ CURRENT.minor), then a full
//! decode. The full decode is closed: every `Deserialize` type reachable
//! from [`IrModule`] carries `#[serde(deny_unknown_fields)]`, so an
//! unknown field inside an otherwise-accepted module is rejected as
//! [`DecodeError::Serde`].

use alloc::string::{String, ToString};
use serde::Deserialize;

use crate::module::{IrFormatVersion, IrModule};

/// Errors from [`from_canonical_bytes`].
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The module's `ir_format` is a newer minor than this `tau` reads.
    #[error("bundle uses ir_format {found}; this tau reads up to {supported_up_to}")]
    FormatTooNew {
        /// The module's declared `ir_format` (e.g. `v2.5.0`).
        found: String,
        /// Highest minor this `tau` accepts, rendered `MAJOR.MINOR.x`.
        supported_up_to: String,
    },
    /// The module's `ir_format` major differs from this `tau`'s.
    #[error("bundle uses ir_format {found}; this tau is a different major ({current})")]
    FormatMajorMismatch {
        /// The module's declared `ir_format`.
        found: String,
        /// This `tau`'s `ir_format` (`IrFormatVersion::CURRENT`).
        current: String,
    },
    /// The `ir_format` string is missing or not `vMAJOR.MINOR.PATCH`.
    #[error("ir_format {found:?} is missing or unparseable: {detail}")]
    BadFormat {
        /// The offending value (empty if absent).
        found: String,
        /// Why it could not be parsed.
        detail: String,
    },
    /// A serde-level decode failure (malformed JSON, wrong shape, etc.).
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

/// Minimal partial-decode struct: peek ONLY `ir_format`. No
/// `deny_unknown_fields` here, so unknown fields from a newer minor do not
/// mask the version error. `ir_format` is `Option` so a totally-absent key
/// is reported as [`DecodeError::BadFormat`] instead of a generic serde
/// "missing field" error.
#[derive(Deserialize)]
struct FormatPeek {
    ir_format: Option<IrFormatVersion>,
}

/// Parse `vMAJOR.MINOR.PATCH` → `(major, minor, patch)`. Tolerates a missing
/// `v` prefix. Rejects extra dotted segments.
fn parse_semver(s: &str) -> Result<(u64, u64, u64), ()> {
    let body = s.strip_prefix('v').unwrap_or(s);
    let mut parts = body.split('.');
    let major: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let minor: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let patch: u64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    if parts.next().is_some() {
        return Err(());
    }
    Ok((major, minor, patch))
}

/// Deserialize canonical bytes to an [`IrModule`], enforcing the `ir_format`
/// acceptance window, then decoding the full module.
pub fn from_canonical_bytes(bytes: &[u8]) -> Result<IrModule, DecodeError> {
    // Phase 1: peek ir_format only.
    let peek: FormatPeek = serde_json::from_slice(bytes)?;
    let found = match peek.ir_format {
        Some(v) => v.0,
        None => {
            return Err(DecodeError::BadFormat {
                found: alloc::string::String::new(),
                detail: "missing ir_format field".into(),
            })
        }
    };
    let current = IrFormatVersion::CURRENT;

    let (fmaj, fmin, _) = parse_semver(&found).map_err(|_| DecodeError::BadFormat {
        found: found.clone(),
        detail: "expected vMAJOR.MINOR.PATCH".to_string(),
    })?;
    let (cmaj, cmin, _) = parse_semver(current).expect("CURRENT is well-formed");

    // Phase 2: acceptance window.
    if fmaj != cmaj {
        return Err(DecodeError::FormatMajorMismatch {
            found,
            current: current.to_string(),
        });
    }
    if fmin > cmin {
        return Err(DecodeError::FormatTooNew {
            found,
            supported_up_to: alloc::format!("{cmaj}.{cmin}.x"),
        });
    }

    // Phase 3: full decode. Closed via `deny_unknown_fields` on every type
    // reachable from `IrModule` — an unknown field anywhere in the tree
    // surfaces here as `DecodeError::Serde`.
    let module: IrModule = serde_json::from_slice(bytes)?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::to_canonical_bytes;
    use crate::module::{IrFormatVersion, IrModule, Workflow};
    use tau_ports::target::registry;

    fn module_at(version: &str) -> alloc::vec::Vec<u8> {
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion(version.into()),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        };
        to_canonical_bytes(&m)
    }

    #[test]
    fn equal_minor_decodes() {
        let bytes = module_at("v2.4.0");
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn lower_minor_decodes() {
        let bytes = module_at("v2.3.0");
        assert!(from_canonical_bytes(&bytes).is_ok());
    }

    #[test]
    fn newer_minor_is_too_new() {
        let bytes = module_at("v2.5.0");
        match from_canonical_bytes(&bytes) {
            Err(DecodeError::FormatTooNew {
                found,
                supported_up_to,
            }) => {
                assert_eq!(found, "v2.5.0");
                assert_eq!(supported_up_to, "2.4.x");
            }
            other => panic!("expected FormatTooNew, got {other:?}"),
        }
    }

    #[test]
    fn newer_major_is_mismatch() {
        let bytes = module_at("v3.0.0");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::FormatMajorMismatch { .. })
        ));
    }

    #[test]
    fn lower_major_is_mismatch() {
        let bytes = module_at("v1.9.0");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::FormatMajorMismatch { .. })
        ));
    }

    #[test]
    fn malformed_version_is_bad_format() {
        let bytes = module_at("banana");
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::BadFormat { .. })
        ));
    }

    #[test]
    fn missing_ir_format_field_is_bad_format() {
        let bytes = module_at("v2.4.0");
        let json = alloc::string::String::from_utf8(bytes).unwrap();
        let stripped = json.replace("\"ir_format\":\"v2.4.0\",", "");
        assert_ne!(
            stripped, json,
            "expected ir_format key to be present and removable"
        );
        assert!(matches!(
            from_canonical_bytes(stripped.as_bytes()),
            Err(DecodeError::BadFormat { .. })
        ));
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        let mut bytes = module_at("v2.4.0");
        // Splice an unknown top-level key into the JSON object.
        let json = String::from_utf8(bytes).unwrap();
        let doctored = json.replacen('{', r#"{"bogus_top":1,"#, 1);
        bytes = doctored.into_bytes();
        assert!(matches!(
            from_canonical_bytes(&bytes),
            Err(DecodeError::Serde(_))
        ));
    }

    #[test]
    fn unknown_nested_field_is_rejected() {
        // Build a module with a pipeline, then inject an unknown key inside
        // the nested "workflow" object.
        let target = registry::list_available().next().unwrap().triple;
        let m = IrModule {
            ir_format: IrFormatVersion::current(),
            tau_version: "0.0.0".into(),
            target,
            workflow: Workflow::default(),
            triggers: alloc::vec::Vec::new(),
        };
        let json = String::from_utf8(to_canonical_bytes(&m)).unwrap();
        let doctored = json.replace(r#""workflow":{"#, r#""workflow":{"ghost":true,"#);
        assert!(matches!(
            from_canonical_bytes(doctored.as_bytes()),
            Err(DecodeError::Serde(_))
        ));
    }

    #[test]
    fn all_known_fields_still_decode() {
        // The canonical bytes of a fully-populated module must still round-trip.
        let bytes = module_at("v2.4.0");
        assert!(from_canonical_bytes(&bytes).is_ok());
    }
}
