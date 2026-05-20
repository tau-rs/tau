//! v1 target triple registry. See spec §4.

use tau_domain::{CapabilityShape, CapabilityShapeSet};

use crate::sandbox::SandboxTier;
use crate::target::adapter_family::AdapterFamily;
use crate::target::platform::Platform;
use crate::target::profile::{TargetCapabilityProfile, TripleStatus};
use crate::target::triple::TargetTriple;

/// One entry in the static registry. The shape set is materialised on
/// demand via a function pointer because `CapabilityShapeSet` cannot
/// be `const`-constructed (its `Custom { name: String }` variant
/// requires a heap allocation).
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct TargetTripleEntry {
    /// The target triple.
    pub triple: TargetTriple,
    /// Constructor for the set of capability shapes this target enforces.
    pub shapes_fn: fn() -> CapabilityShapeSet,
    /// Whether this triple ships in v1 or is reserved for future work.
    pub status: TripleStatus,
}

impl TargetTripleEntry {
    /// Materialise the full profile, allocating the shape set.
    pub fn profile(&self) -> TargetCapabilityProfile {
        TargetCapabilityProfile {
            triple: self.triple,
            required_shapes: (self.shapes_fn)(),
            status: self.status,
        }
    }
}

// ---------------------------------------------------------------------------
// Shape constructors. Hand-written to keep the registry `const`-friendly.
// ---------------------------------------------------------------------------

fn fs_rw_exec_net() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
    s.insert(CapabilityShape::ProcessExec);
    s.insert(CapabilityShape::NetworkHttp);
    s
}

fn all_shapes() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
    s.insert(CapabilityShape::ProcessExec);
    s.insert(CapabilityShape::NetworkHttp);
    s.insert(CapabilityShape::AgentSpawn);
    s
}

// ---------------------------------------------------------------------------
// REGISTRY
// ---------------------------------------------------------------------------

/// All triples known to tau (Available + Reserved).
pub static REGISTRY: &[TargetTripleEntry] = &[
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Native,
            tier: SandboxTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Native,
            tier: SandboxTier::Light,
        },
        shapes_fn: fs_rw_exec_net,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Container,
            tier: SandboxTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Darwin,
            adapter_family: AdapterFamily::Native,
            tier: SandboxTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple::PASSTHROUGH,
        shapes_fn: all_shapes,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Windows,
            adapter_family: AdapterFamily::Native,
            tier: SandboxTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        status: TripleStatus::Reserved {
            reason: "windows AppContainer scaffold; probe Unavailable in v1",
        },
    },
];

/// Look up a registry entry by its triple. `O(n)` over a small registry.
pub fn lookup(triple: &TargetTriple) -> Option<&'static TargetTripleEntry> {
    REGISTRY.iter().find(|e| &e.triple == triple)
}

/// Iterate every entry (Available + Reserved).
pub fn list_all() -> impl Iterator<Item = &'static TargetTripleEntry> {
    REGISTRY.iter()
}

/// Iterate only Available entries.
pub fn list_available() -> impl Iterator<Item = &'static TargetTripleEntry> {
    REGISTRY
        .iter()
        .filter(|e| matches!(e.status, TripleStatus::Available))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_six_entries() {
        assert_eq!(REGISTRY.len(), 6);
    }

    #[test]
    fn registry_triples_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in REGISTRY {
            assert!(
                seen.insert(e.triple),
                "duplicate triple in REGISTRY: {:?}",
                e.triple
            );
        }
    }

    #[test]
    fn list_available_excludes_reserved() {
        let avail: Vec<_> = list_available().map(|e| e.triple).collect();
        assert_eq!(avail.len(), 5);
        for e in avail {
            let entry = lookup(&e).unwrap();
            assert!(matches!(entry.status, TripleStatus::Available));
        }
    }

    #[test]
    fn lookup_finds_linux_native_strict() {
        let t: TargetTriple = "linux-native-strict".parse().unwrap();
        let e = lookup(&t).unwrap();
        assert_eq!(e.triple, t);
        assert!(matches!(e.status, TripleStatus::Available));
        let shapes = (e.shapes_fn)();
        assert!(shapes.contains(&CapabilityShape::FilesystemRead));
        assert!(shapes.contains(&CapabilityShape::FilesystemWrite));
        assert!(shapes.contains(&CapabilityShape::ProcessExec));
        assert!(shapes.contains(&CapabilityShape::NetworkHttp));
        assert!(!shapes.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn lookup_finds_passthrough_with_all_shapes() {
        let e = lookup(&TargetTriple::PASSTHROUGH).unwrap();
        let shapes = (e.shapes_fn)();
        assert!(shapes.contains(&CapabilityShape::AgentSpawn));
    }

    #[test]
    fn lookup_finds_reserved_windows() {
        let t: TargetTriple = "windows-native-strict".parse().unwrap();
        let e = lookup(&t).unwrap();
        match &e.status {
            TripleStatus::Reserved { reason } => {
                assert!(!reason.is_empty());
            }
            other => panic!("expected Reserved, got {other:?}"),
        }
    }

    #[test]
    fn lookup_returns_none_for_unknown() {
        let t: TargetTriple = "darwin-container-strict".parse().unwrap();
        assert!(lookup(&t).is_none());
    }

    #[test]
    fn profile_materialises_shapes() {
        let t: TargetTriple = "linux-native-light".parse().unwrap();
        let e = lookup(&t).unwrap();
        let p = e.profile();
        assert_eq!(p.triple, t);
        assert!(matches!(p.status, TripleStatus::Available));
        assert!(p.required_shapes.contains(&CapabilityShape::FilesystemRead));
    }
}
