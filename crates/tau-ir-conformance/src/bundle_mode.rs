//! Bundle-mode runner: build a bundle, then drive the v0 interpreter
//! over the deserialized IR.
//!
//! # STUB — β.2.6.1 unblocks
//!
//! IGNORE-REASON: `tau run --bundle` interpreter dispatch was left stubbed
//! at the end of β.2.5 (ToolDispatcher wiring deferred). Until that
//! dispatch is wired, `BundleMode::run` cannot actually execute a fixture.
//! This module is present to lock the conformance contract (the
//! `ExecutionMode` trait surface) and to allow the cross-mode comparison
//! test to be registered as `#[ignore]`'d rather than absent.
//!
//! When β.2.6.1 ships:
//! 1. Wire `ToolDispatcher` in `tau-cli/src/cmd/run.rs` for the
//!    `--bundle` path.
//! 2. Replace the `todo!()` below with a real implementation mirroring
//!    the `DevMode` steps but building a bundle first (see plan §6.3).
//! 3. Remove the `#[ignore]` from `tests/conformance.rs`.

use std::path::Path;

use async_trait::async_trait;

use crate::{ConformanceReport, ExecutionMode};

/// Bundle-mode runner.
///
/// Stub body — see module-level IGNORE-REASON.
pub struct BundleMode;

#[async_trait(?Send)]
impl ExecutionMode for BundleMode {
    async fn run(&self, _fixture_dir: &Path) -> ConformanceReport {
        // IGNORE-REASON: β.2.6.1 unblocks: bundle_mode dispatch is stubbed
        // (β.2.5 deferred ToolDispatcher wiring in tau run --bundle path).
        todo!("β.2.6.1 unstubs tau run --bundle interpreter dispatch first")
    }
}
