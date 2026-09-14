//! The back-door tripwire from ADR-0006: the kernel *carries* the bridge
//! vocabulary for its neighbours and never *uses* it. The kernel-proper
//! sources may not name the module.
//!
//! A source-text check is crude, and that is the point: it fails on the first
//! `use crate::bridge::...`, before anyone has to argue about whether a
//! particular parse was really "interpreting a payload".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const KERNEL_PROPER: &[(&str, &str)] = &[
    ("kernel/src/kernel.rs", include_str!("../src/kernel.rs")),
    ("kernel/src/reducer.rs", include_str!("../src/reducer.rs")),
    ("kernel/src/log.rs", include_str!("../src/log.rs")),
    ("kernel/src/syscall.rs", include_str!("../src/syscall.rs")),
    ("kernel/src/driver.rs", include_str!("../src/driver.rs")),
    ("kernel/src/blob.rs", include_str!("../src/blob.rs")),
];

#[test]
fn the_kernel_proper_never_imports_the_bridge() {
    for (path, source) in KERNEL_PROPER {
        for needle in ["crate::bridge", "bridge::", "super::bridge"] {
            assert!(
                !source.contains(needle),
                "{path} names `{needle}`: the kernel must not parse payloads (ADR-0003 invariant 2, ADR-0006)"
            );
        }
    }
}
