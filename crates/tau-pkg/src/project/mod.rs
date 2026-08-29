//! Project-level `tau.toml` parser, validator, and agent resolution.
//!
//! Lifted from `tau-cli::config` 2026-05-17 so that other binaries
//! (notably `tau-app::serve`) can resolve agents without depending
//! on the tau-cli crate.
//!
//! Distinct from the package-level `tau.toml` defined in tau-domain
//! (per ADR-0002); see ADR-0007 for the Cargo `[package]` vs
//! `[workspace]` precedent that justifies the shared filename.

pub mod agent;
pub mod allow;
pub mod dirs;
#[allow(clippy::module_inception)]
pub mod project;

pub use agent::{build_agent_definition, AgentResolutionError};
pub use allow::{AllowConfig, McpAllowEntry, ToolAllowEntry, ToolBinding, UncheckedAllow};
pub use project::{
    AgentEntry, ConditionConfig, DeliverableEntry, GoalEntry, GoalPredicateConfig, JudgeConfig,
    LocusConfig, OnFailConfig, PipelineConfig, PipelineRunRef, PipelineStepConfig,
    ProjectAgentKind, ProjectConfig, ProjectConfigError, PromptEntry, RequiresEntry, StepEntry,
    ToolBody, ToolEntry,
};
