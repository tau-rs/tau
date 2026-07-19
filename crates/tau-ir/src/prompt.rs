//! Where an agent's system prompt comes from (D6-B).
//!
//! Before v2.5.0 an `Agent.prompt` was a bare `String`. That conflated two
//! very different things: a literal inline prompt, and — for `system_file`
//! agents — the *path* the prompt was read from. Lowering stored the path
//! string and the interpreter sent it verbatim as the system prompt, so a
//! `system_file` agent ran with a file path as its prompt and the bundle hash
//! pinned the path, not the content (non-hermetic).
//!
//! `PromptSource` replaces that conflation with an explicit source:
//! - `Inline(String)` — a literal prompt. Serializes as a **bare string**, so
//!   every pre-2.5 module (whose `prompt` was a bare string) reads back as
//!   `Inline` unchanged and re-serializes byte-identically. This is the
//!   additive-compat property that lets `ir_format` bump `v2.4.0 -> v2.5.0`
//!   under the #442 policy.
//! - `Asset { asset }` — a content-addressed reference (`"sha256:<64 hex>"`)
//!   into the bundle's asset store. File-backed prompts lower to this: the
//!   bytes are read at build time, hashed, and stored as an asset. Paths never
//!   enter the IR.

use alloc::string::String;
// The schemars `pattern(...)` derive expands to a `.to_string()` call on the
// pattern literal; bring `ToString` into scope for the `schema` build only.
#[cfg(feature = "schema")]
use alloc::string::ToString;
use serde::{Deserialize, Serialize};

/// Where an agent's system prompt comes from.
///
/// `#[serde(untagged)]` keeps `Inline` on the wire as a bare string —
/// byte-identical to the pre-2.5 `prompt: String` field.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PromptSource {
    /// A literal system prompt. On the wire: a bare JSON string.
    Inline(String),
    /// A content-addressed reference into the bundle's asset store.
    /// On the wire: `{"asset":"sha256:<64 hex>"}`.
    Asset(AssetRef),
}

/// A content-addressed reference to a bundle asset.
///
/// A distinct `deny_unknown_fields` struct (rather than an inline
/// `Asset { asset }` variant) so the generated JSON Schema emits
/// `additionalProperties: false` and rejects stray keys in the asset object.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AssetRef {
    /// The asset hash, `"sha256:" + 64 lowercase hex`.
    #[cfg_attr(feature = "schema", schemars(pattern(r"^sha256:[0-9a-f]{64}$")))]
    pub asset: String,
}

impl PromptSource {
    /// Construct an inline prompt.
    pub fn inline(s: impl Into<String>) -> Self {
        Self::Inline(s.into())
    }

    /// Construct an asset-backed prompt from a hash string.
    ///
    /// Does not validate the hash; use [`PromptSource::validate`] (or
    /// [`is_well_formed_asset_hash`]) for that.
    pub fn asset(hash: impl Into<String>) -> Self {
        Self::Asset(AssetRef { asset: hash.into() })
    }

    /// The inline prompt text, if this is an `Inline` source.
    pub fn as_inline(&self) -> Option<&str> {
        match self {
            Self::Inline(s) => Some(s),
            Self::Asset(_) => None,
        }
    }

    /// The asset hash, if this is an `Asset` source.
    pub fn asset_hash(&self) -> Option<&str> {
        match self {
            Self::Asset(a) => Some(&a.asset),
            Self::Inline(_) => None,
        }
    }

    /// Validate that an `Asset` reference is well-formed. `Inline` always
    /// passes. Returns the malformed hash string on failure so callers can
    /// build a structured error (never `IrError::Parse`).
    pub fn validate(&self) -> Result<(), &str> {
        match self {
            Self::Inline(_) => Ok(()),
            Self::Asset(a) => {
                if is_well_formed_asset_hash(&a.asset) {
                    Ok(())
                } else {
                    Err(&a.asset)
                }
            }
        }
    }
}

impl From<String> for PromptSource {
    fn from(s: String) -> Self {
        Self::Inline(s)
    }
}

impl From<&str> for PromptSource {
    fn from(s: &str) -> Self {
        Self::Inline(s.into())
    }
}

/// True iff `s` is `"sha256:"` followed by exactly 64 lowercase hex digits.
pub fn is_well_formed_asset_hash(s: &str) -> bool {
    match s.strip_prefix("sha256:") {
        Some(hex) => {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_HASH: &str =
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// `Inline` serializes as a BARE STRING — byte-identical to the pre-2.5
    /// `prompt: String` wire form.
    #[test]
    fn inline_serializes_as_bare_string() {
        let p = PromptSource::inline("You are a careful assistant.");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"You are a careful assistant.\"");
    }

    /// A bare JSON string deserializes back into `Inline` (pre-2.5 modules).
    #[test]
    fn bare_string_deserializes_as_inline() {
        let p: PromptSource = serde_json::from_str("\"hello\"").unwrap();
        assert_eq!(p, PromptSource::inline("hello"));
    }

    /// `Asset` serializes as `{"asset":"sha256:..."}` and round-trips.
    #[test]
    fn asset_round_trips_as_object() {
        let p = PromptSource::asset(GOOD_HASH);
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, alloc::format!("{{\"asset\":\"{GOOD_HASH}\"}}"));
        let back: PromptSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    /// An asset object with a stray key is rejected (deny_unknown_fields).
    #[test]
    fn asset_with_unknown_key_is_rejected() {
        let raw = alloc::format!("{{\"asset\":\"{GOOD_HASH}\",\"extra\":1}}");
        let parsed: Result<PromptSource, _> = serde_json::from_str(&raw);
        assert!(
            parsed.is_err(),
            "unknown key in asset object must be rejected"
        );
    }

    #[test]
    fn hash_wellformedness() {
        assert!(is_well_formed_asset_hash(GOOD_HASH));
        // wrong prefix
        assert!(!is_well_formed_asset_hash(
            "md5:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // too short
        assert!(!is_well_formed_asset_hash("sha256:abcd"));
        // uppercase hex not allowed
        assert!(!is_well_formed_asset_hash(
            "sha256:E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
        // non-hex char
        assert!(!is_well_formed_asset_hash(
            "sha256:g3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // no prefix at all
        assert!(!is_well_formed_asset_hash("deadbeef"));
    }

    #[test]
    fn validate_reports_malformed_hash() {
        assert!(PromptSource::inline("x").validate().is_ok());
        assert!(PromptSource::asset(GOOD_HASH).validate().is_ok());
        assert_eq!(
            PromptSource::asset("sha256:nope").validate(),
            Err("sha256:nope")
        );
    }

    #[test]
    fn accessors() {
        assert_eq!(PromptSource::inline("hi").as_inline(), Some("hi"));
        assert_eq!(PromptSource::inline("hi").asset_hash(), None);
        assert_eq!(PromptSource::asset(GOOD_HASH).as_inline(), None);
        assert_eq!(PromptSource::asset(GOOD_HASH).asset_hash(), Some(GOOD_HASH));
    }
}
