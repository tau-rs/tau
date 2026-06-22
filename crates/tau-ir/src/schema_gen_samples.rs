//! Canonical sample IR modules for the published conformance kit (EPIC 2.2).
//! Feature-gated (`schema`) — compiled only when the schema feature is on.

use alloc::collections::BTreeMap;

use crate::budget::AgentBudget;
use crate::capability::CapabilityRequirements;
use crate::ids::{AgentId, PipelineStepId, StepId, ToolId};
use crate::model_ref::ModelRef;
use crate::module::{IrFormatVersion, IrModule, Workflow};
use crate::node::{Agent, Deterministic, Tool, ToolSpec};
use crate::pipeline::{Pipeline, PipelineStep, StepRun};
use crate::tool_impl::{NativeFnRef, ToolImpl};

/// Constructs a minimal valid `IrModule` baseline (no nodes).
fn base_module(workflow: Workflow) -> IrModule {
    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: tau_ports::target::TargetTriple::PASSTHROUGH,
        workflow,
        triggers: alloc::vec::Vec::new(),
    }
}

/// Sample: one agent + one native tool (`ToolImpl::Native`).
pub(crate) fn agent_native_tool() -> IrModule {
    let agent_id = AgentId("writer".into());
    let tool_id = ToolId("read-file".into());

    let agent = Agent {
        id: agent_id.clone(),
        prompt: "You are a writer agent.".into(),
        model_ref: ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        },
        tool_refs: alloc::vec![tool_id.clone()],
        context: None,
        budget: AgentBudget {
            max_turns: Some(10),
            max_tokens: None,
        },
        produces: alloc::vec![],
        output_schema: None,
        durable: None,
    };

    let tool = Tool {
        id: tool_id.clone(),
        impl_: ToolImpl::Native {
            fn_ref: NativeFnRef {
                name: "ReadFile".into(),
            },
            content_hash: [0u8; 32],
        },
        capabilities: CapabilityRequirements {
            declared: alloc::vec![],
        },
        spec: ToolSpec {
            name: "read_file".into(),
            description: "Read a file from the filesystem.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        },
    };

    let mut agents = BTreeMap::new();
    agents.insert(agent_id, agent);
    let mut tools = BTreeMap::new();
    tools.insert(tool_id, tool);

    let workflow = Workflow {
        agents,
        tools,
        ..Workflow::default()
    };

    base_module(workflow)
}

/// Sample: one agent + one MCP tool (`ToolImpl::Mcp`).
pub(crate) fn agent_mcp_tool() -> IrModule {
    let agent_id = AgentId("researcher".into());
    let tool_id = ToolId("weather-current".into());

    let agent = Agent {
        id: agent_id.clone(),
        prompt: "You are a research agent.".into(),
        model_ref: ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        },
        tool_refs: alloc::vec![tool_id.clone()],
        context: None,
        budget: AgentBudget {
            max_turns: Some(5),
            max_tokens: None,
        },
        produces: alloc::vec![],
        output_schema: None,
        durable: None,
    };

    let tool = Tool {
        id: tool_id.clone(),
        impl_: ToolImpl::Mcp {
            url: "https://mcp.weather.example.com".into(),
            contract_hash: [1u8; 32],
            capability_subset: CapabilityRequirements {
                declared: alloc::vec![],
            },
            server_tool_name: "get_current".into(),
        },
        capabilities: CapabilityRequirements {
            declared: alloc::vec![],
        },
        spec: ToolSpec {
            name: "weather_current".into(),
            description: "Get current weather conditions.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
        },
    };

    let mut agents = BTreeMap::new();
    agents.insert(agent_id, agent);
    let mut tools = BTreeMap::new();
    tools.insert(tool_id, tool);

    let workflow = Workflow {
        agents,
        tools,
        ..Workflow::default()
    };

    base_module(workflow)
}

