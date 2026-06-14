//! β.4 context-manager primitive: per-turn transformers over the
//! conversation, applied before history is projected to the LLM.

pub mod estimator;

pub use estimator::{HeuristicEstimator, TokenEstimator};
