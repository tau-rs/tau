//! Fuzz harness for the `tau.toml` (PackageManifest) parser.
//!
//! Pipeline:
//!   `toml::from_str::<UncheckedManifest>(s)` →
//!     `UncheckedManifest::validate()` → `PackageManifest`
//!
//! Both stages are public on tau-domain. The fuzz target exercises the
//! full deserialize + validate pipeline and asserts the result is a
//! typed error, never a panic.
//!
//! Every tau package has a `tau.toml`, and many are authored by hand
//! (skill packages, plugin packages, user-defined tool packages). A
//! panic in this path crashes `tau install`, `tau verify`, `tau check`,
//! and any other tool that loads a manifest.
//!
//! Run locally:
//!     rustup toolchain install nightly
//!     cargo install cargo-fuzz
//!     cd crates/tau-domain/fuzz
//!     cargo +nightly fuzz run parse_package_manifest -- -max_total_time=60

#![no_main]

use libfuzzer_sys::fuzz_target;
use tau_domain::UncheckedManifest;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    // Stage 1: TOML deserialize. Most malformed inputs fail here.
    let Ok(unchecked) = toml::from_str::<UncheckedManifest>(s) else {
        return;
    };

    // Stage 2: semantic validation (name + version + capability
    // shape checks, etc.). Returns typed PackageManifestError on
    // failure; must never panic regardless of what the deserialize
    // stage handed us.
    let _ = unchecked.validate();
});
