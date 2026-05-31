#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Public Rust API surface for embedding tau as a library. One of
//! tau's two stable surfaces (G6, QG12); the other is the serve-mode
//! protocol (sub-project 5+).
//!
//! tau-runtime is the kernel: it loads pre-constructed plugin
//! instances (LlmBackend, Tool, Storage), runs an agent through a
//! multi-turn batch loop, dispatches messages to tools with typed-
//! capability enforcement (G14), and emits structured logs (G9).
//!
//! Solo path only at v0.1 — orchestration of multiple agents is
//! sub-project 5+ (G10).
//!
//! See `docs/decisions/0006-tau-runtime.md` for the design rationale.

pub mod builder;
pub(crate) mod capability;
pub mod clock;
pub mod random;
pub mod capability_override;
pub mod capability_resolver_impl;
pub mod process_gate;
pub use process_gate::DynProcessCapabilityGate;
pub(crate) mod dispatch;
pub mod error;
pub mod options;
pub mod orchestration;
pub mod outcome;
pub mod plugin_host;
mod run;
pub mod runtime_ext;
pub mod sandbox;
pub mod stream;
pub(crate) mod tool_args;

pub use builder::{Runtime, RuntimeBuilder, TauRuntimeBuilder};
pub use clock::TokioClock;
pub use random::OsRandom;
pub use capability_override::{CapabilityOverride, EffectiveCapability, OverrideExpandError};
pub use error::{BuildError, CapabilityDenial, HandshakeFailureReason, PluginKind, RuntimeError};
pub use options::{RunOptions, TokenUsage};
pub use outcome::RunOutcome;
pub use runtime_ext::spawn_root_agent_with_scope;
pub use stream::RunEvent;
