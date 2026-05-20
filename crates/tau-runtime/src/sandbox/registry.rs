//! Internal adapter registry — not user-facing config.
//!
//! Each registered adapter declares: kind, supported platforms, supported
//! tiers, supported shapes, priority, and a constructor function. The
//! resolver ([`crate::sandbox::resolver::resolve_adapter`]) walks the
//! registry, filters by detected platform / probe / tier / shape /
//! plugin-tier-floor, and picks the highest-priority survivor.
//!
//! New adapters are added via tau's source code (or, in Phase 2, via the
//! tau target triple registry sub-project); users do NOT write registry
//! entries.
//!
//! # Note on shapes
//!
//! `CapabilityShape` contains a `Custom { name: String }` variant, so it is
//! not `Copy`. Static `&[CapabilityShape]` slices are therefore not
//! constructible. Each `AdapterRegistration` carries a
//! `shapes_supported_fn: fn() -> CapabilityShapeSet` instead; callers invoke
//! the function to materialise the set.

use tau_domain::CapabilityShape;
use tau_domain::CapabilityShapeSet;
use tau_ports::SandboxTier;

/// Set of platforms an adapter applies to.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSet {
    /// Linux only (e.g., `tau-sandbox-native` Linux landlock/seccomp path).
    LinuxOnly,
    /// Linux and macOS/Darwin. Used for the `Native` adapter, which
    /// routes to `NativeSandbox` on Linux and `DarwinSandbox` on macOS —
    /// both are v1-available. Windows is excluded because the Windows
    /// AppContainer path is Reserved in v1.
    LinuxAndDarwin,
    /// Linux, macOS, and Windows (e.g., container adapter requires
    /// docker/podman binary; the binary may or may not be present, but
    /// the adapter could in principle work on any of these).
    Multi,
    /// Any platform.
    Any,
}

impl PlatformSet {
    /// Does this set include the given platform name (`"linux"`,
    /// `"macos"`, `"windows"`)?
    pub fn includes(&self, platform: &str) -> bool {
        match self {
            PlatformSet::Any => true,
            PlatformSet::Multi => {
                matches!(platform, "linux" | "macos" | "windows")
            }
            PlatformSet::LinuxAndDarwin => matches!(platform, "linux" | "macos"),
            PlatformSet::LinuxOnly => platform == "linux",
        }
    }
}

/// Detect the current platform name.
pub fn detect_platform() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown"
    }
}

/// Opaque adapter kind identifier in the registry. Each value
/// corresponds to one adapter family (Native, Container, Remote,
/// Passthrough). Internal — users never write these.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistryKind {
    /// Linux landlock + seccomp + namespaces (`tau-sandbox-native`).
    Native,
    /// docker / podman shell-out (`tau-sandbox-container`).
    Container,
    /// Remote sandbox (Vercel Sandbox / Sandcastle / etc). Phase 2.
    Remote,
    /// No isolation; explicit opt-out (this crate's `passthrough` module).
    Passthrough,
}

impl RegistryKind {
    /// Adapter name as surfaced in logs and error messages.
    pub fn name(&self) -> &'static str {
        match self {
            RegistryKind::Native => "native",
            RegistryKind::Container => "container",
            RegistryKind::Remote => "remote",
            RegistryKind::Passthrough => "passthrough",
        }
    }
}

/// One entry in the adapter registry.
///
/// Because [`tau_domain::CapabilityShape`] contains `Custom { name: String }`,
/// it is not `Copy`; static slices of shapes are therefore not constructible.
/// Shapes are exposed via `shapes_supported_fn`, a plain function pointer that
/// returns a fresh [`CapabilityShapeSet`] on each call.
#[non_exhaustive]
#[derive(Clone)]
pub struct AdapterRegistration {
    /// Adapter kind.
    pub kind: RegistryKind,
    /// Which platforms this adapter applies to.
    pub platforms: PlatformSet,
    /// Tiers this adapter can deliver.
    pub tiers_supported: &'static [SandboxTier],
    /// Returns the set of shapes this adapter can enforce.
    pub shapes_supported_fn: fn() -> CapabilityShapeSet,
    /// Priority for tie-breaking (higher = preferred when multiple
    /// candidates pass filtering).
    pub priority: u32,
}

