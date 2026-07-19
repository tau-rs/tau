#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Port (trait) definitions for tau's hexagonal architecture. Adapters in
//! tau-infra implement these traits.
//!
//! tau-ports defines four trait families:
//!
//! - [`llm::LlmBackend`] — LLM provider plugins (`kind = "llm-backend"`).
//! - [`tool::Tool`] — tool plugins (`kind = "tool"`).
//! - [`storage::Storage`] — storage plugins (`kind = "storage"`).
//! - [`capability_gate::CapabilityGate`] — universal capability gate
//!   contract; concrete impls live in tau-sandbox-{native,container,darwin,windows}.
//!
//! See `docs/decisions/0003-tau-ports.md` for the design rationale.

extern crate alloc;

#[cfg(any(test, feature = "test-fixtures", feature = "process"))]
extern crate std;

pub mod capability_gate;
pub mod capability_resolver;
pub mod credential;
pub mod error;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
pub mod llm;
pub mod orchestration;
pub mod random;
pub mod skill_resolver;
pub mod storage;
pub mod target;
pub mod time;
pub mod tool;

#[cfg(feature = "process")]
pub use capability_gate::process::ProcessCapabilityGate;
#[cfg(feature = "process")]
pub use capability_gate::{dyn_process::DynProcessGate, passthrough::PassthroughGate};
pub use capability_gate::{
    CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe, CapabilityTier,
    ResourceLimits, WorkingContext,
};
pub use capability_resolver::{CapabilityResolver, CapabilityResolverError, ResolvedGrants};
pub use credential::{
    BakedProvider, CredentialChain, CredentialId, CredentialProvider, CredentialRequest,
    DynCredentialProvider, InvalidCredentialId, ResolvedCredential, Secret,
};
pub use error::{
    CapabilityError, CredentialError, KeyError, LlmError, NamespaceError, StorageError, ToolError,
};
pub use llm::{
    batch_to_stream, stream_to_batch, CompletionChunk, CompletionRequest, CompletionResponse,
    CompletionStream, ContentBlock, LlmBackend, LlmProviderMessage, StopReason, TokenUsage,
    ToolChoice, ToolSpec, ToolUse, ToolUseAccumulator,
};
pub use orchestration::{
    AgentId, CheckpointError, CheckpointStore, RunBudget, RunId, RunSnapshot, RunStatus, Task,
    TaskEvent, TaskId, TaskListFilter, TaskStatus, TraceEvent, TraceEventKind, TurnCheckpoint,
};
#[cfg(any(test, feature = "test-fixtures"))]
pub use random::DeterministicRandom;
pub use random::RandomSource;
pub use skill_resolver::{NoSkillResolver, ResolvedSkill, SkillResolveError, SkillResolver};
pub use storage::{Key, Namespace, Storage};
pub use target::{
    AdapterFamily, ParseError as TargetParseError, Platform, TargetCapabilityProfile, TargetTriple,
    TargetTripleEntry, TripleStatus,
};
pub use tau_domain::CapabilityShapeSet;
pub use time::Clock;
#[cfg(any(test, feature = "test-fixtures"))]
pub use time::MockClock;
pub use tool::{
    DenyEntry, SessionContext, StatelessAdapter, StatelessTool, Tool, ToolContent, ToolResult,
};
