//! Adapter ↔ target-triple satisfaction logic.
//!
//! Joins the static adapter registry (`crate::process_gate::registry::REGISTRY`)
//! with a `TargetTriple`. Pure-data; no async, no probe.

use tau_ports::target::{AdapterFamily, TargetTriple};

use crate::process_gate::registry::{AdapterRegistration, RegistryKind, REGISTRY};

/// Map an internal `RegistryKind` to the user-facing `AdapterFamily`.
///
/// `Wasi` has no current `RegistryKind` — wasi triples never satisfy
/// any registered adapter today.
///
/// # Example
///
/// ```
/// use tau_ports::target::AdapterFamily;
/// use tau_runtime_tokio::process_gate::registry::RegistryKind;
/// use tau_runtime_tokio::process_gate::target_match::kind_to_family;
///
/// assert_eq!(kind_to_family(RegistryKind::Native), AdapterFamily::Native);
/// assert_eq!(kind_to_family(RegistryKind::Container), AdapterFamily::Container);
/// assert_eq!(kind_to_family(RegistryKind::Passthrough), AdapterFamily::Passthrough);
/// ```
pub fn kind_to_family(kind: RegistryKind) -> AdapterFamily {
    match kind {
        RegistryKind::Native => AdapterFamily::Native,
        RegistryKind::Container => AdapterFamily::Container,
        RegistryKind::Remote => AdapterFamily::Remote,
        RegistryKind::Passthrough => AdapterFamily::Passthrough,
    }
}

/// Does the given adapter registration satisfy this triple's constraints?
///
/// Requires:
/// - Adapter's `platforms` set includes the triple's platform (with
///   `Platform::Any` always satisfied by any registration).
/// - Adapter's `RegistryKind` maps to the triple's `AdapterFamily`.
/// - Adapter's `tiers_supported` contains the triple's tier.
///
/// Shape coverage is NOT checked here — that's a separate question
/// answered by comparing the triple's `required_shapes` to the
/// adapter's `shapes_supported_fn()` output.
///
/// # Example
///
/// ```
/// use tau_ports::target::TargetTriple;
/// use tau_runtime_tokio::process_gate::registry::REGISTRY;
/// use tau_runtime_tokio::process_gate::target_match::adapter_satisfies;
///
/// let passthrough = TargetTriple::PASSTHROUGH;
/// let p_reg = REGISTRY.iter().find(|r| r.kind == tau_runtime_tokio::process_gate::registry::RegistryKind::Passthrough).unwrap();
/// assert!(adapter_satisfies(p_reg, &passthrough));
///
/// let linux_strict: TargetTriple = "linux-native-strict".parse().unwrap();
/// let native_reg = REGISTRY.iter().find(|r| r.kind == tau_runtime_tokio::process_gate::registry::RegistryKind::Native).unwrap();
/// // Native adapter satisfies linux-native-strict on any host.
/// // (platform check is against triple.platform, not host OS)
/// assert!(adapter_satisfies(native_reg, &linux_strict));
/// ```
pub fn adapter_satisfies(adapter: &AdapterRegistration, triple: &TargetTriple) -> bool {
    let platform_ok = match triple.platform {
        tau_ports::target::Platform::Any => true,
        tau_ports::target::Platform::Linux => adapter.platforms.includes("linux"),
        tau_ports::target::Platform::Darwin => adapter.platforms.includes("macos"),
        tau_ports::target::Platform::Windows => adapter.platforms.includes("windows"),
        // Forward-compat: unknown future platforms are not satisfied by
        // any registered adapter in this build.
        _ => false,
    };
    if !platform_ok {
        return false;
    }
    if kind_to_family(adapter.kind) != triple.adapter_family {
        return false;
    }
    adapter.tiers_supported.contains(&triple.tier)
}

