//! Executor-agnostic smoke test.
//!
//! `tau-runtime-core`'s lib target is executor-agnostic (no tokio/smol/async-std
//! in src/). Integration tests run on the host's std target so they can use any
//! executor; this test proves the core's primitives are pollable by a non-tokio
//! executor (`futures_executor::block_on` from the `futures` crate).
//!
//! Until the deferred Mutex→RefCell substitution (Tasks 3.5–3.6) lands,
//! this test exercises the EXTRACTED primitives (the `ids` minters) rather
//! than the full agent loop, which still has tokio-bound internal state in
//! tau-runtime's stream.rs.

use std::sync::Arc;

use tau_ports::{Clock, DeterministicRandom, MockClock, RandomSource};
use tau_runtime_core::ids;

#[test]
fn ids_helpers_are_executor_free() {
    let clock: Arc<dyn Clock> = Arc::new(MockClock::new());
    let random: Arc<dyn RandomSource> = Arc::new(DeterministicRandom::seeded(0xC0FFEE));

    // No executor involved — pure sync calls.
    let uuid = ids::uuid_v4(&random);
    assert_eq!(uuid.as_bytes().len(), 16);

    let ulid_str = ids::ulid(&clock, &random);
    assert_eq!(ulid_str.len(), 26, "ULID canonical length is 26 chars");

    let now = ids::now_utc(&clock);
    // MockClock::new() counter starts at 0 and increments on each call.
    // We just verify the helper compiles and returns a valid timestamp.
    assert!(now.timestamp_millis() >= 0);
}

#[test]
fn ids_helpers_drive_under_futures_executor() {
    // Wrap in a trivial async block + drive via futures_executor — proves
    // the core's port-routed helpers are pollable without tokio.
    let clock: Arc<dyn Clock> = Arc::new(MockClock::new());
    let random: Arc<dyn RandomSource> = Arc::new(DeterministicRandom::seeded(0xDEADBEEF));

    let ulid_str = futures_executor::block_on(async { ids::ulid(&clock, &random) });

    assert_eq!(
        ulid_str.len(),
        26,
        "ULID canonical length is 26 chars under futures executor"
    );
}

#[test]
fn uuid_v4_version_nibble_is_correct() {
    let random: Arc<dyn RandomSource> = Arc::new(DeterministicRandom::seeded(12345));
    let uuid = ids::uuid_v4(&random);
    let s = uuid.to_string();
    // UUID v4: 14th char (0-indexed position 14) is the version nibble '4'.
    assert_eq!(&s[14..15], "4", "UUID v4 version nibble must be '4'");
    // Variant: bit pattern 10xx in byte 8 means char at position 19 is 8/9/a/b.
    let variant_char = s.chars().nth(19).unwrap();
    assert!(
        matches!(variant_char, '8' | '9' | 'a' | 'b'),
        "UUID v4 variant char at pos 19 must be 8/9/a/b, got '{variant_char}'"
    );
}
