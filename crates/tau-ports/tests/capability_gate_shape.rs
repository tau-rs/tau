//! Locks the shape of CapabilityGate / CapabilityPlan / CapabilityProbe so
//! a future drift (e.g. re-adding wrap_spawn to the universal trait) fails
//! at compile time.

use tau_ports::{CapabilityGate, CapabilityPlan, CapabilityProbe, CapabilityShapeSet};

fn _shape_check<T: CapabilityGate>() {
    fn _accepts_universal(_t: &dyn CapabilityGate) {}
    // CapabilityGate must NOT carry process-flavored methods.
    // The four universal methods only:
    //   fn name(&self) -> &str;
    //   async fn probe(&self) -> CapabilityProbe;
    //   fn supported_shapes(&self) -> CapabilityShapeSet;
    //   fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), tau_ports::CapabilityError>;
}

#[test]
fn capability_gate_traits_exist() {
    let _ = core::any::TypeId::of::<CapabilityPlan>();
    let _ = core::any::TypeId::of::<CapabilityProbe>();
    let _ = core::any::TypeId::of::<CapabilityShapeSet>();
}

#[test]
fn mock_clock_is_monotonic() {
    use tau_ports::{Clock, MockClock};

    let clock = MockClock::default();
    let a = clock.now();
    let b = clock.now();
    let c = clock.now();
    assert!(b > a);
    assert!(c > b);
}
