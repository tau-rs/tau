//! Non-Linux fallback: every probe returns Unavailable.

use tau_ports::CapabilityProbe;

#[allow(dead_code)] // Used only on non-Linux.
pub(crate) fn unavailable_probe() -> CapabilityProbe {
    CapabilityProbe::Unavailable {
        reason: "tau-sandbox-native requires Linux".into(),
    }
}
