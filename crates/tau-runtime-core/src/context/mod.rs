//! β.4 context-manager primitive: per-turn transformers over the
//! conversation, applied before history is projected to the LLM.

pub mod estimator;
pub mod transformers;

pub use estimator::{HeuristicEstimator, TokenEstimator};
pub use transformers::{CompactToolOutputs, FitBudget, TrimOld};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use tau_domain::{CapabilityShape, Message};

// Re-export the IR-defined determinism class so the trait and the IR share
// one definition (no drift).
pub use tau_ir::context::DeterminismClass;

/// A capability a transformer requires (e.g. fs-write for β.4.2 offload).
/// v1's three builtins return an empty slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityNeed {
    /// The capability shape required (matches `tau_domain::CapabilityShape`).
    pub shape: CapabilityShape,
}

/// Error returned by a context transformer or the pipeline runner.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextError {
    /// Protected content (system prompt + live turn) alone exceeds the budget.
    #[error(
        "context budget unsatisfiable: protected {protected_tokens} tokens > max {max_tokens}"
    )]
    BudgetUnsatisfiable {
        /// Estimated tokens of protected (undroppable) content.
        protected_tokens: u32,
        /// The configured `fit_budget.max_tokens`.
        max_tokens: u32,
    },
    /// A transformer failed internally.
    #[error("context transformer '{name}' failed: {detail}")]
    Transformer {
        /// Transformer name.
        name: String,
        /// Human-readable detail.
        detail: String,
    },
}

/// Capability-scoped context handed to a transformer. The fields a class
/// may access are gated by construction: `Pure` gets the estimator only;
/// `LlmBacked`/`Stateful` (later tiers) get additional handles.
pub struct TransformCx<'a> {
    estimator: &'a dyn TokenEstimator,
    system_prompt: Option<&'a str>,
}

impl<'a> TransformCx<'a> {
    /// Construct a `Pure`-scoped context.
    pub fn pure(estimator: &'a dyn TokenEstimator, system_prompt: Option<&'a str>) -> Self {
        Self {
            estimator,
            system_prompt,
        }
    }

    /// Estimate one message's token cost.
    pub fn estimate_tokens(&self, msg: &Message) -> u32 {
        self.estimator.estimate(msg)
    }

    /// The agent's system prompt, if any (counts against the budget).
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt
    }
}

/// Future returned by [`ContextTransformer::transform`]. Mirrors the
/// boxed-future idiom used by `ToolDispatcher` (no `async_trait` in core).
pub type ContextFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Message>, ContextError>> + Send + 'a>>;

/// One step in an agent's per-turn context pipeline. THE public extension
/// point (contract #5): users implement this for custom nodes.
pub trait ContextTransformer: Send + Sync {
    /// Stable name; matches the `transformer` field in the IR.
    fn name(&self) -> &str;
    /// Determinism class; gates conformance and `TransformCx` scoping.
    fn determinism(&self) -> DeterminismClass;
    /// Capabilities this node needs (empty for v1's builtins).
    fn required_capabilities(&self) -> &[CapabilityNeed];
    /// Transform the per-turn message view.
    fn transform<'a>(&'a self, cx: &'a TransformCx<'a>, msgs: Vec<Message>) -> ContextFuture<'a>;
}