/// Find the first adapter registration that satisfies the triple.
///
/// Returns the static registration; the caller is responsible for
/// instantiating + probing if it wants a live adapter. `None` when no
/// registered adapter can serve this triple (typical for Reserved
/// triples like `wasi-*` or `windows-native-strict`).
///
/// # Example
///
/// ```
/// use tau_ports::target::TargetTriple;
/// use tau_runtime_tokio::process_gate::target_match::registration_for_triple;
/// use tau_runtime_tokio::process_gate::registry::RegistryKind;
///
/// let triple = TargetTriple::PASSTHROUGH;
/// let reg = registration_for_triple(&triple).expect("passthrough always has a registration");
/// assert_eq!(reg.kind, RegistryKind::Passthrough);
///
/// // Reserved triple has no matching adapter.
/// let reserved: TargetTriple = "windows-native-strict".parse().unwrap();
/// assert!(registration_for_triple(&reserved).is_none());
/// ```
pub fn registration_for_triple(triple: &TargetTriple) -> Option<&'static AdapterRegistration> {
    REGISTRY.iter().find(|a| adapter_satisfies(a, triple))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_ports::CapabilityTier;

    fn parse(s: &str) -> TargetTriple {
        s.parse().expect("test triple parses")
    }

    #[test]
    fn linux_native_strict_satisfied_by_native_adapter() {
        let t = parse("linux-native-strict");
        let r = registration_for_triple(&t).expect("adapter found");
        assert_eq!(r.kind, RegistryKind::Native);
        assert!(r.tiers_supported.contains(&CapabilityTier::Strict));
    }

    #[test]
    fn linux_native_light_satisfied_by_native_adapter() {
        let t = parse("linux-native-light");
        let r = registration_for_triple(&t).expect("adapter found");
        assert_eq!(r.kind, RegistryKind::Native);
        assert!(r.tiers_supported.contains(&CapabilityTier::Light));
    }

    #[test]
    fn linux_container_strict_satisfied_by_container_adapter() {
        let t = parse("linux-container-strict");
        let r = registration_for_triple(&t).expect("adapter found");
        assert_eq!(r.kind, RegistryKind::Container);
    }

    #[test]
    fn passthrough_satisfied_by_passthrough_adapter() {
        let t = TargetTriple::PASSTHROUGH;
        let r = registration_for_triple(&t).expect("adapter found");
        assert_eq!(r.kind, RegistryKind::Passthrough);
    }

    #[test]
    fn windows_native_strict_unsatisfied_in_v1() {
        // Native adapter is Linux-only per the registry; the Windows
        // triple has no satisfying entry.
        let t = parse("windows-native-strict");
        assert!(registration_for_triple(&t).is_none());
    }

    #[test]
    fn registry_shape_coverage_check() {
        // For every Available triple, the matched adapter (if any) must
        // be a superset of the triple's required shapes. This guards
        // against drift between the two registries.
        for entry in tau_ports::target::list_available() {
            let Some(adapter) = registration_for_triple(&entry.triple) else {
                panic!(
                    "Available triple {} has no satisfying adapter — shipping a triple with no impl is forbidden",
                    entry.triple
                );
            };
            let triple_shapes = (entry.shapes_fn)();
            let adapter_shapes = (adapter.shapes_supported_fn)();
            // Enumerate the 5 standard shapes; CapabilityShape::Custom isn't in any v1 triple.
            for required in [
                tau_domain::CapabilityShape::FilesystemRead,
                tau_domain::CapabilityShape::FilesystemWrite,
                tau_domain::CapabilityShape::ProcessExec,
                tau_domain::CapabilityShape::NetworkHttp,
                tau_domain::CapabilityShape::AgentSpawn,
            ] {
                if triple_shapes.contains(&required) {
                    assert!(
                        adapter_shapes.contains(&required),
                        "Triple {} requires shape {:?} but matched adapter {:?} does not support it",
                        entry.triple,
                        required,
                        adapter.kind,
                    );
                }
            }
        }
    }
}
