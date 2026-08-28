//! Pins the allocation contract of [`Frame::decode`] — issue #676.
//!
//! `Frame::decode` is the boundary where untrusted bytes from a plugin
//! subprocess enter the host, so its memory behaviour is a security
//! property, not just a performance one. Two things must hold for
//! *every* input, well-formed or not:
//!
//! 1. **Bounded peak.** Peak live allocation during one call is
//!    `PREALLOC_CEILING + BYTES_PER_INPUT_BYTE * body.len() + FIXED_SLACK`.
//!    In particular it does **not** scale with the *declared* length in a
//!    container header — a five-byte `str32` announcing 4 GiB must not
//!    reserve 4 GiB.
//! 2. **Zero retention.** A call allocates nothing that outlives it.
//!
//! Today both properties are inherited from `rmpv`'s decoder, which
//! deliberately does not pre-size containers from the declared length and
//! caps byte-string pre-allocation at 64 KiB. That is an implementation
//! detail of a dependency; this test makes it an invariant *tau* owns, so
//! a version bump or decoder swap that reintroduces length-driven
//! reservation fails here instead of in production.
//!
//! ## Why this test does not assert "no OOM under libFuzzer"
//!
//! #676 was filed against the `frame_decode` fuzz leg, which crosses
//! libFuzzer's 2048 MB RSS limit after several million executions. That
//! growth is **not** attributable to this function: measured here, decode
//! retains zero bytes across millions of calls. The fuzz-leg growth is
//! AddressSanitizer bookkeeping (per-allocation redzones, quarantine, and
//! the append-only stack depot) accumulating over the session, and is
//! addressed in the fuzz workflows rather than in this crate.

// A `#[global_allocator]` needs `unsafe impl GlobalAlloc`. The library
// crate keeps `#![forbid(unsafe_code)]`; an integration test is a separate
// crate, and the workspace policy (`unsafe_code = "warn"`) allows a scoped
// opt-out like this one.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use tau_plugin_protocol::Frame;

/// Largest byte-buffer `rmpv` pre-allocates before it has the bytes in
/// hand (its `PREALLOC_MAX`). This is the one term that is not
/// proportional to the input length, which is exactly why it is named.
const PREALLOC_CEILING: usize = 64 * 1024;

/// Each input byte can materialise at most one `rmpv::Value` (~40 B) into
/// a `Vec` that may be up to 2x over-allocated by amortised growth, plus
/// the re-encoded `params`/`result` copy `Frame::decode` hands back.
const BYTES_PER_INPUT_BYTE: usize = 128;

/// Absorbs small fixed costs (error strings, the `Frame` itself).
const FIXED_SLACK: usize = 8 * 1024;

fn allocation_ceiling(len: usize) -> usize {
    PREALLOC_CEILING + BYTES_PER_INPUT_BYTE * len + FIXED_SLACK
}

// ---------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            PEAK.fetch_max(LIVE.fetch_add(l.size(), Relaxed) + l.size(), Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let np = unsafe { System.realloc(p, l, new) };
        if !np.is_null() {
            let now = LIVE.fetch_add(new, Relaxed) + new - l.size();
            LIVE.fetch_sub(l.size(), Relaxed);
            PEAK.fetch_max(now, Relaxed);
        }
        np
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Decode `body`, returning `(peak allocation during the call, bytes still
/// held after the result is dropped)`.
fn measure(body: &[u8]) -> (usize, i64) {
    let before = LIVE.load(Relaxed);
    PEAK.store(before, Relaxed);
    let decoded = Frame::decode(body);
    let peak = PEAK.load(Relaxed);
    drop(decoded);
    let after = LIVE.load(Relaxed);
    (peak.saturating_sub(before), after as i64 - before as i64)
}

/// xorshift64* — deterministic and allocation-free, so it cannot perturb
/// the measurement.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// `depth` nested one-element arrays terminated by `nil`: one input byte
/// per level of decoder recursion, the cheapest possible nesting bomb.
fn nested_arrays(depth: usize) -> Vec<u8> {
    let mut v = vec![0x91_u8; depth];
    v.push(0xc0);
    v
}

