#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! The tau workflow IR.
//!
//! See `docs/superpowers/specs/2026-05-31-workflow-ir-design.md` for the
//! locked design. Per that spec:
//!
//! - Node types are typed full (Agent + Tool + Deterministic + Subflow) — see [`Node`].
//! - The inter-node wire message is [`Message`] (a thin IR-owned mirror of
//!   `tau_domain::Message`, with `SystemTime` normalized to `i64`-ms).
//! - The IR is content-hashed; both `ir_format` and `tau_version` participate
//!   in the hash. See `canonical` and `hash` modules.

extern crate alloc;
#[cfg(feature = "with-std-adapters")]
extern crate std;

pub mod budget;
pub mod canonical;
pub mod capability;
pub mod context;
pub mod error;
pub mod ids;
#[cfg(feature = "with-std-adapters")]
pub mod lower;
pub mod message;
pub mod module;
pub mod node;
pub mod subflow;
pub mod tool_impl;

// Re-exports of the canonical public API surface.
pub use budget::AgentBudget;
pub use canonical::{from_canonical_bytes, to_canonical_bytes};
pub use capability::{CapabilityRequirements, CapabilityTable};
pub use context::ContextConfig;
pub use error::IrError;
pub use ids::{AgentId, StepId, SubflowId, ToolId};
pub use message::{Message, MessagePayload};
pub use module::{IrFormatVersion, IrModule, Workflow};
pub use node::{Agent, Deterministic, Node, Subflow, Tool};
pub use subflow::SubflowKind;
pub use tool_impl::{Hash256, NativeFnRef, ToolImpl};
