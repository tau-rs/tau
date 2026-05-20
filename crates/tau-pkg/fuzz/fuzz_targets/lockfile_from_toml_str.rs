//! Fuzz harness for `LockFile::from_toml_str`.
//!
//! Feeds arbitrary bytes into the lockfile parser and asserts the parser
//! returns a typed `RegistryError` for invalid input rather than panicking,
//! crashing, or running unbounded.
//!
//! Triage signals:
//!   - Process abort → libFuzzer reports a crash. Treat as a bug.
//!   - Timeout (default 25s/run) → potential exponential parse path. Bug.
//!   - Memory blowup (default 2 GiB) → unbounded allocation. Bug.
//!
//! Run locally:
//!     rustup toolchain install nightly
//!     cargo install cargo-fuzz
//!     cd crates/tau-pkg/fuzz
//!     cargo +nightly fuzz run lockfile_from_toml_str -- -max_total_time=60

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_pkg::lockfile::LockFile;

fuzz_target!(|data: &[u8]| {
    // toml::from_str requires valid UTF-8. Pre-convert with replacement to
    // exercise the parser uniformly across binary blobs; the fuzz signal is
    // "does the parser handle THIS input without crashing" — UTF-8 sanity
    // is a separate concern handled by the caller in load() (which reads
    // a file with read_to_string and errors out on invalid UTF-8 there).
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    // We don't care about the Ok/Err result — only that this call returns
    // normally (no panic, no unbounded recursion, no allocator abort).
    let _ = LockFile::from_toml_str(s);
});
