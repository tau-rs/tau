//! IR node variants. Typed full per D-1: Agent + Tool + Deterministic + Subflow.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget::AgentBudget;
use crate::capability::CapabilityRequirements;
use crate::context::ContextConfig;
use crate::ids::{AgentId, StepId, ToolId};
use crate::subflow::SubflowEdge;
use crate::tool_impl::{NativeFnRef, ToolImpl};

/// One of the four IR node variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// LLM agent loop with tool dispatch.
    Agent(Agent),
    /// A tool node — native impl or MCP contract.
    Tool(Tool),
    /// Pure-function step. No LLM, no I/O.
    Deterministic(Deterministic),
    /// Subflow connection (composition edge).
    Subflow(SubflowEdge),
}

/// An LLM agent loop with tools and optional context block.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// Identifier within the workflow.
    pub id: AgentId,
    /// System prompt.
    pub prompt: String,
    /// Model identifier (e.g. `"claude-haiku-4-5"`).
    pub model: String,
    /// Tools this agent is allowed to call.
    pub tool_refs: Vec<ToolId>,
    /// Optional β.4 context-management config.
    pub context: Option<ContextConfig>,
    /// Execution budget.
    pub budget: AgentBudget,
}

/// A tool node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Identifier within the workflow.
    pub id: ToolId,
    /// How the tool's behavior is provided.
    pub impl_: ToolImpl,
    /// Declared capabilities. Used by the capability-fit check (D-3b)
    /// and by the runtime gate.
    pub capabilities: CapabilityRequirements,
    /// Tool specification (name, description, input schema) used by the
    /// LLM to decide when to call the tool.
    pub spec: ToolSpec,
}

/// Tool specification surface used by the agent loop.
///
/// Mirror of `tau_ports::ToolSpec` adapted for IR storage. Provides the
/// LLM-facing schema; not used for capability decisions.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// LLM-visible name.
    pub name: String,
    /// LLM-visible description.
    pub description: String,
    /// JSON schema for the tool's input.
    pub input_schema: Value,
}

/// A pure-function step.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Deterministic {
    /// Identifier within the workflow.
    pub id: StepId,
    /// Reference to the statically linked Rust function.
    pub fn_ref: NativeFnRef,
    /// JSON schema for the input.
    pub input_schema: Value,
    /// JSON schema for the output.
    pub output_schema: Value,
}

/// `Subflow` re-exported as a `Node` payload (alias).
pub type Subflow = SubflowEdge;
