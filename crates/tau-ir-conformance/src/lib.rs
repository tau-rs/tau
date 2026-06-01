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

/// Deterministic SHA-256 stand-in for a native tool's symbolic name —
/// the same shape `tau-cli::cmd::build::sha256_name` uses to populate the
/// `lower_ir` cache. Shared by DevMode and BundleMode here so both modes
/// hash identical IR bytes for the same workflow.
pub(crate) fn sha256_name(name: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.finalize().into()
}

/// Canonical multiset-key bytes for a `tau_domain::Message`.
///
/// Strips fields that are inherently per-run nondeterministic so the
/// dev-mode and bundle-mode runs of the same fixture produce identical
/// multisets:
///
/// - `id` and `parent_id` (fresh `MessageId` UUIDs minted per `Message::new`).
/// - `created_at` (per-run `SystemTime::now()`).
/// - `AgentInstanceId` UUIDs inside `Address::Agent(...)` — normalized to
///   the bare discriminant `"Agent"`. Tool / User / System addresses
///   keep their full identity (those ARE the load-bearing parts of
///   D-7a's side-effect record).
///
/// Returns JSON bytes of the form
/// `{"sender": "...", "recipient": "...", "payload": <json>}` so two
/// distinct payload shapes still produce distinct keys.
pub(crate) fn canonical_message_bytes(msg: &tau_domain::Message) -> Vec<u8> {
    use tau_domain::Address;
    fn addr_tag(a: &Address) -> serde_json::Value {
        match a {
            // Strip the AgentInstanceId UUID — per-run nondeterminism.
            Address::Agent(_) => serde_json::Value::String("Agent".into()),
            Address::Tool(name) => serde_json::json!({"Tool": name}),
            Address::User => serde_json::Value::String("User".into()),
            Address::System => serde_json::Value::String("System".into()),
            _ => serde_json::Value::String("Unknown".into()),
        }
    }
    let v = serde_json::json!({
        "sender": addr_tag(&msg.sender),
        "recipient": addr_tag(&msg.recipient),
        "payload": serde_json::to_value(&msg.payload).unwrap_or(serde_json::Value::Null),
    });
    serde_json::to_vec(&v).unwrap_or_default()
}

/// Discriminant of a `RunOutcome` (Completed / Failed / future variants)
/// as a stable string. Used by `assert_conform` to compare outcomes
/// without touching the per-run nondeterministic message UUIDs +
/// timestamps embedded in the variant payloads.
pub(crate) fn run_outcome_kind(outcome: &Option<RunOutcome>) -> &'static str {
    match outcome {
        None => "None",
        Some(RunOutcome::Completed { .. }) => "Completed",
        Some(RunOutcome::Failed { .. }) => "Failed",
        // Future variants — RunOutcome is #[non_exhaustive].
        Some(_) => "Unknown",
    }
}

/// Side-effect summary produced by a single execution.
///
/// Used by [`assert_conform`] to compare dev-mode and bundle-mode runs
/// per D-7a (multiset side-effect equivalence).
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceReport {
    /// Final outcome (Completed / Failed / ...).
    ///
    /// `Some(_)` when the interpreter actually ran; `None` when the
    /// build refused before the interpreter could be driven (in which
    /// case `build_refused` is `Some(_)` instead).
    pub run_outcome: Option<RunOutcome>,
    /// Multiset of (tool_name, args_canonical_bytes) → count.
    ///
    /// Keyed by `(tool_name, serde_json::to_vec(args))` so two identical
    /// invocations collapse to one entry with count incremented. Note: this
    /// is insertion-order-dependent for JSON objects; β.2.6.1 will switch
    /// to `tau_ir::to_canonical_bytes` for stable ordering across modes.
    pub tool_calls: BTreeMap<(String, Vec<u8>), u32>,
    /// Multiset of message bodies keyed by canonical bytes → count.
    pub message_added: BTreeMap<Vec<u8>, u32>,
    /// Build-refused marker (D-3b).
    ///
    /// `None` ⇒ lowering succeeded and the run was executed; the
    /// multiset fields reflect the recorded side effects.
    ///
    /// `Some(diagnostic)` ⇒ lowering / IR-bundle decode refused this
    /// fixture before the interpreter could run. The `String` is the
    /// `Display`-formatted `IrError` (or bundle-build error). When this
    /// field is `Some`, the multiset fields are empty placeholders and
    /// [`assert_conform`] compares only the build-refused strings.
    pub build_refused: Option<String>,
}

impl ConformanceReport {
    /// Construct an empty report with the given outcome.
    pub fn new(run_outcome: RunOutcome) -> Self {
        Self {
            run_outcome: Some(run_outcome),
            tool_calls: BTreeMap::new(),
            message_added: BTreeMap::new(),
            build_refused: None,
        }
    }