impl std::fmt::Debug for AdapterRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterRegistration")
            .field("kind", &self.kind)
            .field("platforms", &self.platforms)
            .field("tiers_supported", &self.tiers_supported)
            .field("priority", &self.priority)
            .finish()
    }
}

impl PartialEq for AdapterRegistration {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.platforms == other.platforms
            && self.tiers_supported == other.tiers_supported
            && self.priority == other.priority
            && (self.shapes_supported_fn as usize) == (other.shapes_supported_fn as usize)
    }
}

impl Eq for AdapterRegistration {}

// ---------------------------------------------------------------------------
// Shape constructor functions
// ---------------------------------------------------------------------------

fn all_shapes() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
    s.insert(CapabilityShape::ProcessExec);
    s.insert(CapabilityShape::NetworkHttp);
    s.insert(CapabilityShape::AgentSpawn);
    s
}

fn fs_and_exec_and_net() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
    s.insert(CapabilityShape::ProcessExec);
    s.insert(CapabilityShape::NetworkHttp);
    s
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// The registry. Static; populated at compile time. Users do NOT modify
/// this; new adapters are added via tau's source code.
pub static REGISTRY: &[AdapterRegistration] = &[
    AdapterRegistration {
        kind: RegistryKind::Native,
        platforms: PlatformSet::LinuxAndDarwin,
        tiers_supported: &[SandboxTier::Light, SandboxTier::Strict],
        shapes_supported_fn: fs_and_exec_and_net,
        priority: 100,
    },
    AdapterRegistration {
        kind: RegistryKind::Container,
        platforms: PlatformSet::Multi,
        tiers_supported: &[SandboxTier::Strict],
        shapes_supported_fn: fs_and_exec_and_net,
        priority: 50,
    },
    AdapterRegistration {
        kind: RegistryKind::Remote,
        platforms: PlatformSet::Any,
        tiers_supported: &[SandboxTier::Strict],
        shapes_supported_fn: fs_and_exec_and_net,
        priority: 25,
    },
    AdapterRegistration {
        kind: RegistryKind::Passthrough,
        platforms: PlatformSet::Any,
        tiers_supported: &[SandboxTier::None],
        shapes_supported_fn: all_shapes,
        priority: 0,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_four_entries() {
        assert_eq!(REGISTRY.len(), 4);
    }

    #[test]
    fn registry_kinds_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in REGISTRY {
            assert!(
                seen.insert(entry.kind),
                "duplicate kind {:?} in registry",
                entry.kind
            );
        }
    }

    #[test]
    fn priority_ordering_native_first_passthrough_last() {
        let native = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Native)
            .unwrap();
        let passthrough = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Passthrough)
            .unwrap();
        assert!(native.priority > passthrough.priority);
    }

    #[test]
    fn native_is_linux_and_darwin() {
        // Native adapter covers Linux (NativeSandbox) and macOS/Darwin
        // (DarwinSandbox). Windows is excluded — AppContainer is Reserved in v1.
        let native = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Native)
            .unwrap();
        assert!(native.platforms.includes("linux"));
        assert!(native.platforms.includes("macos"));
        assert!(!native.platforms.includes("windows"));
    }

    #[test]
    fn container_is_multi_platform() {
        let c = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Container)
            .unwrap();
        assert!(c.platforms.includes("linux"));
        assert!(c.platforms.includes("macos"));
        assert!(c.platforms.includes("windows"));
    }

    #[test]
    fn passthrough_supports_all_shapes() {
        let p = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Passthrough)
            .unwrap();
        let set = (p.shapes_supported_fn)();
        assert!(set.contains(&CapabilityShape::FilesystemRead));
        assert!(set.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn passthrough_only_delivers_tier_none() {
        let p = REGISTRY
            .iter()
            .find(|r| r.kind == RegistryKind::Passthrough)
            .unwrap();
        assert_eq!(p.tiers_supported, &[SandboxTier::None]);
    }

    #[test]
    fn detect_platform_returns_known() {
        let p = detect_platform();
        assert!(matches!(p, "linux" | "macos" | "windows" | "unknown"));
    }

    #[test]
    fn registry_kind_names_are_lowercase_kebab() {
        assert_eq!(RegistryKind::Native.name(), "native");
        assert_eq!(RegistryKind::Container.name(), "container");
        assert_eq!(RegistryKind::Remote.name(), "remote");
        assert_eq!(RegistryKind::Passthrough.name(), "passthrough");
    }
}
