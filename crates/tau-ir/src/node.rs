//! IR node variants. Typed full per D-1: Agent + Tool + Deterministic + Subflow.

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::budget::AgentBudget;
use crate::capability::CapabilityRequirements;
use crate::context::ContextConfig;
use crate::durable::Durability;
use crate::ids::{AgentId, StepId, ToolId};
use crate::subflow::SubflowEdge;
use crate::tool_impl::{NativeFnRef, ToolImpl};

/// One of the four IR node variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Identifier within the workflow.
    pub id: AgentId,
    /// System prompt.
    pub prompt: String,
    /// Resolved model selection (backend + vendor id), baked at lowering.
    pub model_ref: crate::model_ref::ModelRef,
    /// Tools this agent is allowed to call.
    pub tool_refs: Vec<ToolId>,
    /// Optional β.4 context-management config.
    pub context: Option<ContextConfig>,
    /// Execution budget.
    pub budget: AgentBudget,
    /// Artifact loci this agent declares it produces (deliverable binding).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: alloc::vec::Vec<alloc::string::String>,
    /// Optional JSON schema describing the agent's structured output.
    /// Plumbed from `[agents.<id>].output_schema`; consumed by a later
    /// judge-compat build-time check. `skip_serializing_if` keeps
    /// schema-less agents byte-stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Optional durable-execution config (ADR-0053). `None` => not
    /// durable (whole-bundle reentrant only). `Some` opts the agent into
    /// turn-level checkpoint/resume. `skip_serializing_if` keeps
    /// non-durable agents byte-stable with pre-A-minimal modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<Durability>,
}

/// A tool node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::AgentBudget;
    use crate::ids::AgentId;

    /// An `Agent` with `produces` set round-trips through serde without loss.
    #[test]
    fn agent_produces_round_trips() {
        let agent = Agent {
            id: AgentId("writer".into()),
            prompt: "You write reports.".into(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec!["/x".into()],
            output_schema: None,
            durable: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        let back: Agent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(agent, back);
    }

    /// An `Agent` with empty `produces` serializes WITHOUT a `"produces"` key
    /// (guards byte-stability for produce-less agents).
    #[test]
    fn agent_empty_produces_omitted_from_json() {
        let agent = Agent {
            id: AgentId("gatherer".into()),
            prompt: alloc::string::String::new(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        assert!(
            !json.contains("\"produces\""),
            "expected 'produces' key to be absent for empty vec; got: {json}"
        );
    }

    /// An `Agent` with `output_schema` set round-trips through serde.
    #[test]
    fn agent_output_schema_round_trips() {
        let agent = Agent {
            id: AgentId("judge".into()),
            prompt: String::new(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: Some(serde_json::json!({"type": "object"})),
            durable: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        let back: Agent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(agent, back);
    }

    /// `output_schema = None` serializes WITHOUT an `"output_schema"` key.
    #[test]
    fn agent_empty_output_schema_omitted_from_json() {
        let agent = Agent {
            id: AgentId("gatherer".into()),
            prompt: String::new(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        assert!(
            !json.contains("\"output_schema\""),
            "expected 'output_schema' key absent for None; got: {json}"
        );
    }

    /// An `Agent` with `durable` set round-trips through serde (ADR-0053).
    #[test]
    fn agent_durable_round_trips() {
        let agent = Agent {
            id: AgentId("fan-monitor".into()),
            prompt: String::new(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: Some(crate::durable::Durability::per_turn_file()),
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        let back: Agent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(agent, back);
    }

    /// `durable = None` serializes WITHOUT a `"durable"` key — byte-stable
    /// with pre-A-minimal modules (mirrors `produces` / `output_schema`).
    #[test]
    fn agent_non_durable_omitted_from_json() {
        let agent = Agent {
            id: AgentId("gatherer".into()),
            prompt: String::new(),
            model_ref: crate::model_ref::ModelRef {
                backend: "anthropic".into(),
                model_id: "claude-haiku-4-5".into(),
            },
            tool_refs: alloc::vec::Vec::new(),
            context: None,
            budget: AgentBudget {
                max_turns: None,
                max_tokens: None,
            },
            produces: alloc::vec::Vec::new(),
            output_schema: None,
            durable: None,
        };
        let json = serde_json::to_string(&agent).expect("serialize");
        assert!(
            !json.contains("\"durable\""),
            "expected 'durable' key absent for None; got: {json}"
        );
    }
}
