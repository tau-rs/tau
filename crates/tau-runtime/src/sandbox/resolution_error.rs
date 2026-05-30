//! Error types for sandbox adapter resolution.
//!
//! [`ResolutionError`] is the failure type returned by the resolver
//! ([`crate::sandbox::resolver`]) when no adapter can satisfy the project's
//! requirements.  [`ResolutionRejection`] records the per-adapter rejection
//! reason collected during the resolver's filtering pass so that callers can
//! render guided multi-option error messages (Task 8).
//!
//! # Usage
//!
//! The resolver walks [`crate::sandbox::registry::REGISTRY`], filters each
//! adapter by platform / probe / tier / shape / plugin-tier-floor, and
//! accumulates `(adapter_name, ResolutionRejection)` pairs for every adapter
//! that does *not* pass.  When no adapter survives all filters the resolver
//! returns `Err(ResolutionError::NoAdapterMatches { tried, .. })`.  A single
//! plugin-tier-floor failure on the selected adapter surfaces as
//! `ResolutionError::PluginTierMismatch` instead — the dedicated variant makes
//! the error renderer cleaner for that narrow case.

use tau_domain::CapabilityShapeSet;
use tau_domain::PluginRequiredTier;
use tau_ports::CapabilityTier;
use thiserror::Error;

// ---------------------------------------------------------------------------
// ResolutionRejection
// ---------------------------------------------------------------------------

/// Why a single adapter was rejected during resolution filtering.
///
/// Carried inside [`ResolutionError::NoAdapterMatches::tried`] — one entry per
/// adapter that was considered but did not pass *all* filters.
///
/// `#[non_exhaustive]`: future filter stages may add new rejection reasons
/// without breaking existing match arms.
///
/// # Example
///
/// ```
/// use tau_runtime::sandbox::resolution_error::ResolutionRejection;
///
/// let r = ResolutionRejection::PlatformMismatch;
/// assert!(r.to_string().contains("platform"));
///
/// let r = ResolutionRejection::ProbeUnavailable("docker not found".into());
/// assert!(r.to_string().contains("probe failed"));
/// assert!(r.to_string().contains("docker not found"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Error)]
pub enum ResolutionRejection {
    /// Adapter does not apply to this platform.
    #[error("adapter does not apply to this platform")]
    PlatformMismatch,

    /// Adapter probed `tau_ports::ProbeOutcome::Unavailable`; the inner
    /// `String` is the human-readable reason returned by the probe.
    #[error("adapter probe failed: {0}")]
    ProbeUnavailable(String),

    /// Adapter delivers a lower tier than the project requires.
    #[error("adapter delivers tier {delivered:?}; need at least {required:?}")]
    TierTooLow {
        /// The highest tier this adapter can deliver.
        delivered: CapabilityTier,
        /// The tier the project (or plugin) requires.
        required: CapabilityTier,
    },

    /// Required capability shapes are not all in the adapter's supported set.
    /// The field is the *set of missing shapes* (required minus supported).
    #[error("adapter does not support shapes: {missing:?}")]
    ShapesUnsupported {
        /// Shapes that were required but are not supported by this adapter.
        missing: CapabilityShapeSet,
    },

    /// At least one plugin's `required_tier` exceeds this adapter's delivered
    /// tier.  Used when encoding plugin-tier failures inside a multi-adapter
    /// no-match scenario; for a single-plugin failure on the *selected*
    /// adapter, use [`ResolutionError::PluginTierMismatch`] instead.
    #[error("plugin '{plugin}' requires tier {required:?}; this adapter cannot deliver that")]
    PluginTierTooLow {
        /// The plugin whose `required_tier` caused rejection.
        plugin: String,
        /// The tier the plugin requires.
        required: PluginRequiredTier,
    },
}

// ---------------------------------------------------------------------------
// ResolutionError
// ---------------------------------------------------------------------------