    /// Construct a build-refused report. Use this when lowering (DevMode)
    /// or bundle build / IR-payload decode (BundleMode) refused before
    /// the interpreter could be driven.
    pub fn build_refused(diagnostic: impl Into<String>) -> Self {
        Self {
            run_outcome: None,
            tool_calls: BTreeMap::new(),
            message_added: BTreeMap::new(),
            build_refused: Some(diagnostic.into()),
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
/// Comparison semantics:
///
/// - When BOTH reports carry `build_refused = Some(_)`: compares only
///   the refusal strings. The two modes must produce the same
///   `IrError::Display` (or equivalent bundle-build error message),
///   matching the D-3b "refusal symmetry" rule — both surfaces must
///   refuse the same fixture for the same reason.
/// - When NEITHER carries `build_refused`: compares `run_outcome`,
///   `tool_calls`, and `message_added` byte-for-byte.
/// - When ONE carries `build_refused` and the other does not: panics
///   immediately — the two surfaces disagree about whether the fixture
///   is buildable, which is a conformance bug.
pub fn assert_conform(dev: &ConformanceReport, bundle: &ConformanceReport) {
    match (&dev.build_refused, &bundle.build_refused) {
        (Some(d), Some(b)) => {
            assert_eq!(
                d, b,
                "build-refused diagnostic mismatch: dev refused with {d:?}, bundle refused with {b:?}"
            );
        }
        (Some(d), None) => {
            panic!(
                "build-refused asymmetry: dev refused ({d:?}) but bundle did not refuse \
                 (bundle outcome: {:?})",
                bundle.run_outcome
            );
        }
        (None, Some(b)) => {
            panic!(
                "build-refused asymmetry: bundle refused ({b:?}) but dev did not refuse \
                 (dev outcome: {:?})",
                dev.run_outcome
            );
        }
        (None, None) => {
            // Compare RunOutcome by *discriminant* only — variant
            // payloads embed per-run nondeterministic message UUIDs
            // and SystemTime::now() values that would never compare
            // equal across two runs. D-7a's side-effect equivalence is
            // captured by `tool_calls` and `message_added` (which key
            // off canonical message bytes that strip those nondeterminisms).
            assert_eq!(
                run_outcome_kind(&dev.run_outcome),
                run_outcome_kind(&bundle.run_outcome),
                "RunOutcome kind mismatch: dev={:?}, bundle={:?}",
                dev.run_outcome,
                bundle.run_outcome,
            );
            assert_eq!(
                dev.tool_calls, bundle.tool_calls,
                "tool-call multiset mismatch"
            );
            assert_eq!(
                dev.message_added, bundle.message_added,
                "message-added multiset mismatch"
            );
        }
    }
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

// ---------------------------------------------------------------------------
// MapBackedDeterministicRegistry — fixture-side DeterministicRegistry
// ---------------------------------------------------------------------------

use std::sync::Arc;

use tau_runtime_core::error::RuntimeError;
use tau_runtime_core::interpreter::deterministic::DeterministicRegistry;

/// Type alias for the closure stored per registered function.
type RegistryFn =
    Arc<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, RuntimeError> + Send + Sync>;

/// A `DeterministicRegistry` backed by a `BTreeMap<String, Fn>`.
///
/// The conformance suite uses this to wire scripted deterministic
/// functions into `RecordingDispatcher::deterministic_registry()`.
/// Fixture authors call [`MapBackedDeterministicRegistry::with`] to
/// register named functions.
#[derive(Default)]
pub struct MapBackedDeterministicRegistry {
    fns: BTreeMap<String, RegistryFn>,
}

impl MapBackedDeterministicRegistry {
    /// Register a function under `fn_name`. The function must be pure
    /// (no I/O, no global mutation).
    pub fn with<F>(mut self, fn_name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&serde_json::Value) -> Result<serde_json::Value, RuntimeError>
            + Send
            + Sync
            + 'static,
    {
        self.fns.insert(fn_name.into(), Arc::new(f));
        self
    }
}

impl DeterministicRegistry for MapBackedDeterministicRegistry {
    fn invoke(
        &self,
        fn_name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        let f = self
            .fns
            .get(fn_name)
            .ok_or_else(|| RuntimeError::Internal {
                message: format!("MapBackedDeterministicRegistry: unknown fn {fn_name:?}"),
            })?;
        f(args)
    }
}

/// The canonical conformance-fixture registry used by the test suite.
///
/// Currently registers:
///
/// - `parse_celsius`: `{"raw": "<digits>"}` → `{"celsius": <int>}`.
///   Used by fixture 05 (`05_deterministic_step`).
pub fn fixture_deterministic_registry() -> Arc<MapBackedDeterministicRegistry> {
    Arc::new(MapBackedDeterministicRegistry::default().with(
        "parse_celsius",
        |args: &serde_json::Value| {
            let raw =
                args.get("raw")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RuntimeError::Internal {
                        message: "parse_celsius: `raw` must be a string".into(),
                    })?;
            let celsius: i64 = raw.trim().parse().map_err(|e| RuntimeError::Internal {
                message: format!("parse_celsius: not an integer: {e}"),
            })?;
            Ok(serde_json::json!({ "celsius": celsius }))
        },
    ))
}
