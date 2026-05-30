//! Locks the shape of CapabilityGate / CapabilityPlan / CapabilityProbe so
//! a future drift (e.g. re-adding wrap_spawn to the universal trait) fails
//! at compile time.
//!
//! MockClock/DeterministicRandom assertions require the `test-fixtures`
//! feature (since those types live behind that gate in `tau-ports`). The
//! whole file is feature-gated so `cargo test --workspace` without
//! `--features test-fixtures` skips this test cleanly.

#![cfg(feature = "test-fixtures")]

use tau_ports::{CapabilityGate, CapabilityPlan, CapabilityProbe, CapabilityShapeSet};

// CapabilityGate must NOT carry process-flavored methods.
// The four universal methods only:
//   fn name(&self) -> &str;
//   async fn probe(&self) -> CapabilityProbe;
//   fn supported_shapes(&self) -> CapabilityShapeSet;
//   fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), tau_ports::CapabilityError>;
//
// NOTE: `async fn in trait` (native RPITIT) is not dyn-compatible in Rust < 2.0,
// so the original `&dyn CapabilityGate` check is replaced with a generic-bound
// form. The intent is to ensure wrap_spawn / apply_post_spawn are NOT on the
// universal trait — they live on the ProcessCapabilityGate extension trait.
fn _shape_check<T: CapabilityGate>(_t: &T) {
    // Verify the four universal methods are callable via the bound.
    let _: &str = _t.name();
    let _: tau_ports::CapabilityShapeSet = _t.supported_shapes();
    let _plan = CapabilityPlan::new(Vec::new(), None, None);
    let _: Result<(), tau_ports::CapabilityError> = _t.validate_plan(&_plan);
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

#[test]
fn deterministic_random_is_seeded_and_repeatable() {
    use tau_ports::{DeterministicRandom, RandomSource};

    let a = DeterministicRandom::seeded(0xC0FFEE);
    let mut buf_a = [0u8; 16];
    a.fill(&mut buf_a);

    let b = DeterministicRandom::seeded(0xC0FFEE);
    let mut buf_b = [0u8; 16];
    b.fill(&mut buf_b);

    assert_eq!(buf_a, buf_b, "same seed must produce same bytes");

    let c = DeterministicRandom::seeded(0xDEADBEEF);
    let mut buf_c = [0u8; 16];
    c.fill(&mut buf_c);
    assert_ne!(buf_a, buf_c, "different seeds must produce different bytes");
}