/// Failure type returned by the sandbox adapter resolver.
///
/// `#[non_exhaustive]`: future resolution stages (e.g., remote-sandbox
/// negotiation) may introduce new failure modes without breaking existing
/// match arms.
///
/// # Example
///
/// ```
/// use tau_runtime::sandbox::resolution_error::{ResolutionError, ResolutionRejection};
/// use tau_ports::CapabilityTier;
///
/// let e = ResolutionError::NoAdapterMatches {
///     tried: vec![("native".into(), ResolutionRejection::PlatformMismatch)],
///     platform: "linux".into(),
///     required_tier: CapabilityTier::Strict,
/// };
/// let s = e.to_string();
/// assert!(s.contains("1"));
/// assert!(s.contains("linux"));
/// assert!(s.contains("Strict"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Error)]
pub enum ResolutionError {
    /// No adapter passed all filters.
    ///
    /// `tried` contains one entry per adapter that was considered; each entry
    /// records the adapter name and the *first* filter stage that rejected it
    /// (earlier stages short-circuit later ones).
    #[error(
        "no sandbox adapter satisfies project requirements \
        (tried {n} adapters; platform={platform}; required_tier={required_tier:?})",
        n = tried.len()
    )]
    NoAdapterMatches {
        /// `(adapter_name, rejection_reason)` for each adapter tried.
        tried: Vec<(String, ResolutionRejection)>,
        /// Platform string at resolution time (e.g., `"macos"`, `"linux"`).
        platform: String,
        /// The minimum tier the project required.
        required_tier: CapabilityTier,
    },

    /// A plugin's `required_tier` exceeds the selected adapter's delivered
    /// tier.  Dedicated variant (vs. encoding inside `tried`) to make the
    /// error renderer cleaner for the single-plugin-failure case.
    #[error(
        "plugin '{plugin}' requires tier {required:?}; \
        selected adapter delivers {delivered:?}"
    )]
    PluginTierMismatch {
        /// The plugin whose `required_tier` caused the mismatch.
        plugin: String,
        /// The tier the plugin requires.
        required: PluginRequiredTier,
        /// The tier the selected adapter actually delivers.
        delivered: CapabilityTier,
    },

    /// Generic catch-all for malformed configuration not covered by the above.
    #[error("sandbox configuration error: {message}")]
    ConfigError {
        /// Human-readable description of what was malformed.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::PluginRequiredTier;
    use tau_ports::CapabilityTier;

    // 1. NoAdapterMatches display includes the count, platform, and required tier.
    #[test]
    fn no_adapter_matches_display_includes_count() {
        let e = ResolutionError::NoAdapterMatches {
            tried: vec![
                ("native".into(), ResolutionRejection::PlatformMismatch),
                (
                    "container".into(),
                    ResolutionRejection::ProbeUnavailable("docker not on PATH".into()),
                ),
            ],
            platform: "macos".into(),
            required_tier: CapabilityTier::Strict,
        };
        let s = format!("{e}");
        assert!(s.contains('2'), "expected count '2' in: {s}");
        assert!(s.contains("macos"), "expected 'macos' in: {s}");
        assert!(s.contains("Strict"), "expected 'Strict' in: {s}");
    }

    // 2. PluginTierMismatch display contains plugin name, required tier, and delivered tier.
    #[test]
    fn plugin_tier_mismatch_display_format() {
        let e = ResolutionError::PluginTierMismatch {
            plugin: "credentials".into(),
            required: PluginRequiredTier::Strict,
            delivered: CapabilityTier::None,
        };
        let s = format!("{e}");
        assert!(s.contains("credentials"), "expected 'credentials' in: {s}");
        assert!(s.contains("Strict"), "expected 'Strict' in: {s}");
        assert!(s.contains("None"), "expected 'None' in: {s}");
    }

    // 3. ConfigError renders the message and the "configuration error" prefix.
    #[test]
    fn config_error_renders_message() {
        let e = ResolutionError::ConfigError {
            message: "weird".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("weird"), "expected 'weird' in: {s}");
        assert!(
            s.contains("configuration error"),
            "expected 'configuration error' in: {s}"
        );
    }

    // 4. All ResolutionRejection variants construct, format without panicking,
    //    and produce expected substrings.
    #[test]
    fn rejection_variants_construct_and_format() {
        // PlatformMismatch
        let r = ResolutionRejection::PlatformMismatch;
        let s = format!("{r}");
        assert!(
            s.contains("platform"),
            "PlatformMismatch: expected 'platform' in: {s}"
        );

        // ProbeUnavailable
        let r = ResolutionRejection::ProbeUnavailable("docker not on PATH".into());
        let s = format!("{r}");
        assert!(
            s.contains("probe failed"),
            "ProbeUnavailable: expected 'probe failed' in: {s}"
        );
        assert!(
            s.contains("docker not on PATH"),
            "ProbeUnavailable: expected reason in: {s}"
        );

        // TierTooLow
        let r = ResolutionRejection::TierTooLow {
            delivered: CapabilityTier::None,
            required: CapabilityTier::Strict,
        };
        let s = format!("{r}");
        assert!(s.contains("None"), "TierTooLow: expected 'None' in: {s}");
        assert!(
            s.contains("Strict"),
            "TierTooLow: expected 'Strict' in: {s}"
        );

        // ShapesUnsupported
        let mut missing = CapabilityShapeSet::new();
        missing.insert(tau_domain::CapabilityShape::NetworkHttp);
        let r = ResolutionRejection::ShapesUnsupported { missing };
        let s = format!("{r}");
        assert!(
            s.contains("shapes"),
            "ShapesUnsupported: expected 'shapes' in: {s}"
        );

        // PluginTierTooLow
        let r = ResolutionRejection::PluginTierTooLow {
            plugin: "credentials".into(),
            required: PluginRequiredTier::Strict,
        };
        let s = format!("{r}");
        assert!(
            s.contains("credentials"),
            "PluginTierTooLow: expected 'credentials' in: {s}"
        );
        assert!(
            s.contains("Strict"),
            "PluginTierTooLow: expected 'Strict' in: {s}"
        );
        assert!(
            s.contains("adapter cannot deliver"),
            "PluginTierTooLow: expected 'adapter cannot deliver' in: {s}"
        );
    }

    // 5. ResolutionError implements std::error::Error.
    #[test]
    fn error_implements_std_error() {
        fn takes_error<E: std::error::Error>(_: &E) {}

        let e = ResolutionError::ConfigError {
            message: "test".into(),
        };
        takes_error(&e);

        // .source() should return None for variants without a #[source] field.
        use std::error::Error;
        assert!(e.source().is_none());
    }
}
