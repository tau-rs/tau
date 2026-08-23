//! v1 target triple registry. See spec §4.

use tau_domain::{CapabilityShape, CapabilityShapeSet, IrFeature};

use crate::capability_gate::CapabilityTier;
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
    /// Optional IR execution features this target can run. A `const` slice
    /// (unlike `shapes_fn`) because `IrFeature` is `Copy` — no allocation.
    pub supported_features: &'static [IrFeature],
    /// Whether this triple ships in v1 or is reserved for future work.
    pub status: TripleStatus,
}

impl TargetTripleEntry {
    /// Materialise the full profile, allocating the shape set.
    pub fn profile(&self) -> TargetCapabilityProfile {
        TargetCapabilityProfile {
            triple: self.triple,
            required_shapes: (self.shapes_fn)(),
            supported_features: self.supported_features,
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

fn fs_rw_net() -> CapabilityShapeSet {
    let mut s = CapabilityShapeSet::new();
    s.insert(CapabilityShape::FilesystemRead);
    s.insert(CapabilityShape::FilesystemWrite);
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
///
/// `supported_features`: every native/host target drives `run_pipeline` and can
/// execute all control-flow (`IrFeature::ALL`). `any-wasi-strict` lists none —
/// the wasm guest drives `run_ir_streaming`, which has no `run_pipeline`
/// control-flow execution path, so a control-flow workflow is refused for wasm
/// at build time (see `tau-ir-lower`'s `feature_fit`).
pub static REGISTRY: &[TargetTripleEntry] = &[
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Native,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Native,
            tier: CapabilityTier::Light,
        },
        shapes_fn: fs_rw_exec_net,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Linux,
            adapter_family: AdapterFamily::Container,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Darwin,
            adapter_family: AdapterFamily::Native,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple::PASSTHROUGH,
        shapes_fn: all_shapes,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Any,
            adapter_family: AdapterFamily::Wasi,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_net,
        // Wasm guests cannot execute control-flow steps (no run_pipeline path).
        supported_features: &[],
        status: TripleStatus::Available,
    },
    TargetTripleEntry {
        triple: TargetTriple {
            platform: Platform::Windows,
            adapter_family: AdapterFamily::Native,
            tier: CapabilityTier::Strict,
        },
        shapes_fn: fs_rw_exec_net,
        supported_features: IrFeature::ALL,
        status: TripleStatus::Available,
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
    use alloc::vec::Vec;

    #[test]
    fn registry_has_seven_entries() {
        assert_eq!(REGISTRY.len(), 7);
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
        // Phase 2 graduated `windows-native-strict` to Available, so all 7
        // registry entries are now Available (0 Reserved).
        let avail: Vec<_> = list_available().map(|e| e.triple).collect();
        assert_eq!(avail.len(), 7);
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
    fn lookup_finds_available_windows() {
        // Phase 2 graduated `windows-native-strict` Reserved -> Available,
        // closing the `host()` divergence gap: default `tau build` and
        // `--target windows-native-strict` now agree on Windows.
        let t: TargetTriple = "windows-native-strict".parse().unwrap();
        let e = lookup(&t).unwrap();
        assert!(matches!(e.status, TripleStatus::Available));
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

    #[test]
    fn any_wasi_strict_supports_no_control_flow_features() {
        // Wasm guests drive run_ir_streaming, not run_pipeline; control-flow
        // has no wasm execution path, so the profile must list no features.
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        let e = lookup(&t).expect("any-wasi-strict must be registered");
        assert!(
            e.supported_features.is_empty(),
            "wasi target must support no IR control-flow features, got {:?}",
            e.supported_features
        );
    }

    #[test]
    fn native_targets_support_all_control_flow_features() {
        for name in ["linux-native-strict", "darwin-native-strict"] {
            let t: TargetTriple = name.parse().unwrap();
            let e = lookup(&t).unwrap();
            assert_eq!(
                e.supported_features,
                IrFeature::ALL,
                "{name} should support all control-flow features"
            );
        }
    }

    #[test]
    fn any_wasi_strict_is_available_with_fs_rw_net_shapes() {
        let t: TargetTriple = "any-wasi-strict".parse().unwrap();
        let e = lookup(&t).expect("any-wasi-strict must be registered");
        assert!(matches!(e.status, TripleStatus::Available));
        let shapes = (e.shapes_fn)();
        assert!(shapes.contains(&CapabilityShape::FilesystemRead));
        assert!(shapes.contains(&CapabilityShape::FilesystemWrite));
        assert!(shapes.contains(&CapabilityShape::NetworkHttp));
        assert!(!shapes.contains(&CapabilityShape::ProcessExec));
        assert!(!shapes.contains(&CapabilityShape::AgentSpawn));
    }
}
