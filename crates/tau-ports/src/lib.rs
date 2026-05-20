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
//! - [`sandbox::Sandbox`] — sandbox adapters; probe-based adapter selection
//!   for OS-native and container sandboxing.
//!
//! See `docs/decisions/0003-tau-ports.md` for the design rationale.

pub mod error;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
pub mod llm;
pub mod orchestration;
pub mod sandbox;
pub mod storage;
pub mod target;
pub mod tool;

pub use error::{KeyError, LlmError, NamespaceError, SandboxError, StorageError, ToolError};
pub use llm::{
    batch_to_stream, stream_to_batch, CompletionChunk, CompletionRequest, CompletionResponse,
    CompletionStream, ContentBlock, LlmBackend, LlmProviderMessage, StopReason, TokenUsage,
    ToolChoice, ToolSpec, ToolUse, ToolUseAccumulator,
};
pub use orchestration::{
    AgentId, RunBudget, RunId, RunSnapshot, RunStatus, Task, TaskEvent, TaskId, TaskListFilter,
    TaskStatus, TraceEvent, TraceEventKind,
};
pub use sandbox::{
    ResourceLimits, Sandbox, SandboxHandle, SandboxPlan, SandboxProbe, SandboxTier, WorkingContext,
};
pub use storage::{Key, Namespace, Storage};
pub use target::{
    AdapterFamily, ParseError as TargetParseError, Platform, TargetCapabilityProfile, TargetTriple,
    TargetTripleEntry, TripleStatus,
};
pub use tool::{
    DenyEntry, SessionContext, StatelessAdapter, StatelessTool, Tool, ToolContent, ToolResult,
};
