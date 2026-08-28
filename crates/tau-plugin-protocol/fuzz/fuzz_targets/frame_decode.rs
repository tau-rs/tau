//! Fuzz harness for `Frame::decode`.
//!
//! Feeds arbitrary bytes into the MessagePack-RPC frame decoder and
//! asserts it returns a typed `ProtocolError` for malformed input
//! rather than panicking, crashing, or running unbounded.
//!
//! Frame::decode is the primary boundary where untrusted bytes (from a
//! plugin subprocess over stdio) enter the runtime, so robustness here
//! directly improves plugin-isolation guarantees.
//!
//! Triage signals:
//!   - Process abort → libFuzzer reports a crash. Treat as a bug.
//!   - Timeout (default 25s/run) → potential exponential parse path. Bug.
//!   - `-malloc_limit_mb` exceeded → a *single* allocation ran away.
//!     This is the real unbounded-allocation signal, and the workflows
//!     set the limit explicitly because it otherwise defaults to
//!     `-rss_limit_mb` (2048 MB), i.e. effectively off.
//!   - `-rss_limit_mb` exceeded, with no single large allocation →
//!     **not** necessarily a target bug. Process RSS under ASan grows
//!     with cumulative executions (redzones, quarantine, and the
//!     append-only stack depot), and this target runs >20k exec/s, so it
//!     reaches that ceiling on session length alone. Issue #676 was
//!     first misdiagnosed this way: the saved artifact was simply the
//!     input executing when the limit tripped, not its cause. Before
//!     concluding "unbounded allocation", replay the artifact under a
//!     tight `-malloc_limit_mb`; if it passes, the growth is the
//!     harness, not the decoder.
//!
//! `Frame::decode`'s own bounds — recursion capped at `MAX_DECODE_DEPTH`,
//! peak allocation proportional to input length, zero retention — are
//! pinned by `tests/decode_allocation_bound.rs` in the parent crate,
//! which is where a regression in them should surface first.
//!
//! Run locally:
//!     rustup toolchain install nightly
//!     cargo install cargo-fuzz
//!     cd crates/tau-plugin-protocol/fuzz
//!     cargo +nightly fuzz run frame_decode -- -max_total_time=60

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_plugin_protocol::Frame;

fuzz_target!(|data: &[u8]| {
    // No Ok/Err discrimination — only that the call returns normally.
    let _ = Frame::decode(data);
});
