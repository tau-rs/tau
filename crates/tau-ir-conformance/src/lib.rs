//! Conformance runner for the tau workflow IR.
//!
//! For each fixture directory, runs the workflow under dev-mode and
//! bundle-mode and compares per D-7a (multiset side-effect equivalence).
//!
//! # Scope
//!
//! β.2.6 ships the framework + one working fixture (01_agent_native_tool)
//! with a fully functional `DevMode` runner. `BundleMode` is stubbed with
//! `todo!()` guarded by `#[ignore]` in the conformance test — β.2.6.1
//! unblocks it once `tau run --bundle` interpreter dispatch is unstubbed
//! (β.2.5 deferred ToolDispatcher wiring).

use std::collections::BTreeMap;
use std::path::Path;

use tau_runtime_core::outcome::RunOutcome;

pub mod bundle_mode;
pub mod dev_mode;

/// Side-effect summary produced by a single execution.
///
/// Used by [`assert_conform`] to compare dev-mode and bundle-mode runs
/// per D-7a (multiset side-effect equivalence).
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceReport {
    /// Final outcome (Completed / Failed / ...).
    pub run_outcome: RunOutcome,
    /// Multiset of (tool_name, args_canonical_bytes) → count.
    ///
    /// Keyed by `(tool_name, serde_json::to_vec(args))` so two identical
    /// invocations collapse to one entry with count incremented. Note: this
    /// is insertion-order-dependent for JSON objects; β.2.6.1 will switch
    /// to `tau_ir::to_canonical_bytes` for stable ordering across modes.
    pub tool_calls: BTreeMap<(String, Vec<u8>), u32>,
    /// Multiset of message bodies keyed by canonical bytes → count.
    pub message_added: BTreeMap<Vec<u8>, u32>,
}

impl ConformanceReport {
    /// Construct an empty report with the given outcome.
    pub fn new(run_outcome: RunOutcome) -> Self {
        Self {
            run_outcome,
            tool_calls: BTreeMap::new(),
            message_added: BTreeMap::new(),
        }
    }

    /// Record a tool invocation.
    ///
    /// `args` is the canonical JSON bytes of the tool's input.
    pub fn record_tool_call(&mut self, tool_name: impl Into<String>, args_canonical: Vec<u8>) {
        *self
            .tool_calls
            .entry((tool_name.into(), args_canonical))
            .or_insert(0) += 1;
    }

    /// Record a message body (canonical bytes).
    pub fn record_message(&mut self, canonical_bytes: Vec<u8>) {
        *self.message_added.entry(canonical_bytes).or_insert(0) += 1;
    }
}

/// Assert that two reports are equivalent per D-7a.
///
/// Panics if `run_outcome`, `tool_calls`, or `message_added` differ.
/// This is the primary conformance assertion: dev-mode and bundle-mode
/// must produce identical side-effect multisets.
pub fn assert_conform(dev: &ConformanceReport, bundle: &ConformanceReport) {
    assert_eq!(dev.run_outcome, bundle.run_outcome, "RunOutcome mismatch");
    assert_eq!(
        dev.tool_calls, bundle.tool_calls,
        "tool-call multiset mismatch"
    );
    assert_eq!(
        dev.message_added, bundle.message_added,
        "message-added multiset mismatch"
    );
}

/// Trait the runner calls to execute a fixture under one mode.
///
/// `DevMode` runs the IR interpreter directly with in-process tool
/// dispatch. `BundleMode` (β.2.6.1) builds a bundle and routes through
/// the bundle's wasm-gated dispatch path.
///
/// `?Send` because `tau_runtime_core::interpreter::run_ir` uses `RefCell`
/// internally and produces a non-`Send` future. Tests must use
/// `#[tokio::test(flavor = "current_thread")]` or a `LocalSet`.
#[async_trait::async_trait(?Send)]
pub trait ExecutionMode {
    /// Run the fixture at `fixture_dir` and return a side-effect report.
    async fn run(&self, fixture_dir: &Path) -> ConformanceReport;
}
