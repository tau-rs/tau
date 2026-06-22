//! EPIC 2.3: lock the WIT host world (the embedding contract) against the
//! `tau-ports` traits it mirrors. ADR-0056: the embedding contract is the
//! minimal 3-function surface, WIT ⊊ ports, enforced by this drift test.
//!
//! Two guarantees:
//!   1. FROZEN SURFACE — the `host` interface declares EXACTLY 3 functions
//!      (complete / now-millis / next-u64). A 4th import or a removed/renamed
//!      function fails the parse assertions.
//!   2. CONTAINMENT (WIT ⊊ ports) — each WIT function's backing port method is
//!      referenced at COMPILE TIME below, so renaming/removing/re-signaturing
//!      `LlmBackend::complete` / `Clock::now` / `RandomSource::fill` breaks
//!      THIS TEST'S COMPILATION.
//!
//! Mapping note: `now-millis: u64` in WIT vs `Clock::now() -> i64` in ports —
//! the sign difference is intentional (WIT has no s64, UNIX epoch fits u64 for
//! milliseconds through the relevant time horizon). The containment guard uses
//! `Clock::now` because that is the port method backing the `now-millis` import.
//! Similarly, `next-u64` in WIT is backed by `RandomSource::fill`, which is the
//! entropy source from which individual u64 values are derived in the guest.

use std::path::PathBuf;

use tau_ports::llm::{CompletionRequest, LlmBackend};
use tau_ports::random::RandomSource;
use tau_ports::time::Clock;

/// Compile-time containment: these fns only compile while the three backing
/// port methods exist with a compatible shape. (Never called.)
#[allow(dead_code)]
fn _ports_back_the_wit_host_world() {
    fn _complete_exists<B: LlmBackend>(b: &B, req: CompletionRequest) {
        // `complete` is async → referencing the returned future is enough.
        let _fut = b.complete(req);
    }
    fn _now_exists(c: &dyn Clock) -> i64 {
        // WIT `now-millis: func() -> u64` is backed by `Clock::now() -> i64`.
        // The sign difference is documented above.
        c.now()
    }
    fn _next_u64_exists(r: &dyn RandomSource) {
        // WIT `next-u64: func() -> u64` is backed by `RandomSource::fill`,
        // the entropy source from which u64 values are drawn in the guest.
        let mut buf = [0u8; 8];
        r.fill(&mut buf);
    }
}

fn wit_dir() -> PathBuf {
    // crates/tau-ports → repo root → wit/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("wit")
}

#[test]
fn host_interface_is_the_frozen_three_function_surface() {
    let mut resolve = wit_parser::Resolve::new();
    resolve.push_dir(wit_dir()).expect("wit/ parses");

    // Find the `host` interface.
    let (_, iface) = resolve
        .interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some("host"))
        .expect("`host` interface present in wit/tau-host.wit");

    // EXACTLY the three expected function names — frozen minimal surface.
    let mut names: Vec<&str> = iface.functions.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["complete", "next-u64", "now-millis"],
        "the host world is the frozen 3-function surface; a change here is a \
         contract change — update the contract version + this test deliberately"
    );

    // Arity sanity per function (param count).
    let by = |n: &str| iface.functions.get(n).unwrap();
    assert_eq!(
        by("complete").params.len(),
        1,
        "complete takes request-json"
    );
    assert_eq!(by("now-millis").params.len(), 0);
    assert_eq!(by("next-u64").params.len(), 0);
}

#[test]
fn package_is_tau_host() {
    let mut resolve = wit_parser::Resolve::new();
    resolve.push_dir(wit_dir()).expect("wit/ parses");
    let has_tau_host = resolve
        .packages
        .iter()
        .any(|(_, p)| p.name.namespace == "tau" && p.name.name == "host");
    assert!(
        has_tau_host,
        "the embedding contract package must be `tau:host` (ADR-0056)"
    );
}