/// Sample: one deterministic step node (`Deterministic`) in a pipeline
/// (`StepRun::Deterministic`).
pub(crate) fn deterministic_step() -> IrModule {
    let step_id = StepId("normalize".into());

    let step = Deterministic {
        id: step_id.clone(),
        fn_ref: NativeFnRef {
            name: "NormalizeText".into(),
        },
        input_schema: serde_json::json!({"type": "string"}),
        output_schema: serde_json::json!({"type": "string"}),
    };

    let pipeline = Pipeline {
        steps: alloc::vec![PipelineStep {
            id: PipelineStepId("step-normalize".into()),
            run: StepRun::Deterministic(step_id.clone()),
            input: "${input}".into(),
        }],
    };

    let mut steps = BTreeMap::new();
    steps.insert(step_id, step);

    let workflow = Workflow {
        steps,
        pipeline: Some(pipeline),
        ..Workflow::default()
    };

    base_module(workflow)
}

/// Sample: one agent + one step tool (`ToolImpl::Step`) delegating to a
/// deterministic step node.
pub(crate) fn tool_impl_step() -> IrModule {
    let agent_id = AgentId("processor".into());
    let tool_id = ToolId("normalize-text".into());
    let step_id = StepId("normalize".into());

    let agent = Agent {
        id: agent_id.clone(),
        prompt: "You are a text processor agent.".into(),
        model_ref: ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        },
        tool_refs: alloc::vec![tool_id.clone()],
        context: None,
        budget: AgentBudget {
            max_turns: Some(5),
            max_tokens: None,
        },
        produces: alloc::vec![],
        output_schema: None,
        durable: None,
    };

    let step_tool = Tool {
        id: tool_id.clone(),
        impl_: ToolImpl::Step {
            id: step_id.clone(),
        },
        capabilities: CapabilityRequirements {
            declared: alloc::vec![],
        },
        spec: ToolSpec {
            name: "normalize_text".into(),
            description: "Normalize text via a deterministic step.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        },
    };

    let step = crate::node::Deterministic {
        id: step_id.clone(),
        fn_ref: NativeFnRef {
            name: "NormalizeText".into(),
        },
        input_schema: serde_json::json!({"type": "string"}),
        output_schema: serde_json::json!({"type": "string"}),
    };

    let mut agents = BTreeMap::new();
    agents.insert(agent_id, agent);
    let mut tools = BTreeMap::new();
    tools.insert(tool_id, step_tool);
    let mut steps = BTreeMap::new();
    steps.insert(step_id, step);

    let workflow = Workflow {
        agents,
        tools,
        steps,
        ..Workflow::default()
    };

    base_module(workflow)
}

/// Sample: one agent + one subflow tool (`ToolImpl::Subflow`) spawning a
/// child agent.
pub(crate) fn subflow() -> IrModule {
    let parent_id = AgentId("orchestrator".into());
    let child_id = AgentId("executor".into());
    let tool_id = ToolId("run-executor".into());

    let parent_agent = Agent {
        id: parent_id.clone(),
        prompt: "You orchestrate sub-agents.".into(),
        model_ref: ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        },
        tool_refs: alloc::vec![tool_id.clone()],
        context: None,
        budget: AgentBudget {
            max_turns: Some(3),
            max_tokens: None,
        },
        produces: alloc::vec![],
        output_schema: None,
        durable: None,
    };

    let child_agent = Agent {
        id: child_id.clone(),
        prompt: "You execute tasks.".into(),
        model_ref: ModelRef {
            backend: "anthropic".into(),
            model_id: "claude-haiku-4-5".into(),
        },
        tool_refs: alloc::vec![],
        context: None,
        budget: AgentBudget {
            max_turns: Some(5),
            max_tokens: None,
        },
        produces: alloc::vec![],
        output_schema: None,
        durable: None,
    };

    let subflow_tool = Tool {
        id: tool_id.clone(),
        impl_: ToolImpl::Subflow {
            target: child_id.clone(),
        },
        capabilities: CapabilityRequirements {
            declared: alloc::vec![],
        },
        spec: ToolSpec {
            name: "run_executor".into(),
            description: "Spawn the executor sub-agent.".into(),
            input_schema: serde_json::json!({"type": "object"}),
        },
    };

    let mut agents = BTreeMap::new();
    agents.insert(parent_id, parent_agent);
    agents.insert(child_id, child_agent);
    let mut tools = BTreeMap::new();
    tools.insert(tool_id, subflow_tool);

    let workflow = Workflow {
        agents,
        tools,
        ..Workflow::default()
    };

    base_module(workflow)
}
