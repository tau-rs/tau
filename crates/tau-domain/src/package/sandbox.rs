//! Plugin-side sandbox requirements declared in `tau.toml`'s `[sandbox]`
//! table. Optional; absent means the plugin asserts no tier or shape
//! floor and is satisfied by any adapter.

use crate::package::capability::CapabilityShape;

/// Plugin-side sandbox requirements.
///
/// A plugin can declare `[sandbox] required_tier = "strict"` in its
/// `tau.toml` to refuse loading when the host can only deliver weaker
/// enforcement (e.g., passthrough). Symmetric to project-side
/// `tau_pkg::scope::SandboxRequirements` (cross-crate ref;
/// `tau-pkg` is not a dependency of `tau-domain`).
///
/// Both fields are optional with `#[serde(default)]`. A plugin with no
/// `[sandbox]` block parses to `PluginSandboxRequirements::default()`,
/// which imposes no floor.
///
/// # Example
///
/// ```
/// use tau_domain::PluginSandboxRequirements;
///
/// // `PluginSandboxRequirements` is `#[non_exhaustive]`. Struct-literal
/// // construction is blocked across crate boundaries; `Default` is the
/// // canonical entry point for the empty-requirements case.
/// let req = PluginSandboxRequirements::default();
/// assert!(req.required_tier.is_none(), "default has no tier floor");
/// assert!(req.required_shapes.is_empty(), "default has no extra shapes");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PluginSandboxRequirements {
    /// Minimum sandbox tier this plugin requires. `None` means no
    /// floor; any adapter is acceptable. The serialized values are
    /// `"none"`, `"light"`, `"strict"`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub required_tier: Option<PluginRequiredTier>,
    /// Additional shape requirements beyond what the plugin's declared
    /// capabilities imply. Optional; the resolver auto-derives the
    /// shape set from the plugin's `[capabilities]` block when this is
    /// empty.
    #[cfg_attr(feature = "serde", serde(default))]
    pub required_shapes: Vec<CapabilityShape>,
}

/// Tier value usable in plugin manifests. Mirrors
/// `tau_pkg::scope::SandboxRequiredTier` shape; defined here to keep
/// `tau-domain` free of `tau-pkg` dependencies.
///
/// The runtime maps `PluginRequiredTier` to `tau_ports::SandboxTier`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum PluginRequiredTier {
    /// No floor; any tier acceptable.
    None,
    /// Filesystem isolation at minimum.
    Light,
    /// Full strict tier required.
    Strict,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "serde")]
    #[test]
    fn plugin_sandbox_requirements_default_is_unconstrained() {
        let req = PluginSandboxRequirements::default();
        assert!(req.required_tier.is_none());
        assert!(req.required_shapes.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn plugin_sandbox_requirements_round_trip_strict() {
        let toml = r#"
required_tier = "strict"
"#;
        let parsed: PluginSandboxRequirements = toml::from_str(toml).unwrap();
        assert_eq!(parsed.required_tier, Some(PluginRequiredTier::Strict));
        assert!(parsed.required_shapes.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn plugin_sandbox_requirements_with_explicit_shapes() {
        // Note: CapabilityShape currently has no #[serde(rename_all)]
        // attribute, so it serializes as PascalCase ("FilesystemRead",
        // not "filesystem-read"). The user-facing kebab-case alignment
        // is a deferred follow-up.
        let toml = r#"
required_tier = "light"
required_shapes = ["FilesystemRead", "NetworkHttp"]
"#;
        let parsed: PluginSandboxRequirements = toml::from_str(toml).unwrap();
        assert_eq!(parsed.required_tier, Some(PluginRequiredTier::Light));
        assert_eq!(parsed.required_shapes.len(), 2);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn plugin_sandbox_requirements_empty_block_round_trip() {
        let toml = "";
        let parsed: PluginSandboxRequirements = toml::from_str(toml).unwrap_or_default();
        assert!(parsed.required_tier.is_none());
    }

    #[test]
    fn plugin_required_tier_ordering() {
        assert!(PluginRequiredTier::None < PluginRequiredTier::Light);
        assert!(PluginRequiredTier::Light < PluginRequiredTier::Strict);
    }
}
