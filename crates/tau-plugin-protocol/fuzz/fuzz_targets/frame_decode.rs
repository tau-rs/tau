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
//!   - Memory blowup (default 2 GiB) → unbounded allocation
//!     (rmpv decoding a huge integer-prefixed array, for instance). Bug.
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