/// The input libFuzzer saved when the `frame_decode` leg crossed its RSS
/// limit in CI run 33085192232. Kept in the fuzz corpus so the nightly
/// re-executes it too; `include_bytes!` here means deleting it breaks the
/// build rather than silently shrinking coverage.
const CI_OOM_ARTIFACT: &[u8] =
    include_bytes!("../fuzz/corpus/frame_decode/regress_676_ci_oom_artifact");

/// Container headers that declare an enormous length while carrying no
/// payload. A decoder that sized its buffer from the header would try to
/// reserve gigabytes from five bytes of input.
fn huge_declared_lengths() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("array32 len=2^32-1", vec![0xdd, 0xff, 0xff, 0xff, 0xff]),
        ("map32 len=2^32-1", vec![0xdf, 0xff, 0xff, 0xff, 0xff]),
        ("str32 len=2^32-1", vec![0xdb, 0xff, 0xff, 0xff, 0xff]),
        ("bin32 len=2^32-1", vec![0xc6, 0xff, 0xff, 0xff, 0xff]),
        ("ext32 len=2^32-1", vec![0xc9, 0xff, 0xff, 0xff, 0xff, 0x01]),
        ("array16 len=65535", vec![0xdc, 0xff, 0xff]),
        ("str16 len=65535", vec![0xda, 0xff, 0xff]),
    ]
}

/// One test function on purpose: the counting allocator is process-wide,
/// so a second concurrently-running test would pollute the measurement.
#[test]
fn decode_allocation_is_bounded_and_retains_nothing() {
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();

    for (name, body) in huge_declared_lengths() {
        cases.push((name.to_string(), body));
    }
    cases.push(("ci oom artifact (#676)".into(), CI_OOM_ARTIFACT.to_vec()));
    cases.push(("nested arrays x1023".into(), nested_arrays(1023)));
    cases.push(("nested arrays x1024".into(), nested_arrays(1024)));
    cases.push(("nested arrays x8192".into(), nested_arrays(8192)));

    // Deterministic pseudo-random bodies, the shape the fuzzer explores.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..2_000 {
        let len = (rng.next() % 4096) as usize;
        let body: Vec<u8> = (0..len).map(|_| (rng.next() >> 24) as u8).collect();
        cases.push((format!("random #{i} len={len}"), body));
    }

    let mut worst_ratio = 0.0_f64;
    let mut worst_name = String::new();

    for (name, body) in &cases {
        let (peak, retained) = measure(body);

        assert_eq!(
            retained, 0,
            "Frame::decode retained {retained} bytes after dropping its result, on {name:?}"
        );

        let ceiling = allocation_ceiling(body.len());
        assert!(
            peak <= ceiling,
            "Frame::decode peak allocation {peak} exceeds ceiling {ceiling} \
             for a {}-byte body, on {name:?}",
            body.len(),
        );

        let over_prealloc = peak.saturating_sub(PREALLOC_CEILING) as f64;
        let ratio = over_prealloc / body.len().max(1) as f64;
        if ratio > worst_ratio {
            worst_ratio = ratio;
            worst_name = name.clone();
        }
    }

    // Surfaced so a future tightening of the constants has evidence to
    // work from rather than a guess.
    println!(
        "worst observed bytes-allocated-per-input-byte (above PREALLOC_CEILING): \
         {worst_ratio:.1} on {worst_name:?} (asserted ceiling: {BYTES_PER_INPUT_BYTE})"
    );
}

/// Retention would show up as drift over many calls even if a single call
/// looks clean, so re-check the live-bytes counter over a long loop. This
/// is the property #676 originally reported as violated.
#[test]
#[ignore = "long-running; run with --ignored to reproduce the #676 retention check"]
fn decode_does_not_retain_across_many_calls() {
    let corpus: Vec<Vec<u8>> = huge_declared_lengths()
        .into_iter()
        .map(|(_, b)| b)
        .chain([CI_OOM_ARTIFACT.to_vec(), nested_arrays(1024)])
        .collect();

    // Warm up so first-touch allocations are not counted as drift.
    for body in &corpus {
        let _ = Frame::decode(body);
    }

    let baseline = LIVE.load(Relaxed);
    for i in 0..2_000_000_usize {
        let _ = Frame::decode(&corpus[i % corpus.len()]);
    }
    let drift = LIVE.load(Relaxed) as i64 - baseline as i64;

    assert_eq!(
        drift, 0,
        "Frame::decode drifted {drift} live bytes over 2,000,000 calls"
    );
}
