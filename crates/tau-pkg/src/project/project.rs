//! Project `tau.toml` deserialization, validation, and error taxonomy.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use tau_domain::Capability;

/// Unchecked deserialization shape — fields are typed but no semantic
/// validation has run. Use [`UncheckedProjectConfig::validate`] to
/// produce a [`ProjectConfig`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedProjectConfig {
    /// Top-level `[project]` table.
    pub project: UncheckedProject,
    /// Map of agent id → unchecked agent definition.
    #[serde(default)]
    pub agents: BTreeMap<String, UncheckedAgent>,
    /// Map of tool name → unchecked tool definition (IR lowering, β.2.2).
    #[serde(default)]
    pub tools: BTreeMap<String, UncheckedTool>,
    /// Map of step name → unchecked deterministic step definition (IR lowering, β.2.2).
    #[serde(default)]
    pub steps: BTreeMap<String, UncheckedStep>,
    /// Map of trigger name → unchecked trigger definition (slice 1).
    /// The TOML key is `[trigger.<name>]` (singular), matching the spec.
    #[serde(default, rename = "trigger")]
    pub triggers: BTreeMap<String, UncheckedTrigger>,
    /// Optional `[pipeline]` table with ordered `[[pipeline.steps]]`.
    #[serde(default)]
    pub pipeline: Option<UncheckedPipeline>,
    /// Map of goal id → unchecked goal definition.
    #[serde(default)]
    pub goals: BTreeMap<String, UncheckedGoal>,
    /// Map of deliverable id → unchecked deliverable definition.
    #[serde(default)]
    pub deliverables: BTreeMap<String, UncheckedDeliverable>,
    /// Map of model alias → unchecked `{ backend, model }` entry.
    #[serde(default)]
    pub models: BTreeMap<String, RawModelEntry>,
    /// Top-level package declarations (e.g. `packages = ["anthropic@^1"]`).
    /// Backend names in `[models]` may resolve against these as well as
    /// against agent `package` fields.
    #[serde(default)]
    pub packages: Vec<String>,
}

/// `[project]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedProject {
    /// Free-form project name; required, validated non-empty.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: String,
}

/// `[agents.<id>]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedAgent {
    /// Human-readable agent name displayed in UIs.
    pub display_name: String,
    /// Package reference of the form `<name>@<semver-req>`.
    pub package: String,
    /// Optional `[agents.<id>.requires]` sub-table.
    #[serde(default)]
    pub requires: Option<UncheckedRequires>,
    /// Capability override entries; default empty. Each entry must
    /// match a `kind` declared by the agent's package manifest.
    /// Validation runs in `validate_agent`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<UncheckedCapabilityOverride>,
    /// Free-form `[agents.<id>.config]` sub-table; passed through.
    #[serde(default)]
    pub config: Option<toml::Table>,
    /// Optional `[agents.<id>.prompt]` sub-table.
    #[serde(default)]
    pub prompt: Option<UncheckedPrompt>,
    /// Optional `[agents.<id>.context]` sub-table (β.4).
    #[serde(default)]
    pub context: Option<UncheckedContext>,
    /// Optional `[agents.<id>.durable]` sub-table (ADR-0053).
    #[serde(default)]
    pub durable: Option<UncheckedDurable>,
    // --- IR lowering fields (β.2.2) ---
    /// LLM model identifier (e.g. `"claude-haiku-4-5"`). Used by the IR
    /// lowering pass; ignored by the existing agent-resolution path.
    #[serde(default)]
    pub model: Option<String>,
    /// Tool names this agent is allowed to call. Maps to `Agent::tool_refs`
    /// in the IR. Ignored by the existing agent-resolution path.
    #[serde(default)]
    pub tool_refs: Vec<String>,
    /// Maximum number of turns the agent loop may take.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Maximum tokens (input + output) across the entire run.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Artifact paths / named outputs this agent declares it produces.
    /// Cross-checked against `fs-write` capabilities at validation time
    /// and bound to `[deliverables.*]`/`[goals.*]` loci.
    #[serde(default)]
    pub produces: Vec<String>,
    /// `[[agents.<id>.credentials]]` declarations; default empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credentials: Vec<UncheckedAgentCredential>,
    /// JSON schema describing this agent's structured output. Pass-through
    /// (no deep validation) — mirrors `[steps.<name>].output_schema`. Used
    /// by the IR lowering pass and a later judge-compat build-time check.
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
}

/// `[[agents.<id>.credentials]]` entry — unchecked deserialization.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedAgentCredential {
    /// Logical credential id the chain resolves (e.g. `anthropic_api_key`).
    pub id: String,
    /// Environment-variable name the host injects the resolved secret into.
    pub env: String,
}

/// Validated per-agent credential declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCredential {
    /// Validated logical credential id.
    pub id: tau_ports::CredentialId,
    /// Validated environment-variable name (`[A-Z_][A-Z0-9_]*`).
    pub env: String,
}

/// `[agents.<id>.requires]` sub-table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedRequires {
    /// Required tool packages with explicit source declarations.
    /// Replaces the v0.1 advisory-only `Vec<String>` schema (Tier 2
    /// priority 5).
    #[serde(default)]
    pub tools: Vec<UncheckedRequiredTool>,
    /// Phase 1+; ignored at v0.1.
    #[serde(default)]
    pub packages: Vec<String>,
}

/// One `[[agents.<id>.requires.tools]]` array entry.
///
/// Replaces the v0.1 bare-string form. Each entry must declare a
/// `source` (typed `PackageSource` — string serde format like
/// `"https://example.com/x.git"` or `"<location>#<rev>"`); `version`
/// is optional and defaults to `"*"`.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedRequiredTool {
    /// Package name.
    pub name: String,
    /// Source to fetch from. Reuses `PackageSource::FromStr` serde:
    /// `"<location>"` or `"<location>#<rev>"`.
    pub source: tau_domain::PackageSource,
    /// Optional semver requirement; defaults to `"*"` when absent.
    #[serde(default)]
    pub version: Option<String>,
}

/// `[agents.<id>.prompt]` sub-table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedPrompt {
    /// Inline system prompt; mutually exclusive with `system_file`.
    #[serde(default)]
    pub system: Option<String>,
    /// Path to a system prompt file; mutually exclusive with `system`.
    #[serde(default)]
    pub system_file: Option<PathBuf>,
}

/// `[agents.<id>.context]` sub-table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedContext {
    /// Ordered `[[agents.<id>.context.pipeline]]` entries.
    #[serde(default)]
    pub pipeline: Vec<UncheckedContextStep>,
    /// Per-node config tables: `[agents.<id>.context.steps.<name>]`.
    #[serde(default)]
    pub steps: Option<toml::Table>,
}

/// One `[[agents.<id>.context.pipeline]]` entry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedContextStep {
    /// Transformer name (builtin or custom).
    pub transformer: String,
    /// `builtin` (default) | `custom`.
    #[serde(default)]
    pub kind: Option<String>,
    /// For custom nodes: `native` | `wasm` | `mcp`.
    #[serde(default)]
    pub source: Option<String>,
    /// For custom nodes: providing package ref.
    #[serde(default)]
    pub package: Option<String>,
    /// For custom nodes: declared determinism (`pure` default).
    #[serde(default)]
    pub determinism: Option<String>,
}

/// `[agents.<id>.durable]` — either a bare intent string
/// (`durable = "survive-restarts"`) or the explicit `{ checkpoint, store }`
/// table (ADR-0053). Untagged: serde tries `Explicit` (a table) first, then
/// `Intent` (a string).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UncheckedDurable {
    /// `[agents.<id>.durable] { checkpoint, store }`.
    Explicit(UncheckedDurableExplicit),
    /// `durable = "survive-restarts"`.
    Intent(String),
}

/// Explicit durable table. `deny_unknown_fields` so a typo'd key fails the
/// build rather than being silently dropped.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDurableExplicit {
    /// Checkpoint granularity. A-minimal accepts `"per_turn"` or `"per_tool_call"`.
    pub checkpoint: String,
    /// Durable store. A-minimal accepts only `"file"`.
    pub store: String,
}

/// Single `[[agents.<id>.capabilities]]` array-of-tables entry.
///
/// Note: spec §4.2 defines `allow_methods` / `deny_methods` for
/// `net.http` capability narrowing, but the runtime does not yet
/// enforce method subsets. To prevent silent data loss, this struct
/// uses `#[serde(deny_unknown_fields)]` — a TOML containing
/// `allow_methods` will fail parsing with a clear error pointing at
/// the known fields. When an HTTP tool plugin lands and method
/// enforcement is wired through `compute_effective`, those fields
/// can be added without breaking existing configs.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedCapabilityOverride {
    /// Capability kind discriminator (`fs.read`, `fs.write`, `fs.exec`,
    /// `net.http`, `process.spawn`).
    pub kind: String,
    /// Narrowed allow-list (paths). Optional; absent = "use package's
    /// allow-list verbatim".
    #[serde(default)]
    pub allow_paths: Option<Vec<String>>,
    /// Path globs to subtract from the effective allow-list.
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Narrowed allow-list (hosts) for `net.http`.
    #[serde(default)]
    pub allow_hosts: Option<Vec<String>>,
    /// Hosts to subtract from the effective allow-list (`net.http`).
    #[serde(default)]
    pub deny_hosts: Vec<String>,
    /// Narrowed allow-list (commands) for `process.spawn`.
    #[serde(default)]
    pub allow_commands: Option<Vec<String>>,
    /// Commands to subtract (`process.spawn`).
    #[serde(default)]
    pub deny_commands: Vec<String>,
    /// Narrowed `max_bytes` (only meaningful for `fs.write`).
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

// ----- Pipeline structs (β.2.x) -----

/// Raw `[pipeline]` table (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedPipeline {
    /// Ordered steps from `[[pipeline.steps]]`.
    #[serde(default)]
    pub steps: Vec<UncheckedPipelineStep>,
}

/// Raw `[[pipeline.steps]]` entry (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UncheckedPipelineStep {
    /// Step handle.
    pub id: String,
    /// `"agent:<id>"` | `"tool:<id>"` | `"deterministic:<id>"`.
    pub run: String,
    /// Input template; defaults to `"${input}"` when omitted.
    pub input: Option<String>,
}

/// Validated pipeline.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PipelineConfig {
    /// Ordered, validated steps.
    pub steps: Vec<PipelineStepConfig>,
}

/// Validated pipeline step.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PipelineStepConfig {
    /// Step handle.
    pub id: String,
    /// Resolved run target.
    pub run: PipelineRunRef,
    /// Input template (defaulted to `"${input}"`).
    pub input: String,
}

/// A validated `run = "<kind>:<id>"` reference.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineRunRef {
    /// `agent:<id>`
    Agent(String),
    /// `tool:<id>`
    Tool(String),
    /// `deterministic:<id>`
    Deterministic(String),
    /// `check:<id>` — explicitly position a postcondition check.
    Check(String),
}

// ----- IR lowering structs (β.2.2) -----

/// Author-declared sampling allowlist for an MCP-contracted tool.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingConfig {
    /// Allowlisted LLM model ids. Empty = sampling refused.
    #[serde(default)]
    pub models: Vec<String>,
}

/// Body of a `[tools.<name>]` table — discriminates the implementation kind.
///
/// These variants are mutually exclusive; exactly one must be present in the
/// TOML table. The `#[serde(rename_all = "lowercase")]` matches the wire
/// keys: `native`, `mcp`, `subflow`.
///
/// # Example
///
/// ```toml
/// [tools.read_temp]
/// native = "ReadTemp"
/// capabilities = []
///
/// [tools.weather]
/// mcp = "https://mcp.weather.example.com"
/// capabilities = [{ kind = "net.http" }]
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolBody {
    /// Statically linked native Rust tool. The value is the symbolic name
    /// of the `impl Tool for X` type (e.g. `"ReadTemp"`).
    Native(String),
    /// MCP-contracted external server. The value is the MCP server URL.
    Mcp(String),
    /// Subflow-as-tool: sugar for a `SubflowEdge::Spawn` edge. The value
    /// is the target agent id within the same workflow.
    Subflow(String),
}

/// Unchecked `[tools.<name>]` table (β.2.2).
///
/// Fields use `#[serde(deny_unknown_fields)]` so typos in the TOML are
/// caught at parse time rather than silently discarded.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedTool {
    /// Discriminated implementation body (`native`, `mcp`, or `subflow`).
    #[serde(flatten)]
    pub body: ToolBody,
    /// LLM-visible description of the tool.
    #[serde(default)]
    pub description: String,
    /// JSON schema for the tool's input (freeform; passed through to IR).
    #[serde(default)]
    pub input_schema: serde_json::Value,
    /// Declared capabilities. Each entry uses the `{ kind = "…" }` struct
    /// form (same as package manifests). An empty array means no extra
    /// capabilities beyond ambient.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Sampling allowlist (β.3 — empty/missing = sampling refused).
    #[serde(default)]
    pub sampling: Option<SamplingConfig>,
    /// Roots advertised to the MCP server via `roots/list` (β.3 —
    /// must be subset of `fs.read` caps; checked at lowering time).
    #[serde(default)]
    pub roots: Vec<std::path::PathBuf>,
}

/// Unchecked `[trigger.<name>]` table (slice 1).
///
/// `#[serde(deny_unknown_fields)]` catches typos. Slice-2 fields (`path`,
/// `methods`, `source`) are intentionally absent — a webhook/queue trigger
/// declared today fails fast (either on the unknown field or on the
/// unsupported-kind check in `validate_trigger`).
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedTrigger {
    /// `cron` | `manual` (slice 1).
    pub kind: String,
    /// Entrypoint agent id.
    pub agent: String,
    /// 5-field cron expression (cron only).
    #[serde(default)]
    pub schedule: Option<String>,
    /// IANA timezone name (cron only; defaults to `UTC`).
    #[serde(default)]
    pub timezone: Option<String>,
    /// Re-invocation policy.
    #[serde(default)]
    pub retry: Option<UncheckedRetry>,
}

/// Unchecked `[trigger.<name>.retry]` sub-table.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedRetry {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// Backoff parameters.
    pub backoff: UncheckedBackoff,
    /// Sink reference for exhausted runs.
    #[serde(default)]
    pub dead_letter: Option<String>,
}

/// Unchecked `backoff` inline table.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedBackoff {
    /// `fixed` | `exponential`.
    pub strategy: String,
    /// Base delay, duration string.
    pub base: String,
    /// Max delay, duration string.
    pub max: String,
}

/// Validated `[tools.<name>]` entry produced by `validate()`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ToolEntry {
    /// Tool name (the TOML table key).
    pub name: String,
    /// Discriminated implementation body.
    pub body: ToolBody,
    /// LLM-visible description.
    pub description: String,
    /// JSON schema for the tool's input.
    pub input_schema: serde_json::Value,
    /// Declared capabilities.
    pub capabilities: Vec<Capability>,
    /// Sampling allowlist (β.3 — empty/missing = sampling refused).
    pub sampling: Option<SamplingConfig>,
    /// Roots advertised to the MCP server via `roots/list` (β.3).
    pub roots: Vec<std::path::PathBuf>,
}

/// Unchecked `[steps.<name>]` table (β.2.2).
///
/// A deterministic step is a pure Rust function applied to its input
/// without an LLM call. Fields use `#[serde(deny_unknown_fields)]`.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedStep {
    /// Symbolic name of the Rust fn (e.g. `"parse_celsius"`). Maps to
    /// the `fn_ref` field in `Deterministic`.
    pub deterministic: String,
    /// JSON schema for the step's input.
    #[serde(default)]
    pub input_schema: serde_json::Value,
    /// JSON schema for the step's output.
    #[serde(default)]
    pub output_schema: serde_json::Value,
}

/// Validated `[steps.<name>]` entry.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct StepEntry {
    /// Step name (the TOML table key).
    pub name: String,
    /// Symbolic name of the Rust fn.
    pub fn_name: String,
    /// JSON schema for the step's input.
    pub input_schema: serde_json::Value,
    /// JSON schema for the step's output.
    pub output_schema: serde_json::Value,
}

/// Validated `[trigger.<name>]` entry produced by `validate()`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct TriggerEntry {
    /// Trigger name (the TOML table key).
    pub name: String,
    /// `cron` | `manual`.
    pub kind: String,
    /// Entrypoint agent id (existence checked at IR lowering, not here).
    pub agent: String,
    /// 5-field cron expression (cron only).
    pub schedule: Option<String>,
    /// IANA timezone (defaults to `UTC` for cron; empty string for manual).
    /// Callers distinguish "no timezone" via `.is_empty()` (manual triggers).
    pub timezone: String,
    /// Re-invocation policy.
    pub retry: Option<RetryEntry>,
}

/// Validated `[trigger.<name>.retry]` entry.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RetryEntry {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// `fixed` | `exponential`.
    pub backoff_strategy: String,
    /// Base delay, duration string.
    pub backoff_base: String,
    /// Max delay, duration string.
    pub backoff_max: String,
    /// Sink reference for exhausted runs.
    pub dead_letter: Option<String>,
}

// ----- Validated shapes -----

/// Raw `[goals.<id>]` table (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedGoal {
    /// Read locus: a filesystem path or `steps.<id>.output`.
    pub evaluates: String,
    /// Menu predicate name (mutually exclusive with `fn`).
    #[serde(default)]
    pub check: Option<String>,
    /// Regex for `check = "matches"`.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Expected value for `check = "equals"`.
    #[serde(default)]
    pub equals: Option<String>,
    /// Threshold for `check = "min_count"`.
    #[serde(default)]
    pub min_count: Option<u64>,
    /// JSON schema for `check = "schema_valid"`.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    /// Native-fn escape hatch (`<crate>::<path>`), mutually exclusive with `check`.
    #[serde(default, rename = "fn")]
    pub r#fn: Option<String>,
}

/// A read locus: a filesystem path or a named step output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocusConfig {
    /// Filesystem path.
    Path(String),
    /// `steps.<id>.output` → the step id.
    Output(String),
}

/// Validated goal predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalPredicateConfig {
    /// Locus resolves to something.
    Exists,
    /// Resolves and is non-empty.
    NonEmpty,
    /// Equals the given literal.
    Equals(String),
    /// Matches the given regex.
    Matches(String),
    /// At least N items (lines/array entries).
    MinCount(u64),
    /// Validates against the given JSON schema.
    SchemaValid(serde_json::Value),
    /// Registered native fn (`<crate>::<path>`).
    NativeFn(String),
}

/// Validated `[goals.<id>]` entry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct GoalEntry {
    /// Goal id (table key).
    pub id: String,
    /// Read locus.
    pub evaluates: LocusConfig,
    /// Verification predicate.
    pub predicate: GoalPredicateConfig,
}

/// Raw `[models.<alias>]` inline table (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawModelEntry {
    /// Backend package name.
    pub backend: String,
    /// Vendor model id.
    pub model: String,
}

/// Raw `[deliverables.<id>]` table (pre-validation).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncheckedDeliverable {
    /// Filesystem path locus (mutually exclusive with `output`).
    #[serde(default)]
    pub path: Option<String>,
    /// Step output locus (mutually exclusive with `path`). Use the form
    /// `steps.<id>.output`.
    #[serde(default)]
    pub output: Option<String>,
    /// Natural-language acceptance criterion evaluated by the judge.
    pub must_satisfy: String,
    /// Failure handling: `"abort"` (default) or `"retry"`.
    #[serde(default)]
    pub on_fail: Option<String>,
    /// Maximum check evaluations (default: 1 for abort, 3 for retry).
    #[serde(default)]
    pub max_attempts: Option<u32>,
    /// Pipeline step id to rewind to on retry. Defaults to the producer
    /// step when absent.
    #[serde(default)]
    pub retry_from: Option<String>,
    /// Model override for the built-in judge (runtime no-op in v1).
    /// Mutually exclusive with `judge`.
    #[serde(default)]
    pub judge_model: Option<String>,
    /// Custom judge agent id (`[agents.<id>]`). Mutually exclusive with
    /// `judge_model`.
    #[serde(default)]
    pub judge: Option<String>,
}

/// Failure handling for a deliverable check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnFailConfig {
    /// Exit non-zero on first failure.
    Abort,
    /// Rewind to the gate step and re-run.
    Retry,
}

/// Who evaluates a deliverable's content.
#[derive(Debug, Clone, PartialEq)]
pub enum JudgeConfig {
    /// The canonical (implicit) judge, optional `judge_model` alias override.
    Default {
        /// `judge_model` override (runtime no-op in v1).
        model: Option<String>,
    },
    /// A user `[agents.*]` used as judge.
    Agent(String),
}

/// Validated `[deliverables.<id>]` entry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct DeliverableEntry {
    /// Deliverable id (table key).
    pub id: String,
    /// Produced locus.
    pub locus: LocusConfig,
    /// Natural-language acceptance criterion.
    pub must_satisfy: String,
    /// Failure handling strategy.
    pub on_fail: OnFailConfig,
    /// Maximum check evaluations (>= 1).
    pub max_attempts: u32,
    /// Rewind point step id (None = default to producer step).
    pub retry_from: Option<String>,
    /// Who judges the content.
    pub judge: JudgeConfig,
    /// Resolved producing agent id (filled by validation).
    ///
    /// Empty string before `validate_postconditions` runs; afterwards
    /// always holds the unique agent id whose `produces` list covers
    /// this deliverable's locus.
    pub producer: String,
    /// Resolved retry gate step id (filled by validation; empty for abort).
    ///
    /// For `on_fail = "retry"` deliverables, holds the pipeline step id
    /// the runtime rewinds to on failure. Set by `validate_postconditions`;
    /// always empty for `on_fail = "abort"` deliverables.
    pub gate: String,
}

/// Validated `[models.<alias>]` entry: a concrete backend + vendor model id.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Backend package name (must be a declared package).
    pub backend: String,
    /// Vendor model id (e.g. `"claude-haiku-4-5"`). Trusted; not validated offline.
    pub model: String,
}

/// Validated project config. Constructed via
/// [`UncheckedProjectConfig::validate`] only.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProjectConfig {
    /// Validated, non-empty project name.
    pub project_name: String,
    /// Optional description (may be empty).
    pub description: String,
    /// Map of agent id → validated agent entry.
    pub agents: BTreeMap<String, AgentEntry>,
    /// Map of tool name → validated tool entry (IR lowering, β.2.2).
    pub tools: BTreeMap<String, ToolEntry>,
    /// Map of step name → validated step entry (IR lowering, β.2.2).
    pub steps: BTreeMap<String, StepEntry>,
    /// Map of trigger name → validated trigger entry (slice 1).
    pub triggers: BTreeMap<String, TriggerEntry>,
    /// Optional validated pipeline.
    pub pipeline: Option<PipelineConfig>,
    /// Map of goal id → validated goal entry.
    pub goals: BTreeMap<String, GoalEntry>,
    /// Map of deliverable id → validated deliverable entry.
    pub deliverables: BTreeMap<String, DeliverableEntry>,
    /// Map of model alias → validated `{ backend, model }`.
    pub models: BTreeMap<String, ModelEntry>,
    /// Top-level declared packages (raw strings like `"anthropic@^1"`).
    /// Names parsed from these are valid `[models]` backend identifiers.
    pub packages: Vec<String>,
}

/// Validated context-pipeline step.
#[derive(Debug, Clone)]
pub struct ContextStepEntry {
    /// Transformer name.
    pub transformer: String,
    /// `pure` | `llm_backed` | `stateful`.
    pub determinism: String,
    /// `Some((source, package))` for custom nodes, else `None` (builtin).
    pub custom: Option<(String, String)>,
    /// Per-node config from `[...context.steps.<name>]`, as serde_json values.
    pub config: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Validated `[agents.<id>.durable]` (ADR-0053 + EPIC 6.1). Present only
/// when the agent opts into durable execution.
#[derive(Debug, Clone)]
pub enum DurableEntry {
    /// Validated intent string (currently only `"survive-restarts"`).
    Intent(String),
    /// Validated explicit form. `checkpoint ∈ {per_turn, per_tool_call}`, `store == "file"`.
    Explicit {
        /// Validated checkpoint granularity.
        checkpoint: String,
        /// Validated durable store.
        store: String,
    },
}

/// Validated entry for a single agent.
///
/// # Example
///
/// ```
/// use std::collections::BTreeMap;
/// use tau_pkg::project::project::{AgentEntry, RequiresEntry, PromptEntry};
///
/// let entry = AgentEntry::new(
///     "reviewer".to_string(),
///     "Code Reviewer".to_string(),
///     "code-reviewer@^0.1".to_string(),
///     RequiresEntry::default(),
///     BTreeMap::new(),
///     PromptEntry::None,
///     vec![],
/// );
/// assert_eq!(entry.id, "reviewer");
/// assert_eq!(entry.display_name, "Code Reviewer");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct AgentEntry {
    /// Agent id (the table key under `[agents.<id>]`).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Package reference (`<name>@<semver-req>`).
    pub package: String,
    /// Validated `requires` block.
    pub requires: RequiresEntry,
    /// Free-form configuration table.
    pub config: BTreeMap<String, toml::Value>,
    /// Validated prompt selection.
    pub prompt: PromptEntry,
    /// Project-supplied capability overrides (raw, validated only for
    /// shape + duplicate-kind at parse time). The intersect-vs-manifest
    /// check runs at `tau run` time (in tau-runtime) and at
    /// `tau list --capabilities` rendering time. Empty = no override
    /// (effective grant = package manifest verbatim).
    pub capability_overrides: Vec<crate::capability_override::CapabilityOverride>,
    // --- IR lowering fields (β.2.2) ---
    /// LLM model identifier (IR lowering use). Empty string if absent.
    pub model: String,
    /// Tool names this agent references (IR lowering use).
    pub tool_refs: Vec<String>,
    /// Maximum turns (IR lowering use).
    pub max_turns: Option<u32>,
    /// Maximum tokens (IR lowering use).
    pub max_tokens: Option<u64>,
    /// Artifact paths / named outputs this agent declares it produces.
    /// Cross-checked against `fs-write` capabilities at validation time
    /// and bound to `[deliverables.*]`/`[goals.*]` loci.
    pub produces: Vec<String>,
    /// Validated `[agents.<id>.context]` pipeline (β.4). Empty = no
    /// context-management pipeline declared (default behaviour).
    pub context: Vec<ContextStepEntry>,
    /// Validated credential declarations (β.5).
    pub credentials: Vec<AgentCredential>,
    /// JSON schema describing this agent's structured output (IR lowering
    /// use). `None` = unspecified. Pass-through; any well-formed JSON value
    /// is accepted (no deep JSON-schema validation).
    pub output_schema: Option<serde_json::Value>,
    /// Validated `[agents.<id>.durable]` block (ADR-0053). `None` = not
    /// durable (whole-bundle reentrant only).
    pub durable: Option<DurableEntry>,
}

impl AgentEntry {
    /// Construct an `AgentEntry`. Required because the struct is
    /// `#[non_exhaustive]` — callers outside this crate cannot use
    /// struct-literal syntax.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        display_name: String,
        package: String,
        requires: RequiresEntry,
        config: BTreeMap<String, toml::Value>,
        prompt: PromptEntry,
        capability_overrides: Vec<crate::capability_override::CapabilityOverride>,
    ) -> Self {
        Self {
            id,
            display_name,
            package,
            requires,
            config,
            prompt,
            capability_overrides,
            model: String::new(),
            tool_refs: Vec::new(),
            max_turns: None,
            max_tokens: None,
            produces: Vec::new(),
            context: Vec::new(),
            credentials: Vec::new(),
            output_schema: None,
            durable: None,
        }
    }
}

/// Validated `requires` sub-table.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct RequiresEntry {
    /// Required tool packages with explicit source + optional version
    /// constraint. Resolved + installed at `tau run`/`tau chat`/`tau resolve`
    /// time via `tau_pkg::resolve_requires_tools`.
    pub tools: Vec<crate::RequiredTool>,
}

/// Validated prompt selection. `system` and `system_file` are mutually
/// exclusive, so this enum encodes the three valid states.
///
/// # Example
///
/// ```
/// use tau_pkg::project::project::PromptEntry;
///
/// let p = PromptEntry::Inline("You are a careful reviewer.".to_string());
/// assert!(matches!(p, PromptEntry::Inline(_)));
///
/// let p2 = PromptEntry::File(std::path::PathBuf::from("prompts/r.md"));
/// assert!(matches!(p2, PromptEntry::File(_)));
///
/// assert!(matches!(PromptEntry::None, PromptEntry::None));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub enum PromptEntry {
    /// No prompt configured.
    #[default]
    None,
    /// Inline prompt string.
    Inline(String),
    /// Path to an external prompt file.
    File(PathBuf),
}

// ----- Errors -----

/// Errors produced when loading or validating a project `tau.toml`.
///
/// # Example
///
/// ```
/// use tau_pkg::project::project::ProjectConfigError;
///
/// let err = ProjectConfigError::EmptyProjectName;
/// let display = format!("{err}");
/// assert!(display.contains("non-empty"));
///
/// let err2 = ProjectConfigError::AgentValidation {
///     id: "reviewer".to_string(),
///     message: "display_name must be non-empty".to_string(),
/// };
/// let display2 = format!("{err2}");
/// assert!(display2.contains("reviewer"));
/// assert!(display2.contains("display_name"));
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ProjectConfigError {
    /// No `tau.toml` file found.
    #[error("project tau.toml not found in scope (run `tau init` to create one)")]
    NotFound,

    /// Filesystem read failure (other than "not found").
    #[error("failed to read project tau.toml at {path:?}: {source}")]
    Read {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// TOML parse failure.
    #[error("failed to parse project tau.toml at {path:?}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying TOML parse error.
        #[source]
        source: toml::de::Error,
    },

    /// `project.name` was empty after trimming.
    #[error("project name must be non-empty")]
    EmptyProjectName,

    /// Generic per-agent semantic validation failure.
    #[error("agent {id:?}: {message}")]
    AgentValidation {
        /// Agent id that failed validation.
        id: String,
        /// Human-readable message describing the violation.
        message: String,
    },

    /// Project override on `kind` expanded the package's grant. Carries
    /// the agent id, the failing kind, and a human-readable reason.
    #[error("agent {id:?}: capability override on {kind:?} expands the package's grant: {reason}")]
    CapabilityOverrideExpands {
        /// Agent id whose override failed validation.
        id: String,
        /// The capability kind that expanded.
        kind: String,
        /// Human-readable reason from `compute_effective`.
        reason: String,
    },

    /// Agent declared both `prompt.system` and `prompt.system_file`.
    #[error("agent {id:?}: prompt requires exactly one of `system` or `system_file`, found both")]
    PromptAmbiguous {
        /// Agent id whose prompt block was ambiguous.
        id: String,
    },

    /// Bare-string entry in `[agents.<id>.requires.tools]` is no longer
    /// supported. Each entry must use the struct form with a `source`
    /// declaration. Tier 2 priority 5 closed the v0.1 advisory-only
    /// behavior. Reserved for future custom-deserializer use; at v0.1
    /// the bare-string rejection happens via serde's natural type-mismatch
    /// error on `UncheckedRequiredTool` deserialization.
    #[error(
        "agent {agent_id:?}: requires.tools[{index}]: bare-string {value:?} no longer supported; use struct form with `source` per spec docs/superpowers/specs/2026-04-30-transitive-deps-design.md §4"
    )]
    RequiresToolsBareStringRejected {
        /// Agent id whose entry was rejected.
        agent_id: String,
        /// Index in the tools array of the offending entry.
        index: usize,
        /// The bare string value as it appeared in the TOML.
        value: String,
    },

    // --- IR lowering errors (β.2.2) ---
    /// In-memory TOML parse failure (for `parse_str`, as opposed to file-based `from_path`).
    #[error("failed to parse tau.toml from string: {source}")]
    ParseStr {
        /// Underlying TOML parse error.
        #[source]
        source: toml::de::Error,
    },

    /// A `[tools.<name>]` entry failed validation.
    #[error("tool {name:?}: {message}")]
    ToolValidation {
        /// Tool name that failed.
        name: String,
        /// Human-readable reason.
        message: String,
    },

    /// A `[steps.<name>]` entry failed validation.
    #[error("step {name:?}: {message}")]
    StepValidation {
        /// Step name that failed.
        name: String,
        /// Human-readable reason.
        message: String,
    },

    /// A `[trigger.<name>]` entry failed validation.
    #[error("trigger {name:?}: {message}")]
    TriggerValidation {
        /// Trigger name that failed.
        name: String,
        /// Human-readable reason.
        message: String,
    },

    /// A `[pipeline]` table was declared with no `[[pipeline.steps]]`
    /// entries. An empty pipeline is rejected at build time rather than
    /// silently falling through to the single-agent path.
    #[error("pipeline must declare at least one step")]
    EmptyPipeline,

    /// A `[[pipeline.steps]]` entry failed validation.
    #[error("pipeline step {id:?}: {message}")]
    PipelineValidation {
        /// Step id that failed.
        id: String,
        /// Human-readable reason.
        message: String,
    },

    /// `[tools.<name>] mcp = "..."` URL has an unsupported scheme.
    #[error("tool {tool:?}: unsupported MCP URL scheme: {url:?}")]
    UnsupportedMcpUrl {
        /// Tool name.
        tool: String,
        /// Offending URL.
        url: String,
    },

    /// A `[goals.<id>]` entry failed semantic validation.
    #[error("goal {id:?}: {message}")]
    GoalValidation {
        /// Goal id that failed.
        id: String,
        /// Human-readable reason.
        message: String,
    },

    /// A `[deliverables.<id>]` entry failed semantic validation.
    #[error("deliverable {id:?}: {message}")]
    DeliverableValidation {
        /// Deliverable id that failed.
        id: String,
        /// Human-readable reason.
        message: String,
    },

    /// `judge` and `judge_model` were both set on a deliverable.
    #[error(
        "deliverable '{id}' sets both judge_model and judge — a custom judge brings its own model"
    )]
    JudgeAndModelConflict {
        /// Deliverable id that had the conflict.
        id: String,
    },

    // --- Task 4: producer binding + capability coverage ---
    /// A deliverable declares a locus no agent's `produces` covers.
    #[error("deliverable '{id}' has no producer: no step declares produces = [{locus:?}]")]
    DeliverableNoProducer {
        /// Deliverable id.
        id: String,
        /// The locus string (path or output ref) that was unmatched.
        locus: String,
    },

    /// More than one agent claims to produce the deliverable's locus.
    #[error(
        "deliverable '{id}' is produced by multiple agents ({agents:?}); a deliverable must bind to exactly one producer"
    )]
    DeliverableAmbiguousProducer {
        /// Deliverable id.
        id: String,
        /// Sorted agent ids that all claim the same locus.
        agents: Vec<String>,
    },

    /// The producing agent holds no fs-write capability covering the path.
    #[error(
        "step '{agent}' declares it produces '{path}' but holds no fs-write capability covering that path"
    )]
    DeliverableProducerLacksCapability {
        /// Deliverable id.
        id: String,
        /// Agent id that declared the `produces` entry.
        agent: String,
        /// The path the agent claims to produce.
        path: String,
    },

    // --- Task 5: gate-position + retry-span guarantees ---
    /// `retry_from` names a step that runs after the producer step.
    #[error(
        "deliverable '{id}' has retry_from = \"{gate}\" but '{gate}' runs after producer '{producer}' \
        — the gate must be at or before the producer"
    )]
    GateAfterProducer {
        /// Deliverable id.
        id: String,
        /// The gate step id named in `retry_from`.
        gate: String,
        /// The producer agent id whose pipeline step is the upper bound.
        producer: String,
    },

    /// The retry span contains no non-deterministic (agent) step.
    #[error(
        "deliverable '{id}' sets on_fail = \"retry\" but the retry span contains no \
        non-deterministic step; retrying cannot change the result"
    )]
    RetrySpanNoLlm {
        /// Deliverable id.
        id: String,
    },

    /// `retry_from` names a step id that does not exist in the pipeline.
    #[error("deliverable '{id}' has retry_from = \"{gate}\" but no pipeline step has that id")]
    UnknownRetryFrom {
        /// Deliverable id.
        id: String,
        /// The unknown step id from `retry_from`.
        gate: String,
    },

    // --- Task 6: regex compiles + judge resolution ---
    /// A `check = "matches"` goal has a pattern that is not a valid regex.
    #[error("goal '{id}' has check = \"matches\" but its pattern is not a valid regex: {message}")]
    BadGoalRegex {
        /// Goal id.
        id: String,
        /// The regex error message.
        message: String,
    },

    /// A deliverable's `judge` names an agent that is not defined.
    #[error("deliverable '{id}' sets judge = \"{judge}\" but no [agents.{judge}] is defined")]
    UnknownJudgeAgent {
        /// Deliverable id.
        id: String,
        /// The unknown agent id named as judge.
        judge: String,
    },

    /// A credential declaration on an agent failed validation.
    #[error("agent {id:?}: credential declaration invalid: {message}")]
    CredentialDeclaration {
        /// Agent id whose credential declaration failed.
        id: String,
        /// Human-readable reason.
        message: String,
    },

    // --- Task 4 (D7 stage 1): model alias + backend validation ---
    /// A `[models]` entry is missing `backend` or `model`.
    #[error("model alias `{alias}` is malformed: needs both `backend` and `model`")]
    MalformedModelEntry {
        /// The alias that was malformed.
        alias: String,
    },

    /// A `[models]` entry references a backend that is not a declared package.
    #[error("model alias `{alias}` references undeclared backend `{backend}`")]
    ModelBackendNotDeclared {
        /// The alias whose backend was undeclared.
        alias: String,
        /// The backend package name that was not declared.
        backend: String,
    },

    /// `agent.model` / `judge_model` references an alias absent from `[models]`.
    #[error("{referrer} references unknown model alias `{alias}`")]
    UnknownModelAlias {
        /// Human-readable description of which field made the reference.
        referrer: String,
        /// The alias that was not in `[models]`.
        alias: String,
    },

    /// An agent declares no `model`.
    #[error("agent `{agent}` has no `model` (declare one in `[models]` and reference it)")]
    MissingAgentModel {
        /// The agent id that had no model.
        agent: String,
    },
}

// ----- Validation logic -----

impl UncheckedProjectConfig {
    /// Validate semantic invariants and produce a [`ProjectConfig`].
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::project::project::UncheckedProjectConfig;
    ///
    /// let toml_str = r#"
    /// [project]
    /// name = "my-project"
    /// "#;
    /// let unchecked: UncheckedProjectConfig = toml::from_str(toml_str).expect("parse");
    /// let config = unchecked.validate().expect("valid config");
    /// assert_eq!(config.project_name, "my-project");
    /// assert!(config.agents.is_empty());
    /// ```
    pub fn validate(self) -> Result<ProjectConfig, ProjectConfigError> {
        if self.project.name.trim().is_empty() {
            return Err(ProjectConfigError::EmptyProjectName);
        }

        let mut agents = BTreeMap::new();
        for (id, raw) in self.agents {
            agents.insert(id.clone(), validate_agent(id, raw)?);
        }

        let mut tools = BTreeMap::new();
        for (name, raw) in self.tools {
            tools.insert(name.clone(), validate_tool(name, raw)?);
        }

        let mut steps = BTreeMap::new();
        for (name, raw) in self.steps {
            steps.insert(name.clone(), validate_step(name, raw)?);
        }

        let mut triggers = BTreeMap::new();
        for (name, raw) in self.triggers {
            triggers.insert(name.clone(), validate_trigger(name, raw)?);
        }

        let pipeline = match &self.pipeline {
            Some(p) => Some(validate_pipeline(p)?),
            None => None,
        };

        let mut goals = BTreeMap::new();
        for (id, raw) in self.goals {
            goals.insert(id.clone(), validate_goal(id, raw)?);
        }

        let mut deliverables = BTreeMap::new();
        for (id, raw) in self.deliverables {
            deliverables.insert(id.clone(), validate_deliverable(id, raw)?);
        }

        let models: BTreeMap<String, ModelEntry> = self
            .models
            .into_iter()
            .map(|(alias, raw)| {
                (
                    alias,
                    ModelEntry {
                        backend: raw.backend,
                        model: raw.model,
                    },
                )
            })
            .collect();

        let mut result = ProjectConfig {
            project_name: self.project.name,
            description: self.project.description,
            agents,
            tools,
            steps,
            triggers,
            pipeline,
            goals,
            deliverables,
            models,
            packages: self.packages,
        };

        validate_postconditions(&mut result)?;
        validate_models(&result)?;

        Ok(result)
    }
}

fn validate_agent(id: String, raw: UncheckedAgent) -> Result<AgentEntry, ProjectConfigError> {
    if raw.display_name.trim().is_empty() {
        return Err(ProjectConfigError::AgentValidation {
            id,
            message: "display_name must be non-empty".into(),
        });
    }
    if raw.package.trim().is_empty() {
        return Err(ProjectConfigError::AgentValidation {
            id,
            message: "package must be non-empty".into(),
        });
    }

    // Convert the typed unchecked overrides into runtime-shape
    // CapabilityOverride values. The intersect-vs-manifest check runs
    // at `tau run` time (Task 5) and at `tau list --capabilities`
    // rendering time (Task 9); here we only validate parse-local
    // invariants (duplicate kinds).
    let capability_overrides: Vec<crate::capability_override::CapabilityOverride> = raw
        .capabilities
        .iter()
        .map(unchecked_to_capability_override)
        .collect();

    {
        use std::collections::BTreeSet;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for ov in &capability_overrides {
            if !seen.insert(ov.kind.clone()) {
                return Err(ProjectConfigError::CapabilityOverrideExpands {
                    id: id.clone(),
                    kind: ov.kind.clone(),
                    reason: "duplicate kind in project override".into(),
                });
            }
        }
    }

    let prompt = match raw.prompt {
        None => PromptEntry::None,
        Some(p) => match (p.system, p.system_file) {
            (Some(s), None) => PromptEntry::Inline(s),
            (None, Some(f)) => PromptEntry::File(f),
            (Some(_), Some(_)) => return Err(ProjectConfigError::PromptAmbiguous { id }),
            (None, None) => PromptEntry::None,
        },
    };

    let requires = match raw.requires {
        None => RequiresEntry::default(),
        Some(r) => {
            let mut tools: Vec<crate::RequiredTool> = Vec::with_capacity(r.tools.len());
            for raw_tool in r.tools {
                let name = tau_domain::PackageName::from_str(&raw_tool.name).map_err(|e| {
                    ProjectConfigError::AgentValidation {
                        id: id.clone(),
                        message: format!(
                            "requires.tools entry {:?}: invalid name: {e}",
                            raw_tool.name
                        ),
                    }
                })?;
                let version_req = match raw_tool.version.as_deref() {
                    None | Some("") => semver::VersionReq::STAR,
                    Some(s) => semver::VersionReq::parse(s).map_err(|e| {
                        ProjectConfigError::AgentValidation {
                            id: id.clone(),
                            message: format!(
                                "requires.tools entry {:?}: invalid version {s:?}: {e}",
                                raw_tool.name
                            ),
                        }
                    })?,
                };
                tools.push(crate::RequiredTool::new(name, raw_tool.source, version_req));
            }
            RequiresEntry { tools }
        }
    };

    let config = raw
        .config
        .map(|t| t.into_iter().collect::<BTreeMap<_, _>>())
        .unwrap_or_default();

    // β.4: validate the optional context pipeline. Per-node config is
    // stored as serde_json::Value so Task 4 lowering into
    // tau_ir::context::ContextStep.config is a trivial clone.
    let context: Vec<ContextStepEntry> = match raw.context {
        None => Vec::new(),
        Some(ctx) => {
            let steps_tbl = ctx.steps.unwrap_or_default();
            let mut out = Vec::with_capacity(ctx.pipeline.len());
            for s in ctx.pipeline {
                let determinism = s.determinism.unwrap_or_else(|| "pure".into());
                let custom = match s.kind.as_deref() {
                    Some("custom") => {
                        let source = s.source.clone().ok_or_else(|| {
                            ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!(
                                    "context custom node {:?} needs `source`",
                                    s.transformer
                                ),
                            }
                        })?;
                        let package = s.package.clone().ok_or_else(|| {
                            ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!(
                                    "context custom node {:?} needs `package`",
                                    s.transformer
                                ),
                            }
                        })?;
                        if source != "native" {
                            return Err(ProjectConfigError::AgentValidation {
                                id: id.clone(),
                                message: format!(
                                    "context node {:?}: source {source:?} not supported in v1 (only `native`)",
                                    s.transformer
                                ),
                            });
                        }
                        // TODO(β.4.x): custom-node capability subset check —
                        // custom nodes may declare capabilities that must be a
                        // subset of the agent's grants. Task 13 adds the
                        // rejection test. v1 builtins declare none.
                        Some((source, package))
                    }
                    _ => None,
                };
                let node_cfg: std::collections::BTreeMap<String, serde_json::Value> = steps_tbl
                    .get(&s.transformer)
                    .and_then(|v| v.as_table())
                    .map(|t| {
                        t.iter()
                            .filter_map(|(k, v)| {
                                serde_json::to_value(v.clone())
                                    .ok()
                                    .map(|jv| (k.clone(), jv))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(ContextStepEntry {
                    transformer: s.transformer,
                    determinism,
                    custom,
                    config: node_cfg,
                });
            }
            out
        }
    };

    // ADR-0053: validate the optional durable-execution block. A-minimal
    // accepts `checkpoint = "per_turn"` or `checkpoint = "per_tool_call"` with `store = "file"`; any
    // other value is a build error (build-time enforcement — the
    // `#[non_exhaustive]` IR enums grow additively for finer granularities
    // / stores later).
    let durable: Option<DurableEntry> = match raw.durable {
        None => None,
        Some(UncheckedDurable::Intent(s)) => {
            if s != "survive-restarts" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable {s:?} unsupported (accepts \"survive-restarts\" or an explicit {{ checkpoint, store }} table)"
                    ),
                });
            }
            Some(DurableEntry::Intent(s))
        }
        Some(UncheckedDurable::Explicit(d)) => {
            if d.checkpoint != "per_turn" && d.checkpoint != "per_tool_call" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable.checkpoint {:?} unsupported (accepts \"per_turn\" or \"per_tool_call\")",
                        d.checkpoint
                    ),
                });
            }
            if d.store != "file" {
                return Err(ProjectConfigError::AgentValidation {
                    id: id.clone(),
                    message: format!(
                        "durable.store {:?} unsupported (A-minimal accepts only \"file\")",
                        d.store
                    ),
                });
            }
            Some(DurableEntry::Explicit {
                checkpoint: d.checkpoint,
                store: d.store,
            })
        }
    };

    // β.5: validate credential declarations.
    let mut credentials = Vec::with_capacity(raw.credentials.len());
    let mut seen_envs = std::collections::BTreeSet::new();
    for cred in raw.credentials {
        let cid = tau_ports::CredentialId::parse(cred.id.clone()).map_err(|e| {
            ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: format!("invalid id {:?}: {}", cred.id, e.reason),
            }
        })?;
        if !is_valid_env_name(&cred.env) {
            return Err(ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: format!(
                    "invalid env var name {:?} (must match [A-Z_][A-Z0-9_]*)",
                    cred.env
                ),
            });
        }
        if !seen_envs.insert(cred.env.clone()) {
            return Err(ProjectConfigError::CredentialDeclaration {
                id: id.clone(),
                message: format!("duplicate env var {:?}", cred.env),
            });
        }
        credentials.push(AgentCredential {
            id: cid,
            env: cred.env,
        });
    }

    Ok(AgentEntry {
        id,
        display_name: raw.display_name,
        package: raw.package,
        requires,
        config,
        prompt,
        capability_overrides,
        model: raw.model.unwrap_or_default(),
        tool_refs: raw.tool_refs,
        max_turns: raw.max_turns,
        max_tokens: raw.max_tokens,
        produces: raw.produces,
        context,
        credentials,
        output_schema: raw.output_schema,
        durable,
    })
}

/// Returns true if `name` is a valid POSIX-ish env var name: `[A-Z_][A-Z0-9_]*`.
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn validate_tool(name: String, raw: UncheckedTool) -> Result<ToolEntry, ProjectConfigError> {
    match &raw.body {
        ToolBody::Native(fn_name) => {
            if fn_name.trim().is_empty() {
                return Err(ProjectConfigError::ToolValidation {
                    name,
                    message: "native body must specify a non-empty fn name".into(),
                });
            }
        }
        ToolBody::Mcp(url) => {
            if url.trim().is_empty() {
                return Err(ProjectConfigError::ToolValidation {
                    name,
                    message: "mcp body must specify a non-empty url".into(),
                });
            }
            let url_trim = url.trim();
            if !(url_trim.starts_with("stdio:")
                || url_trim.starts_with("http://")
                || url_trim.starts_with("https://")
                || url_trim.starts_with("cassette:"))
            {
                return Err(ProjectConfigError::UnsupportedMcpUrl {
                    tool: name.clone(),
                    url: url.clone(),
                });
            }
        }
        ToolBody::Subflow(target) => {
            if target.trim().is_empty() {
                return Err(ProjectConfigError::ToolValidation {
                    name,
                    message: "subflow body must specify a non-empty target agent id".into(),
                });
            }
        }
    }
    Ok(ToolEntry {
        name,
        body: raw.body,
        description: raw.description,
        input_schema: raw.input_schema,
        capabilities: raw.capabilities,
        sampling: raw.sampling,
        roots: raw.roots,
    })
}

fn validate_step(name: String, raw: UncheckedStep) -> Result<StepEntry, ProjectConfigError> {
    if raw.deterministic.trim().is_empty() {
        return Err(ProjectConfigError::StepValidation {
            name,
            message: "deterministic fn name must be non-empty".into(),
        });
    }
    Ok(StepEntry {
        name,
        fn_name: raw.deterministic,
        input_schema: raw.input_schema,
        output_schema: raw.output_schema,
    })
}

fn validate_trigger(
    name: String,
    raw: UncheckedTrigger,
) -> Result<TriggerEntry, ProjectConfigError> {
    let err = |message: String| ProjectConfigError::TriggerValidation {
        name: name.clone(),
        message,
    };

    if raw.agent.trim().is_empty() {
        return Err(err("agent must be non-empty".into()));
    }

    // Slice 1 supports cron + manual only.
    match raw.kind.as_str() {
        "cron" => {}
        "manual" => {
            if raw.schedule.is_some() {
                return Err(err("manual triggers take no schedule".into()));
            }
            if raw.timezone.is_some() {
                return Err(err("manual triggers take no timezone".into()));
            }
        }
        "webhook" | "queue" => {
            return Err(err(format!(
                "kind {:?} is not supported yet (slice 1 supports cron and manual); \
                 webhook/queue arrive in slice 2",
                raw.kind
            )));
        }
        other => {
            return Err(err(format!(
                "unknown kind {other:?}; expected cron or manual"
            )));
        }
    }

    // cron-specific validation.
    let (schedule, timezone) = if raw.kind == "cron" {
        let sched = raw
            .schedule
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| err("cron triggers require a non-empty schedule".to_string()))?;
        let field_count = sched.split_whitespace().count();
        if field_count != 5 {
            return Err(err(format!(
                "cron schedule must have 5 whitespace-separated fields, found {field_count}"
            )));
        }
        // Build-time enforcement: range-check any field that is a plain
        // non-negative integer. Fields using `*`, ranges (`-`), lists (`,`),
        // or steps (`/`) are passed through for the host scheduler to
        // interpret (the full cron grammar is the host's job) — but a bare
        // out-of-range integer like hour `25` is a build-time-detectable
        // mistake, so we reject it here.
        const CRON_RANGES: [(&str, u32, u32); 5] = [
            ("minute", 0, 59),
            ("hour", 0, 23),
            ("day-of-month", 1, 31),
            ("month", 1, 12),
            ("day-of-week", 0, 7),
        ];
        for (field, (fname, lo, hi)) in sched.split_whitespace().zip(CRON_RANGES.iter()) {
            if !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()) {
                let v: u32 = field.parse().map_err(|_| {
                    err(format!(
                        "cron {fname} field {field:?} is not a valid integer"
                    ))
                })?;
                if v < *lo || v > *hi {
                    return Err(err(format!(
                        "cron {fname} field {v} is out of range {lo}..={hi}"
                    )));
                }
            }
        }
        let tz = raw.timezone.unwrap_or_else(|| "UTC".to_string());
        (Some(sched.to_string()), tz)
    } else {
        (None, String::new())
    };

    // retry validation.
    let retry = match raw.retry {
        None => None,
        Some(r) => {
            if r.max_attempts < 1 {
                return Err(err("retry.max_attempts must be >= 1".into()));
            }
            match r.backoff.strategy.as_str() {
                "fixed" | "exponential" => {}
                other => {
                    return Err(err(format!(
                        "retry.backoff.strategy {other:?} must be fixed or exponential"
                    )));
                }
            }
            // Durations are host-honoured; validate they parse so a typo
            // is caught at build time (Rust-class build-time enforcement).
            humantime::parse_duration(&r.backoff.base)
                .map_err(|e| err(format!("retry.backoff.base is not a valid duration: {e}")))?;
            humantime::parse_duration(&r.backoff.max)
                .map_err(|e| err(format!("retry.backoff.max is not a valid duration: {e}")))?;
            Some(RetryEntry {
                max_attempts: r.max_attempts,
                backoff_strategy: r.backoff.strategy,
                backoff_base: r.backoff.base,
                backoff_max: r.backoff.max,
                dead_letter: r.dead_letter,
            })
        }
    };

    Ok(TriggerEntry {
        name,
        kind: raw.kind,
        agent: raw.agent,
        schedule,
        timezone,
        retry,
    })
}

fn validate_pipeline(raw: &UncheckedPipeline) -> Result<PipelineConfig, ProjectConfigError> {
    if raw.steps.is_empty() {
        return Err(ProjectConfigError::EmptyPipeline);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut steps = Vec::with_capacity(raw.steps.len());
    for s in &raw.steps {
        if !seen.insert(s.id.clone()) {
            return Err(ProjectConfigError::PipelineValidation {
                id: s.id.clone(),
                message: format!("step id {:?} declared more than once", s.id),
            });
        }
        let run = match s.run.split_once(':') {
            Some(("agent", id)) => PipelineRunRef::Agent(id.to_string()),
            Some(("tool", id)) => PipelineRunRef::Tool(id.to_string()),
            Some(("deterministic", id)) => PipelineRunRef::Deterministic(id.to_string()),
            Some(("check", id)) => PipelineRunRef::Check(id.to_string()),
            _ => {
                return Err(ProjectConfigError::PipelineValidation {
                    id: s.id.clone(),
                    message: format!(
                    "run must be \"agent:<id>\" | \"tool:<id>\" | \"deterministic:<id>\" | \"check:<id>\", got {:?}",
                    s.run
                ),
                })
            }
        };
        steps.push(PipelineStepConfig {
            id: s.id.clone(),
            run,
            input: s.input.clone().unwrap_or_else(|| "${input}".to_string()),
        });
    }
    Ok(PipelineConfig { steps })
}

/// Parse a locus string into a [`LocusConfig`].
///
/// A value of the form `steps.<id>.output` resolves to `Output("<id>")`;
/// everything else resolves to `Path(s)`.
pub fn parse_locus(s: &str) -> LocusConfig {
    // Match "steps.<id>.output"
    if let Some(rest) = s.strip_prefix("steps.") {
        if let Some(id) = rest.strip_suffix(".output") {
            if !id.is_empty() {
                return LocusConfig::Output(id.to_string());
            }
        }
    }
    LocusConfig::Path(s.to_string())
}

fn validate_goal(id: String, raw: UncheckedGoal) -> Result<GoalEntry, ProjectConfigError> {
    let evaluates = parse_locus(&raw.evaluates);

    // fn and check are mutually exclusive
    match (&raw.r#fn, &raw.check) {
        (Some(_), Some(_)) => {
            return Err(ProjectConfigError::GoalValidation {
                id,
                message: "only one of `fn` or `check` may be set".into(),
            });
        }
        (None, None) => {
            return Err(ProjectConfigError::GoalValidation {
                id,
                message: "one of `fn` or `check` must be set".into(),
            });
        }
        (Some(fn_name), None) => {
            return Ok(GoalEntry {
                id,
                evaluates,
                predicate: GoalPredicateConfig::NativeFn(fn_name.clone()),
            });
        }
        (None, Some(_)) => {} // fall through to check dispatch below
    }

    let check = raw.check.as_deref().unwrap();
    let predicate = match check {
        "exists" => GoalPredicateConfig::Exists,
        "non_empty" => GoalPredicateConfig::NonEmpty,
        "equals" => match raw.equals {
            Some(v) => GoalPredicateConfig::Equals(v),
            None => {
                return Err(ProjectConfigError::GoalValidation {
                    id,
                    message: "check = \"equals\" requires the `equals` field".into(),
                });
            }
        },
        "matches" => match raw.pattern {
            Some(p) => {
                // Validate the regex compiles at build time.
                if let Err(e) = regex::Regex::new(&p) {
                    return Err(ProjectConfigError::BadGoalRegex {
                        id,
                        message: e.to_string(),
                    });
                }
                GoalPredicateConfig::Matches(p)
            }
            None => {
                return Err(ProjectConfigError::GoalValidation {
                    id,
                    message: "check = \"matches\" requires the `pattern` field".into(),
                });
            }
        },
        "min_count" => match raw.min_count {
            Some(n) => GoalPredicateConfig::MinCount(n),
            None => {
                return Err(ProjectConfigError::GoalValidation {
                    id,
                    message: "check = \"min_count\" requires the `min_count` field".into(),
                });
            }
        },
        "schema_valid" => match raw.schema {
            Some(s) => GoalPredicateConfig::SchemaValid(s),
            None => {
                return Err(ProjectConfigError::GoalValidation {
                    id,
                    message: "check = \"schema_valid\" requires the `schema` field".into(),
                });
            }
        },
        other => {
            return Err(ProjectConfigError::GoalValidation {
                id,
                message: format!("unknown check {other:?}; valid values: exists, non_empty, equals, matches, min_count, schema_valid"),
            });
        }
    };

    Ok(GoalEntry {
        id,
        evaluates,
        predicate,
    })
}

fn validate_deliverable(
    id: String,
    raw: UncheckedDeliverable,
) -> Result<DeliverableEntry, ProjectConfigError> {
    // Exactly one of path/output must be set.
    let locus = match (raw.path, raw.output) {
        (Some(p), None) => parse_locus(&p),
        (None, Some(o)) => parse_locus(&o),
        (Some(_), Some(_)) => {
            return Err(ProjectConfigError::DeliverableValidation {
                id,
                message: "exactly one of `path` or `output` must be set, not both".into(),
            });
        }
        (None, None) => {
            return Err(ProjectConfigError::DeliverableValidation {
                id,
                message: "one of `path` or `output` must be set".into(),
            });
        }
    };

    // judge and judge_model are mutually exclusive — check BEFORE collapsing.
    if raw.judge.is_some() && raw.judge_model.is_some() {
        return Err(ProjectConfigError::JudgeAndModelConflict { id });
    }

    // on_fail: default "abort", only "abort"/"retry" accepted.
    let on_fail = match raw.on_fail.as_deref() {
        None | Some("abort") => OnFailConfig::Abort,
        Some("retry") => OnFailConfig::Retry,
        Some(other) => {
            return Err(ProjectConfigError::DeliverableValidation {
                id,
                message: format!("on_fail must be \"abort\" or \"retry\", got {other:?}"),
            });
        }
    };

    // max_attempts defaults: 1 for abort, 3 for retry.
    let max_attempts = match raw.max_attempts {
        Some(n) => n,
        None => match on_fail {
            OnFailConfig::Abort => 1,
            OnFailConfig::Retry => 3,
        },
    };

    // max_attempts must be >= 1 (the field doc says so; guard it here).
    if max_attempts == 0 {
        return Err(ProjectConfigError::DeliverableValidation {
            id,
            message: "max_attempts must be >= 1".into(),
        });
    }

    // Collapse judge fields.
    let judge = match raw.judge {
        Some(agent_id) => JudgeConfig::Agent(agent_id),
        None => JudgeConfig::Default {
            model: raw.judge_model,
        },
    };

    Ok(DeliverableEntry {
        id,
        locus,
        must_satisfy: raw.must_satisfy,
        on_fail,
        max_attempts,
        retry_from: raw.retry_from,
        judge,
        producer: String::new(),
        gate: String::new(),
    })
}

/// Run cross-entity postcondition checks on an otherwise-valid [`ProjectConfig`].
///
/// Resolves each deliverable's producer agent (the unique agent whose `produces`
/// list contains a locus matching the deliverable) and checks that the producer
/// holds an `fs.write` capability covering the declared path (for
/// `LocusConfig::Path` loci). Also validates gate-position, retry-span-has-LLM,
/// and unknown-retry-from for RETRY deliverables.
/// Fills `DeliverableEntry::producer` and `DeliverableEntry::gate` as side-effects.
fn validate_postconditions(cfg: &mut ProjectConfig) -> Result<(), ProjectConfigError> {
    use crate::capability_override::glob_subset::is_glob_subset;
    use tau_domain::FsCapability;

    // First pass: resolve producers, run capability checks, and for RETRY
    // deliverables run gate-position + retry-span guarantees (all immutable
    // borrows of agents/tools/pipeline). Collect
    // (deliverable_id, resolved_producer_id, resolved_gate_id) triples.
    let mut resolved: Vec<(String, String, String)> = Vec::new();

    for (deliverable_id, deliverable) in &cfg.deliverables {
        // Collect agent ids whose `produces` contains a locus equal to this
        // deliverable's locus (after running each entry through parse_locus).
        let mut producers: Vec<String> = cfg
            .agents
            .iter()
            .filter(|(_, agent)| {
                agent
                    .produces
                    .iter()
                    .any(|p| parse_locus(p) == deliverable.locus)
            })
            .map(|(id, _)| id.clone())
            .collect();
        producers.sort();

        let producer_id = match producers.len() {
            0 => {
                // Derive a display string for the locus.
                let locus_str = match &deliverable.locus {
                    LocusConfig::Path(p) => p.clone(),
                    LocusConfig::Output(o) => format!("steps.{o}.output"),
                };
                return Err(ProjectConfigError::DeliverableNoProducer {
                    id: deliverable_id.clone(),
                    locus: locus_str,
                });
            }
            1 => producers.remove(0),
            _ => {
                return Err(ProjectConfigError::DeliverableAmbiguousProducer {
                    id: deliverable_id.clone(),
                    agents: producers,
                });
            }
        };

        // Capability check only for path loci.
        if let LocusConfig::Path(path) = &deliverable.locus {
            let agent = cfg.agents.get(&producer_id).expect("producer agent exists");

            // Collect all write paths from tools the producer references.
            let write_paths: Vec<String> = agent
                .tool_refs
                .iter()
                .filter_map(|tool_name| cfg.tools.get(tool_name))
                .flat_map(|tool| {
                    tool.capabilities.iter().filter_map(|cap| {
                        if let tau_domain::Capability::Filesystem(FsCapability::Write {
                            paths,
                            ..
                        }) = cap
                        {
                            Some(paths.clone())
                        } else {
                            None
                        }
                    })
                })
                .flatten()
                .collect();

            let covered = write_paths
                .iter()
                .any(|cap_path| is_glob_subset(path, cap_path));
            if !covered {
                return Err(ProjectConfigError::DeliverableProducerLacksCapability {
                    id: deliverable_id.clone(),
                    agent: producer_id,
                    path: path.clone(),
                });
            }
        }

        // Gate checks: only for RETRY deliverables.
        let gate_id = if deliverable.on_fail == OnFailConfig::Retry {
            // A retry needs a pipeline with the producer in it.
            let steps = match &cfg.pipeline {
                Some(p) => &p.steps,
                None => {
                    // No pipeline → no sequence to rewind.
                    return Err(ProjectConfigError::RetrySpanNoLlm {
                        id: deliverable_id.clone(),
                    });
                }
            };

            // Find the producer step index: the step whose run is
            // PipelineRunRef::Agent(producer_id).
            let producer_step_index = steps
                .iter()
                .position(|s| s.run == PipelineRunRef::Agent(producer_id.clone()));

            let producer_step_index = match producer_step_index {
                Some(i) => i,
                None => {
                    // Producer agent not in the pipeline → no sequence to rewind.
                    return Err(ProjectConfigError::RetrySpanNoLlm {
                        id: deliverable_id.clone(),
                    });
                }
            };

            // Determine the gate step.
            let (gate_step_id, gate_step_index) = match &deliverable.retry_from {
                Some(g) => {
                    // Explicit retry_from: find its index.
                    match steps.iter().position(|s| &s.id == g) {
                        Some(i) => (g.clone(), i),
                        None => {
                            return Err(ProjectConfigError::UnknownRetryFrom {
                                id: deliverable_id.clone(),
                                gate: g.clone(),
                            });
                        }
                    }
                }
                None => {
                    // Default gate = the producer step itself.
                    let producer_step_id = steps[producer_step_index].id.clone();
                    (producer_step_id, producer_step_index)
                }
            };

            // Guarantee 1: gate_index <= producer_index.
            if gate_step_index > producer_step_index {
                return Err(ProjectConfigError::GateAfterProducer {
                    id: deliverable_id.clone(),
                    gate: gate_step_id,
                    producer: producer_id,
                });
            }

            // Guarantee 2: at least one agent step in [gate_index..=producer_index].
            let span_has_llm = steps[gate_step_index..=producer_step_index]
                .iter()
                .any(|s| matches!(s.run, PipelineRunRef::Agent(_)));
            if !span_has_llm {
                return Err(ProjectConfigError::RetrySpanNoLlm {
                    id: deliverable_id.clone(),
                });
            }

            gate_step_id
        } else {
            // ABORT deliverables: gate field stays empty.
            String::new()
        };

        resolved.push((deliverable_id.clone(), producer_id, gate_id));
    }

    // Second pass: fill in the `producer` and `gate` fields on each deliverable.
    for (deliverable_id, producer_id, gate_id) in resolved {
        if let Some(deliverable) = cfg.deliverables.get_mut(&deliverable_id) {
            deliverable.producer = producer_id;
            deliverable.gate = gate_id;
        }
    }

    // Third pass: judge-agent existence check.
    // JudgeConfig::Agent(a) where `a` is not a key in cfg.agents → UnknownJudgeAgent.
    for (deliverable_id, deliverable) in &cfg.deliverables {
        if let JudgeConfig::Agent(judge_id) = &deliverable.judge {
            if !cfg.agents.contains_key(judge_id) {
                return Err(ProjectConfigError::UnknownJudgeAgent {
                    id: deliverable_id.clone(),
                    judge: judge_id.clone(),
                });
            }
        }
    }

    Ok(())
}

/// Return the package name (the part before the first `@`) from a package
/// reference string like `"anthropic@^1"`. Returns the whole string if there
/// is no `@`.
fn package_name_from_ref(pkg_ref: &str) -> &str {
    pkg_ref.split('@').next().unwrap_or(pkg_ref)
}

/// Stage-1 build-time refusals for the `[models]` table and all references
/// to model aliases (D7).
///
/// Runs **after** `validate_postconditions` so postcondition errors take
/// priority; model errors are additive atop a structurally-sound config.
fn validate_models(cfg: &ProjectConfig) -> Result<(), ProjectConfigError> {
    // Collect the set of declared package names.  A package name is the
    // prefix before the first `@`; e.g. `"anthropic@^1"` → `"anthropic"`.
    // We union two sources:
    //   1. Top-level `packages` entries (e.g. `packages = ["anthropic@^1"]`).
    //   2. Each agent's `package` field.
    // This allows a backend to be declared solely in the top-level
    // `[packages]` table without any agent referencing it directly.
    let declared_packages: std::collections::BTreeSet<&str> = cfg
        .packages
        .iter()
        .map(|p| package_name_from_ref(p))
        .chain(
            cfg.agents
                .values()
                .map(|a| package_name_from_ref(&a.package)),
        )
        .collect();

    // 1. Validate every `[models]` entry.
    for (alias, m) in &cfg.models {
        // Defense-in-depth: RawModelEntry's required fields make serde reject a
        // [models] entry missing `backend`/`model` at TOML parse time, so this
        // guard only fires on direct in-Rust construction. Kept intentionally.
        if m.backend.is_empty() || m.model.is_empty() {
            return Err(ProjectConfigError::MalformedModelEntry {
                alias: alias.clone(),
            });
        }
        if !declared_packages.contains(m.backend.as_str()) {
            return Err(ProjectConfigError::ModelBackendNotDeclared {
                alias: alias.clone(),
                backend: m.backend.clone(),
            });
        }
    }

    // 2. Every agent must have a non-empty model alias that resolves in
    //    `[models]`.
    for (id, agent) in &cfg.agents {
        if agent.model.is_empty() {
            return Err(ProjectConfigError::MissingAgentModel { agent: id.clone() });
        }
        if !cfg.models.contains_key(&agent.model) {
            return Err(ProjectConfigError::UnknownModelAlias {
                referrer: format!("agent `{id}`"),
                alias: agent.model.clone(),
            });
        }
    }

    // 3. Every deliverable judge_model override must resolve in `[models]`.
    for (id, d) in &cfg.deliverables {
        if let JudgeConfig::Default { model: Some(alias) } = &d.judge {
            if !cfg.models.contains_key(alias) {
                return Err(ProjectConfigError::UnknownModelAlias {
                    referrer: format!("deliverable `{id}` judge_model"),
                    alias: alias.clone(),
                });
            }
        }
    }

    Ok(())
}

fn unchecked_to_capability_override(
    raw: &UncheckedCapabilityOverride,
) -> crate::capability_override::CapabilityOverride {
    use crate::capability_override::CapabilityOverride;

    // Fold the kind-specific allow_* / deny_* fields into a single
    // `(allow, deny)` pair. The runtime cap_kind() picks the right
    // strings based on the matching package capability.
    let (allow, deny) = match raw.kind.as_str() {
        "fs.read" | "fs.write" | "fs.exec" => (raw.allow_paths.clone(), raw.deny_paths.clone()),
        "net.http" => (raw.allow_hosts.clone(), raw.deny_hosts.clone()),
        "process.spawn" => (raw.allow_commands.clone(), raw.deny_commands.clone()),
        _ => (None, Vec::new()),
    };
    CapabilityOverride::new(raw.kind.clone(), allow, deny, raw.max_bytes)
}

// ----- File entrypoint -----

impl ProjectConfig {
    /// Parse and validate from a TOML string. Convenience wrapper for
    /// tests and in-memory usage (as opposed to [`ProjectConfig::from_path`]
    /// for file-based loading).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_pkg::project::project::ProjectConfig;
    ///
    /// let toml = r#"
    ///     [project]
    ///     name = "demo"
    /// "#;
    /// let config = ProjectConfig::parse_str(toml).expect("valid");
    /// assert_eq!(config.project_name, "demo");
    /// assert!(config.tools.is_empty());
    /// assert!(config.steps.is_empty());
    /// ```
    pub fn parse_str(toml: &str) -> Result<Self, ProjectConfigError> {
        let unchecked: UncheckedProjectConfig =
            toml::from_str(toml).map_err(|source| ProjectConfigError::ParseStr { source })?;
        unchecked.validate()
    }

    /// Load and validate from a path. Convenience wrapper around the
    /// deserialize-then-validate pipeline.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ProjectConfigError> {
        let path = path.as_ref();
        let bytes = std::fs::read_to_string(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProjectConfigError::NotFound
            } else {
                ProjectConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        let unchecked: UncheckedProjectConfig =
            toml::from_str(&bytes).map_err(|source| ProjectConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        unchecked.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> Result<ProjectConfig, ProjectConfigError> {
        let unchecked: UncheckedProjectConfig = toml::from_str(toml_str).unwrap();
        unchecked.validate()
    }

    #[test]
    fn model_entry_holds_backend_and_model() {
        let m = ModelEntry {
            backend: "anthropic".into(),
            model: "claude-haiku-4-5".into(),
        };
        assert_eq!(m.backend, "anthropic");
        assert_eq!(m.model, "claude-haiku-4-5");
    }

    #[test]
    fn models_table_parses() {
        let toml = r#"
            [project]
            name = "p"
            [models]
            haiku = { backend = "anthropic", model = "claude-haiku-4-5" }
            [agents.bot]
            display_name = "Bot"
            package = "anthropic@^1"
            model = "haiku"
        "#;
        let cfg = parse(toml).unwrap();
        assert_eq!(cfg.models["haiku"].backend, "anthropic");
        assert_eq!(cfg.models["haiku"].model, "claude-haiku-4-5");
    }

    #[test]
    fn parse_minimal_project_only_succeeds() {
        let cfg = parse("[project]\nname = \"x\"\n").unwrap();
        assert_eq!(cfg.project_name, "x");
        assert!(cfg.agents.is_empty());
    }

    #[test]
    fn parse_with_one_full_agent_succeeds() {
        let toml_str = r#"
            [project]
            name = "demo"

            [models]
            default = { backend = "code-reviewer", model = "model-v1" }

            [agents.reviewer]
            display_name = "Code Reviewer"
            package      = "code-reviewer@^0.1"
            model        = "default"

            [[agents.reviewer.requires.tools]]
            name = "fs-read"
            source = "https://example.com/fs-read.git"

            [agents.reviewer.config]
            model = "claude"

            [agents.reviewer.prompt]
            system = "You are a careful reviewer."
        "#;
        let cfg = parse(toml_str).unwrap();
        assert_eq!(cfg.agents.len(), 1);
        let agent = cfg.agents.get("reviewer").unwrap();
        assert_eq!(agent.display_name, "Code Reviewer");
        assert_eq!(agent.requires.tools.len(), 1);
        assert_eq!(agent.requires.tools[0].name.as_str(), "fs-read");
        assert!(
            matches!(&agent.prompt, PromptEntry::Inline(s) if s == "You are a careful reviewer.")
        );
    }

    #[test]
    fn validate_rejects_empty_project_name() {
        let result = parse("[project]\nname = \"\"\n");
        assert!(matches!(result, Err(ProjectConfigError::EmptyProjectName)));
    }

    #[test]
    fn validate_accepts_capability_override() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

            [[agents.r.capabilities]]
            kind        = "fs.read"
            allow_paths = ["${PROJECT}/src/**"]
            deny_paths  = ["${PROJECT}/.env"]
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert_eq!(agent.capability_overrides.len(), 1);
        let ov = &agent.capability_overrides[0];
        assert_eq!(ov.kind, "fs.read");
        assert_eq!(
            ov.allow.as_deref().unwrap(),
            &["${PROJECT}/src/**".to_string()]
        );
        assert_eq!(ov.deny, vec!["${PROJECT}/.env".to_string()]);
    }

    #[test]
    fn validate_rejects_duplicate_kind_in_override() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"


            [[agents.r.capabilities]]
            kind        = "fs.read"
            allow_paths = ["${PROJECT}/src/**"]

            [[agents.r.capabilities]]
            kind        = "fs.read"
            allow_paths = ["${PROJECT}/docs/**"]
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::CapabilityOverrideExpands { id, kind, reason }) = result else {
            panic!("expected CapabilityOverrideExpands: {result:?}")
        };
        assert_eq!(id, "r");
        assert_eq!(kind, "fs.read");
        assert!(reason.contains("duplicate"));
    }

    #[test]
    fn validate_rejects_unknown_capability_override_field() {
        // Defends against silent discard of spec-defined-but-not-yet-
        // implemented fields like net.http's allow_methods. The
        // #[serde(deny_unknown_fields)] turns any unknown key into a
        // clear TOML parse error.
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"


            [[agents.r.capabilities]]
            kind          = "net.http"
            allow_hosts   = ["api.example.com"]
            allow_methods = ["GET"]
        "#;
        // Use the lower-level toml::from_str rather than parse() so we
        // can observe the deserialization error directly. parse() would
        // wrap it in toml::de::Error.
        let result: Result<UncheckedProjectConfig, _> = toml::from_str(toml_str);
        let Err(err) = result else {
            panic!("expected unknown-field error: {result:?}")
        };
        let msg = err.to_string();
        assert!(
            msg.contains("allow_methods") || msg.contains("unknown field"),
            "expected error mentioning allow_methods or unknown field; got: {msg}"
        );
    }

    #[test]
    fn validate_no_capability_block_keeps_overrides_empty() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

        "#;
        let cfg = parse(toml_str).unwrap();
        assert!(cfg.agents.get("r").unwrap().capability_overrides.is_empty());
    }

    #[test]
    fn validate_rejects_prompt_with_both_system_and_system_file() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"


            [agents.r.prompt]
            system      = "inline"
            system_file = "prompts/r.md"
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::PromptAmbiguous { id, .. }) = result else {
            panic!("expected PromptAmbiguous: {result:?}")
        };
        assert_eq!(id, "r");
    }

    #[test]
    fn validate_accepts_prompt_with_only_system() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

            [agents.r.prompt]
            system = "be helpful"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert!(matches!(&agent.prompt, PromptEntry::Inline(s) if s == "be helpful"));
    }

    #[test]
    fn parses_agent_context_pipeline() {
        let toml_str = r#"
            [project]
            name = "p"

            [models]
            default = { backend = "demo", model = "model-v1" }

            [agents.a]
            display_name = "A"
            package = "demo@^0.1"
            model   = "default"

            [[agents.a.context.pipeline]]
            transformer = "trim_old"

            [agents.a.context.steps.trim_old]
            keep_last_turns = 4

            [[agents.a.context.pipeline]]
            transformer = "fit_budget"

            [agents.a.context.steps.fit_budget]
            max_tokens = 4000
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("a").unwrap();
        assert_eq!(agent.context.len(), 2);
        assert_eq!(agent.context[0].transformer, "trim_old");
        assert_eq!(agent.context[0].determinism, "pure");
        assert!(agent.context[0].custom.is_none());
        assert_eq!(
            agent.context[0]
                .config
                .get("keep_last_turns")
                .and_then(|v| v.as_u64()),
            Some(4)
        );
        assert_eq!(agent.context[1].transformer, "fit_budget");
        assert_eq!(
            agent.context[1]
                .config
                .get("max_tokens")
                .and_then(|v| v.as_u64()),
            Some(4000)
        );
    }

    #[test]
    fn rejects_custom_context_node_missing_source() {
        let toml_str = r#"
            [project]
            name = "p"

            [agents.a]
            display_name = "A"
            package = "demo@^0.1"


            [[agents.a.context.pipeline]]
            transformer = "my_custom"
            kind = "custom"
            package = "pkg@^0.1"
        "#;
        let err = parse(toml_str).expect_err("missing source should reject");
        assert!(matches!(err, ProjectConfigError::AgentValidation { .. }));
    }

    #[test]
    fn rejects_custom_context_node_non_native_source() {
        let toml_str = r#"
            [project]
            name = "p"

            [agents.a]
            display_name = "A"
            package = "demo@^0.1"


            [[agents.a.context.pipeline]]
            transformer = "my_custom"
            kind = "custom"
            source = "wasm"
            package = "pkg@^0.1"
        "#;
        let err = parse(toml_str).expect_err("non-native source should reject in v1");
        assert!(matches!(err, ProjectConfigError::AgentValidation { .. }));
    }

    #[test]
    fn validate_accepts_prompt_with_only_system_file() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

            [agents.r.prompt]
            system_file = "prompts/r.md"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        let PromptEntry::File(p) = &agent.prompt else {
            panic!("expected File: {:?}", agent.prompt)
        };
        assert_eq!(p.to_str(), Some("prompts/r.md"));
    }

    #[test]
    fn validate_accepts_no_prompt_table() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert!(matches!(&agent.prompt, PromptEntry::None));
    }

    #[test]
    fn validate_rejects_empty_display_name() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = ""
            package      = "p@^0.1"

        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::AgentValidation { id, message }) = result else {
            panic!("expected AgentValidation: {result:?}")
        };
        assert_eq!(id, "r");
        assert!(message.contains("display_name"));
    }

    #[test]
    fn validate_rejects_empty_package() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = ""

        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::AgentValidation { message, .. }) = result else {
            panic!()
        };
        assert!(message.contains("package"));
    }

    #[test]
    fn validate_rejects_bare_string_tools_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"


            [agents.r.requires]
            tools = ["fs-read"]
        "#;
        // serde rejects the bare string — toml::de error surfaces as Parse.
        let result: Result<UncheckedProjectConfig, _> = toml::from_str(toml_str);
        assert!(
            result.is_err(),
            "bare-string tools entry must fail to deserialize"
        );
    }

    #[test]
    fn validate_accepts_struct_tools_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

            [[agents.r.requires.tools]]
            name = "fs-read"
            source = "https://example.com/fs-read.git"
            version = "^0.1"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert_eq!(agent.requires.tools.len(), 1);
        assert_eq!(agent.requires.tools[0].name.as_str(), "fs-read");
        assert_eq!(agent.requires.tools[0].version_req.to_string(), "^0.1");
    }

    #[test]
    fn validate_default_version_is_star() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

            [[agents.r.requires.tools]]
            name = "fs-read"
            source = "https://example.com/fs-read.git"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert_eq!(agent.requires.tools[0].version_req.to_string(), "*");
    }

    #[test]
    fn parse_with_two_agents_keeps_both() {
        let toml_str = r#"
            [project]
            name = "demo"

            [models]
            default = { backend = "p", model = "model-v1" }
            beta-m  = { backend = "q", model = "model-v1" }

            [agents.alpha]
            display_name = "Alpha"
            package      = "p@^0.1"
            model        = "default"

            [agents.beta]
            display_name = "Beta"
            package      = "q@^0.1"
            model        = "beta-m"

        "#;
        let cfg = parse(toml_str).unwrap();
        assert_eq!(cfg.agents.len(), 2);
        assert!(cfg.agents.contains_key("alpha"));
        assert!(cfg.agents.contains_key("beta"));
    }

    // --- IR lowering tests (β.2.2) ---

    #[test]
    fn parse_str_accepts_minimal_project() {
        let cfg = ProjectConfig::parse_str("[project]\nname = \"demo\"\n").unwrap();
        assert_eq!(cfg.project_name, "demo");
        assert!(cfg.tools.is_empty());
        assert!(cfg.steps.is_empty());
    }

    #[test]
    fn parse_str_error_on_invalid_toml() {
        let result = ProjectConfig::parse_str("not valid toml {{{{");
        assert!(
            matches!(result, Err(ProjectConfigError::ParseStr { .. })),
            "expected ParseStr error"
        );
    }

    #[test]
    fn parse_native_tool_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.read_temp]
            native = "ReadTemp"
            description = "reads the temperature"
            capabilities = []
        "#;
        let cfg = parse(toml_str).unwrap();
        assert_eq!(cfg.tools.len(), 1);
        let tool = cfg.tools.get("read_temp").unwrap();
        assert_eq!(tool.name, "read_temp");
        assert!(matches!(&tool.body, ToolBody::Native(s) if s == "ReadTemp"));
        assert_eq!(tool.description, "reads the temperature");
        assert!(tool.capabilities.is_empty());
    }

    #[test]
    fn parse_mcp_tool_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.weather]
            mcp = "https://mcp.weather.example.com"
            capabilities = []
        "#;
        let cfg = parse(toml_str).unwrap();
        let tool = cfg.tools.get("weather").unwrap();
        assert!(
            matches!(&tool.body, ToolBody::Mcp(url) if url == "https://mcp.weather.example.com")
        );
    }

    #[test]
    fn parse_subflow_tool_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.alert]
            subflow = "alerter"
            capabilities = []
        "#;
        let cfg = parse(toml_str).unwrap();
        let tool = cfg.tools.get("alert").unwrap();
        assert!(matches!(&tool.body, ToolBody::Subflow(t) if t == "alerter"));
    }

    #[test]
    fn parse_step_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [steps.normalize]
            deterministic = "parse_celsius"
        "#;
        let cfg = parse(toml_str).unwrap();
        assert_eq!(cfg.steps.len(), 1);
        let step = cfg.steps.get("normalize").unwrap();
        assert_eq!(step.name, "normalize");
        assert_eq!(step.fn_name, "parse_celsius");
    }

    #[test]
    fn validate_rejects_empty_step_fn_name() {
        let toml_str = r#"
            [project]
            name = "x"

            [steps.bad]
            deterministic = ""
        "#;
        let result = parse(toml_str);
        assert!(
            matches!(result, Err(ProjectConfigError::StepValidation { .. })),
            "expected StepValidation error, got: {result:?}"
        );
    }

    #[test]
    fn validate_rejects_empty_native_fn_name() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.bad]
            native = ""
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::ToolValidation { name, message }) = result else {
            panic!("expected ToolValidation error, got: {result:?}")
        };
        assert_eq!(name, "bad");
        assert!(message.contains("fn name"), "unexpected message: {message}");
    }

    #[test]
    fn validate_rejects_empty_mcp_url() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.bad]
            mcp = ""
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::ToolValidation { name, message }) = result else {
            panic!("expected ToolValidation error, got: {result:?}")
        };
        assert_eq!(name, "bad");
        assert!(message.contains("url"), "unexpected message: {message}");
    }

    #[test]
    fn validate_rejects_empty_subflow_target() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.bad]
            subflow = ""
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::ToolValidation { name, message }) = result else {
            panic!("expected ToolValidation error, got: {result:?}")
        };
        assert_eq!(name, "bad");
        assert!(
            message.contains("target agent id"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn parse_agent_ir_fields() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            claude-haiku-4-5 = { backend = "p", model = "claude-haiku-4-5" }

            [agents.monitor]
            display_name = "Monitor"
            package      = "p@^0.1"

            model        = "claude-haiku-4-5"
            tool_refs    = ["read_temp", "set_fan"]
            max_turns    = 10
            max_tokens   = 4096
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("monitor").unwrap();
        assert_eq!(agent.model, "claude-haiku-4-5");
        assert_eq!(agent.tool_refs, vec!["read_temp", "set_fan"]);
        assert_eq!(agent.max_turns, Some(10));
        assert_eq!(agent.max_tokens, Some(4096));
    }

    #[test]
    fn parse_agent_ir_fields_default_empty() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            model        = "default"

        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert_eq!(agent.model, "default");
        assert!(agent.tool_refs.is_empty());
        assert!(agent.max_turns.is_none());
        assert!(agent.max_tokens.is_none());
    }

    #[test]
    fn parse_tool_with_net_http_capability() {
        // Capability uses the struct form { kind = "net.http" }.
        // In TOML, inline arrays-of-tables use the array-of-inline-tables
        // syntax when embedded inline in a regular table.
        let toml_str = r#"
            [project]
            name = "x"

            [tools.weather]
            mcp = "https://mcp.weather.example.com"
            capabilities = [{ kind = "net.http" }]
        "#;
        let cfg = parse(toml_str).unwrap();
        let tool = cfg.tools.get("weather").unwrap();
        assert_eq!(tool.capabilities.len(), 1);
    }

    #[test]
    fn unchecked_tool_parses_sampling_and_roots() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.weather]
            mcp = "https://mcp.example.com"
            description = "weather"
            capabilities = []
            sampling = { models = ["claude-haiku-4-5"] }
            roots = ["/tmp/mcp"]
        "#;
        let cfg = parse(toml_str).unwrap();
        let tool = cfg.tools.get("weather").unwrap();
        let sampling = tool.sampling.as_ref().expect("sampling present");
        assert_eq!(sampling.models.len(), 1);
        assert_eq!(sampling.models[0], "claude-haiku-4-5");
        assert_eq!(tool.roots.len(), 1);
    }

    #[test]
    fn mcp_url_with_unsupported_scheme_rejected() {
        let toml_str = r#"
            [project]
            name = "x"

            [tools.bad]
            mcp = "ws://example.com"
        "#;
        let err = ProjectConfig::parse_str(toml_str).expect_err("should reject");
        assert!(matches!(err, ProjectConfigError::UnsupportedMcpUrl { .. }));
    }

    #[test]
    fn mcp_url_with_stdio_or_https_accepted() {
        for url in [
            "stdio:weather-server",
            "https://mcp.example.com",
            "http://localhost:8080",
        ] {
            let toml_str = format!(
                r#"
                [project]
                name = "x"

                [tools.test]
                mcp = "{url}"
            "#
            );
            ProjectConfig::parse_str(&toml_str)
                .unwrap_or_else(|e| panic!("URL {url:?} should be accepted but got: {e}"));
        }
    }

    #[test]
    fn cassette_url_validates() {
        let toml = r#"
            [project]
            name = "x"

            [tools.weather]
            mcp = "cassette:./fixtures/weather.jsonl"
        "#;
        let project = ProjectConfig::parse_str(toml).expect("cassette: URL should validate");
        let tool = project.tools.get("weather").expect("tool present");
        match &tool.body {
            ToolBody::Mcp(url) => assert_eq!(url, "cassette:./fixtures/weather.jsonl"),
            other => panic!("expected Mcp body, got {other:?}"),
        }
    }

    // --- Trigger slice-1 tests ---

    #[test]
    fn parse_cron_trigger_with_retry() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.summarizer]
            display_name = "Summarizer"
            package      = "p@^0.1"
            model        = "default"

            [trigger.nightly]
            kind     = "cron"
            agent    = "summarizer"
            schedule = "0 3 * * *"

            [trigger.nightly.retry]
            max_attempts = 3
            backoff      = { strategy = "exponential", base = "30s", max = "10m" }
            dead_letter  = "dlq-sink"
        "#;
        let cfg = parse(toml_str).unwrap();
        let t = cfg.triggers.get("nightly").expect("trigger present");
        assert_eq!(t.kind, "cron");
        assert_eq!(t.agent, "summarizer");
        assert_eq!(t.schedule.as_deref(), Some("0 3 * * *"));
        assert_eq!(t.timezone, "UTC"); // defaulted
        let r = t.retry.as_ref().expect("retry present");
        assert_eq!(r.max_attempts, 3);
        assert_eq!(r.backoff_strategy, "exponential");
        assert_eq!(r.dead_letter.as_deref(), Some("dlq-sink"));
    }

    #[test]
    fn parse_manual_trigger() {
        let toml_str = r#"
            [project]
            name = "x"

            [models]
            default = { backend = "p", model = "model-v1" }

            [agents.summarizer]
            display_name = "S"
            package      = "p@^0.1"
            model        = "default"

            [trigger.manual]
            kind  = "manual"
            agent = "summarizer"
        "#;
        let cfg = parse(toml_str).unwrap();
        let t = cfg.triggers.get("manual").unwrap();
        assert_eq!(t.kind, "manual");
        assert!(t.schedule.is_none());
        assert!(t.retry.is_none());
    }

    #[test]
    fn validate_rejects_cron_without_schedule() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
        "#;
        let Err(ProjectConfigError::TriggerValidation { name, message }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert_eq!(name, "t");
        assert!(message.contains("schedule"), "got: {message}");
    }

    #[test]
    fn validate_rejects_manual_with_schedule() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "manual"
            agent = "a"
            schedule = "0 3 * * *"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("manual"), "got: {message}");
    }

    #[test]
    fn validate_rejects_unsupported_kind() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "webhook"
            agent = "a"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(
            message.contains("webhook") || message.contains("not supported"),
            "got: {message}"
        );
    }

    #[test]
    fn validate_rejects_bad_cron_field_count() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * *"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("5"), "got: {message}");
    }

    #[test]
    fn validate_rejects_bad_backoff_duration() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * * *"
            [trigger.t.retry]
            max_attempts = 2
            backoff = { strategy = "fixed", base = "not-a-duration", max = "10m" }
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(
            message.contains("duration") || message.contains("base"),
            "got: {message}"
        );
    }

    #[test]
    fn no_trigger_table_keeps_triggers_empty() {
        let cfg = parse("[project]\nname = \"x\"\n").unwrap();
        assert!(cfg.triggers.is_empty());
    }

    #[test]
    fn validate_rejects_max_attempts_zero() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * * *"
            [trigger.t.retry]
            max_attempts = 0
            backoff = { strategy = "fixed", base = "30s", max = "10m" }
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("max_attempts"), "got: {message}");
    }

    #[test]
    fn validate_rejects_unknown_kind_non_webhook() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "event"
            agent = "a"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(message.contains("unknown kind"), "got: {message}");
    }

    #[test]
    fn validate_rejects_bad_backoff_max_duration() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 3 * * *"
            [trigger.t.retry]
            max_attempts = 2
            backoff = { strategy = "fixed", base = "30s", max = "not-a-duration" }
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(
            message.contains("max") || message.contains("duration"),
            "got: {message}"
        );
    }

    #[test]
    fn validate_rejects_out_of_range_cron_hour() {
        let toml_str = r#"
            [project]
            name = "x"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"

            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "0 25 * * *"
        "#;
        let Err(ProjectConfigError::TriggerValidation { message, .. }) = parse(toml_str) else {
            panic!("expected TriggerValidation");
        };
        assert!(
            message.contains("hour") && message.contains("range"),
            "got: {message}"
        );
    }

    #[test]
    fn validate_accepts_star_range_step_list_cron_fields() {
        // `*`, ranges, lists, and steps pass through — the host validates the
        // full cron grammar; tau only range-checks bare integers.
        let toml_str = r#"
            [project]
            name = "x"
            [models]
            default = { backend = "p", model = "model-v1" }
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "default"
            [trigger.t]
            kind = "cron"
            agent = "a"
            schedule = "*/5 1-3 * * 1,3,5"
        "#;
        parse(toml_str).expect("non-integer cron fields must pass through");
    }

    #[test]
    fn parses_pipeline_steps() {
        let toml = r#"
            [project]
            name = "demo"
            [[pipeline.steps]]
            id = "gather"
            run = "agent:gather"
            input = "${input}"
            [[pipeline.steps]]
            id = "writer"
            run = "agent:writer"
            input = "${steps.gather.output}"
        "#;
        let cfg = ProjectConfig::parse_str(toml).expect("parses");
        let pipe = cfg.pipeline.expect("pipeline present");
        assert_eq!(pipe.steps.len(), 2);
        assert_eq!(pipe.steps[0].id, "gather");
        assert_eq!(pipe.steps[0].run, PipelineRunRef::Agent("gather".into()));
        assert_eq!(pipe.steps[1].input, "${steps.gather.output}");
    }

    #[test]
    fn parses_check_pipeline_step() {
        let toml = r#"
            [project]
            name = "demo"
            [[pipeline.steps]]
            id = "report"
            run = "check:report"
            input = "${input}"
        "#;
        let cfg = ProjectConfig::parse_str(toml).expect("parses");
        let pipe = cfg.pipeline.expect("pipeline present");
        assert_eq!(pipe.steps[0].run, PipelineRunRef::Check("report".into()));
    }

    #[test]
    fn rejects_unknown_run_kind() {
        let toml = r#"
            [project]
            name = "demo"
            [[pipeline.steps]]
            id = "x"
            run = "wizard:x"
        "#;
        assert!(ProjectConfig::parse_str(toml).is_err());
    }

    #[test]
    fn rejects_empty_pipeline() {
        // A `[pipeline]` table with no `[[pipeline.steps]]` entries must
        // fail at build time rather than silently falling through to the
        // single-agent path.
        let toml = r#"
            [project]
            name = "demo"
            [pipeline]
        "#;
        let result = ProjectConfig::parse_str(toml);
        assert!(
            matches!(result, Err(ProjectConfigError::EmptyPipeline)),
            "expected EmptyPipeline, got: {result:?}"
        );
    }

    #[test]
    fn defaults_pipeline_input_to_top_level() {
        let toml = r#"
            [project]
            name = "demo"
            [[pipeline.steps]]
            id = "x"
            run = "agent:x"
        "#;
        let cfg = ProjectConfig::parse_str(toml).unwrap();
        assert_eq!(cfg.pipeline.unwrap().steps[0].input, "${input}");
    }

    // --- Task 2: [goals.*] tests ---

    #[test]
    fn goal_matches_parses_path_locus_and_regex_predicate() {
        let toml = r#"
[project]
name = "p"
[goals.has_sources]
evaluates = "/workspace/report.md"
check     = "matches"
pattern   = "(?m)^## Sources"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        let v = cfg.validate().unwrap();
        let g = &v.goals["has_sources"];
        assert_eq!(
            g.evaluates,
            LocusConfig::Path("/workspace/report.md".into())
        );
        assert_eq!(
            g.predicate,
            GoalPredicateConfig::Matches("(?m)^## Sources".into())
        );
    }

    #[test]
    fn goal_fn_escape_hatch_parses_output_locus() {
        let toml = r#"
[project]
name = "p"
[goals.link_health]
evaluates = "steps.writer.output"
fn        = "research_checks::all_links_resolve"
"#;
        let v: ProjectConfig = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap();
        let g = &v.goals["link_health"];
        assert_eq!(g.evaluates, LocusConfig::Output("writer".into()));
        assert_eq!(
            g.predicate,
            GoalPredicateConfig::NativeFn("research_checks::all_links_resolve".into())
        );
    }

    #[test]
    fn goal_matches_without_pattern_is_rejected() {
        let toml = r#"
[project]
name = "p"
[goals.bad]
evaluates = "/x"
check     = "matches"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, ProjectConfigError::GoalValidation { .. }));
    }

    // --- Task 3: [deliverables.*] tests ---

    #[test]
    fn deliverable_path_locus_retry_parses() {
        let toml = r#"
[project]
name = "p"

[models]
default = { backend = "d", model = "model-v1" }

[agents.writer]
display_name = "W"
package = "d@^0.1"

model = "default"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[[pipeline.steps]]
id = "writer"
run = "agent:writer"
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "A coherent summary."
on_fail      = "retry"
max_attempts = 3
retry_from   = "writer"
"#;
        let v = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap();
        let d = &v.deliverables["report"];
        assert_eq!(d.locus, LocusConfig::Path("/workspace/report.md".into()));
        assert_eq!(d.on_fail, OnFailConfig::Retry);
        assert_eq!(d.max_attempts, 3);
        assert_eq!(d.retry_from.as_deref(), Some("writer"));
        assert_eq!(d.judge, JudgeConfig::Default { model: None });
    }

    #[test]
    fn deliverable_rejects_both_path_and_output() {
        let toml = r#"
[project]
name = "p"
[deliverables.bad]
path         = "/x"
output       = "steps.writer.output"
must_satisfy = "x"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectConfigError::DeliverableValidation { .. }
        ));
    }

    #[test]
    fn deliverable_judge_and_model_conflict_rejected_early() {
        let toml = r#"
[project]
name = "p"
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
judge        = "critic"
judge_model  = "claude-haiku-4-5"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, ProjectConfigError::JudgeAndModelConflict { id } if id == "report"));
    }

    #[test]
    fn agent_produces_parses_and_validates() {
        let toml = r#"
[project]
name = "p"

[models]
haiku = { backend = "demo", model = "claude-haiku-4-5" }

[agents.writer]
display_name = "Writer"
package      = "demo@^0.1"

model        = "haiku"
produces     = ["/workspace/report.md"]
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).expect("parse");
        let validated = cfg.validate().expect("validate");
        assert_eq!(
            validated.agents["writer"].produces,
            vec!["/workspace/report.md".to_string()]
        );
    }

    // --- Task 4: producer binding + capability coverage ---

    #[test]
    fn deliverable_without_producer_is_rejected() {
        let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"

model = "m"
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::DeliverableNoProducer { id, .. } if id == "report")
        );
    }

    #[test]
    fn producer_lacking_fs_write_capability_is_rejected() {
        let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"

model = "m"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/other/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectConfigError::DeliverableProducerLacksCapability { .. }
        ));
    }

    #[test]
    fn producer_with_covering_capability_validates() {
        let toml = r#"
[project]
name = "p"
[models]
default = { backend = "d", model = "model-v1" }
[agents.writer]
display_name = "W"
package = "d@^0.1"
model = "default"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
        assert!(toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .is_ok());
    }

    #[test]
    fn producer_field_is_filled_after_successful_validate() {
        let toml = r#"
[project]
name = "p"
[models]
default = { backend = "d", model = "model-v1" }
[agents.writer]
display_name = "W"
package = "d@^0.1"
model = "default"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
"#;
        let cfg = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(cfg.deliverables["report"].producer, "writer");
    }

    // --- Task 5: gate position + retry-span + unknown retry_from ---

    fn cfg_with_pipeline(retry_from: &str, polish_after: bool) -> String {
        // gather -> writer (producer) -> [polish], deliverable retries from `retry_from`
        let polish_step = if polish_after {
            "[[pipeline.steps]]\nid=\"polish\"\nrun=\"agent:polish\"\ninput=\"${steps.writer.output}\"\n"
        } else {
            ""
        };
        let polish_agent = if polish_after {
            "[agents.polish]\ndisplay_name=\"P\"\npackage=\"d@^0.1\"\nmodel=\"default\"\n"
        } else {
            ""
        };
        format!(
            r#"
[project]
name = "p"

[models]
default = {{ backend = "d", model = "model-v1" }}

[agents.gather]
display_name="G"
package="d@^0.1"

model="default"
[agents.writer]
display_name="W"
package="d@^0.1"

model="default"
produces=["/workspace/report.md"]
tool_refs=["write_file"]
{polish_agent}[tools.write_file]
native="WriteFile"
capabilities=[{{ kind="fs.write", paths=["/workspace/**"] }}]
[[pipeline.steps]]
id="gather"
run="agent:gather"
input="${{input}}"
[[pipeline.steps]]
id="writer"
run="agent:writer"
input="${{steps.gather.output}}"
{polish_step}[deliverables.report]
path="/workspace/report.md"
must_satisfy="x"
on_fail="retry"
max_attempts=3
retry_from="{retry_from}"
"#
        )
    }

    #[test]
    fn retry_gate_before_producer_validates() {
        assert!(
            toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("gather", false))
                .unwrap()
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn retry_gate_after_producer_is_rejected() {
        let err = toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("polish", true))
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, ProjectConfigError::GateAfterProducer { .. }));
    }

    #[test]
    fn retry_from_unknown_step_is_rejected() {
        let err = toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("nope", false))
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, ProjectConfigError::UnknownRetryFrom { .. }));
    }

    #[test]
    fn max_attempts_zero_is_rejected() {
        let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"

model = "m"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
max_attempts = 0
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectConfigError::DeliverableValidation { id, .. } if id == "report"
        ));
    }

    #[test]
    fn retry_span_no_llm_is_rejected() {
        // A pipeline with only deterministic steps between gate and producer step.
        // We simulate this by having the "writer" step (producer) be the same
        // step as the gate step via retry_from pointing to it — but then
        // using a pipeline with only a tool step in-between.
        // Actually, the scenario: the gate IS the producer step, and there's
        // no agent step in between — but a single agent step (writer itself)
        // IS the span. So RetrySpanNoLlm fires when the producer has no
        // pipeline step (on_fail=retry but no pipeline present).
        let toml = r#"
[project]
name = "p"
[agents.writer]
display_name = "W"
package = "d@^0.1"

model = "m"
produces  = ["/workspace/report.md"]
tool_refs = ["write_file"]
[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "x"
on_fail      = "retry"
max_attempts = 3
"#;
        // No pipeline → no sequence to rewind → RetrySpanNoLlm
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(
            err,
            ProjectConfigError::RetrySpanNoLlm { id } if id == "report"
        ));
    }

    #[test]
    fn gate_field_is_filled_after_successful_validate() {
        let cfg = toml::from_str::<UncheckedProjectConfig>(&cfg_with_pipeline("gather", false))
            .unwrap()
            .validate()
            .unwrap();
        // gate defaults to retry_from value ("gather") for a RETRY deliverable
        assert_eq!(cfg.deliverables["report"].gate, "gather");
    }

    // --- Task 6: regex compiles + judge resolution ---

    #[test]
    fn goal_bad_regex_is_rejected() {
        let toml = r#"
[project]
name = "p"
[goals.g]
evaluates = "/x"
check     = "matches"
pattern   = "("
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(matches!(err, ProjectConfigError::BadGoalRegex { .. }));
    }

    #[test]
    fn deliverable_unknown_judge_agent_rejected() {
        let toml = r#"
[project]
name = "p"
[agents.writer]
display_name="W"
package="d@^0.1"

model="m"
produces=["/workspace/report.md"]
tool_refs=["write_file"]
[tools.write_file]
native="WriteFile"
capabilities=[{ kind = "fs.write", paths = ["/workspace/**"] }]
[deliverables.report]
path="/workspace/report.md"
must_satisfy="x"
judge="ghost"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::UnknownJudgeAgent { id, judge } if id=="report" && judge=="ghost")
        );
    }

    #[test]
    fn agent_credentials_validate_ok() {
        let toml = r#"
[project]
name = "p"
description = "d"

[models]
default = { backend = "anthropic", model = "claude-haiku-4-5" }

[agents.assistant]
display_name = "A"
package = "anthropic@^1"
model = "default"

[[agents.assistant.credentials]]
id = "anthropic_api_key"
env = "ANTHROPIC_API_KEY"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        let validated = cfg.validate().unwrap();
        let agent = validated.agents.get("assistant").unwrap();
        assert_eq!(agent.credentials.len(), 1);
        assert_eq!(agent.credentials[0].id.as_str(), "anthropic_api_key");
        assert_eq!(agent.credentials[0].env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn agent_credentials_reject_bad_id() {
        let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"

[[agents.a.credentials]]
id = "Bad Id"
env = "X"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ProjectConfigError::CredentialDeclaration { .. }
        ));
    }

    #[test]
    fn agent_credentials_reject_bad_env_name() {
        let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"

[[agents.a.credentials]]
id = "ok_id"
env = "lower_case"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ProjectConfigError::CredentialDeclaration { .. }
        ));
    }

    #[test]
    fn agent_credentials_reject_env_name_with_leading_digit() {
        let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"

[[agents.a.credentials]]
id = "ok_id"
env = "1KEY"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ProjectConfigError::CredentialDeclaration { .. }
        ));
    }

    #[test]
    fn agent_output_schema_parses_and_passes_through() {
        let toml = r#"
packages = ["mock"]

[project]
name = "p"

[models]
default = { backend = "mock", model = "model-v1" }

[agents.judge]
display_name = "Judge"
package = "p@^0.1"
model = "default"
output_schema = { type = "object", required = ["verdict"] }
"#;
        let cfg = ProjectConfig::parse_str(toml).expect("parse");
        let agent = cfg.agents.get("judge").expect("agent present");
        let schema = agent.output_schema.as_ref().expect("output_schema present");
        assert_eq!(schema["type"], serde_json::json!("object"));
        assert_eq!(schema["required"], serde_json::json!(["verdict"]));
    }

    #[test]
    fn agent_without_output_schema_is_none() {
        let toml = r#"
packages = ["mock"]

[project]
name = "p"

[models]
default = { backend = "mock", model = "model-v1" }

[agents.plain]
display_name = "Plain"
package = "p@^0.1"
model = "default"
"#;
        let cfg = ProjectConfig::parse_str(toml).expect("parse");
        assert!(cfg.agents.get("plain").unwrap().output_schema.is_none());
    }

    #[test]
    fn agent_credentials_multiple_distinct_envs_ok() {
        let toml = r#"
[project]
name = "p"
description = "d"

[models]
default = { backend = "x", model = "model-v1" }

[agents.a]
display_name = "A"
package = "x@^1"
model = "default"

[[agents.a.credentials]]
id = "openai_api_key"
env = "OPENAI_API_KEY"
[[agents.a.credentials]]
id = "openai_org"
env = "OPENAI_ORG"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        let validated = cfg.validate().unwrap();
        let agent = validated.agents.get("a").unwrap();
        assert_eq!(agent.credentials.len(), 2);
        assert_eq!(agent.credentials[0].env, "OPENAI_API_KEY");
        assert_eq!(agent.credentials[1].env, "OPENAI_ORG");
    }

    #[test]
    fn agent_credentials_reject_duplicate_env() {
        let toml = r#"
[project]
name = "p"
description = "d"
[agents.a]
display_name = "A"
package = "x@^1"

[[agents.a.credentials]]
id = "id_one"
env = "SAME"
[[agents.a.credentials]]
id = "id_two"
env = "SAME"
"#;
        let cfg: UncheckedProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            cfg.validate().unwrap_err(),
            ProjectConfigError::CredentialDeclaration { .. }
        ));
    }

    // --- Task 4: validate_models build-time refusals (D7 stage 1) ---

    #[test]
    fn unknown_model_alias_is_refused() {
        let toml = r#"
[project]
name="p"
[models]
haiku = { backend="anthropic", model="claude-haiku-4-5" }
[agents.writer]
display_name="Writer"
package="anthropic@^1"
model = "haiko"
prompt = { system = "hi" }
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::UnknownModelAlias { .. }),
            "expected UnknownModelAlias, got: {err:?}"
        );
    }

    #[test]
    fn model_backend_must_be_declared() {
        let toml = r#"
[project]
name="p"
[models]
gpt = { backend="openai", model="gpt-5" }
[agents.writer]
display_name="Writer"
package="anthropic@^1"
model = "gpt"
prompt = { system = "hi" }
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::ModelBackendNotDeclared { .. }),
            "expected ModelBackendNotDeclared, got: {err:?}"
        );
    }

    #[test]
    fn model_backend_declared_via_packages_table_is_accepted() {
        // The backend `anthropic` is declared ONLY in the top-level
        // `packages` array — no agent uses `package = "anthropic@…"`.
        // This must validate OK: the [packages] table is a legitimate
        // declaration source for model backends.
        let toml = r#"
packages = ["anthropic@^1"]
[project]
name="p"
[models]
haiku = { backend="anthropic", model="claude-haiku-4-5" }
[agents.writer]
display_name="Writer"
package="code-reviewer@^0.1"
model = "haiku"
prompt = { system = "hi" }
"#;
        toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .expect("backend declared in [packages] should be accepted");
    }

    #[test]
    fn agent_without_model_is_refused() {
        let toml = r#"
[project]
name="p"
[agents.writer]
display_name="Writer"
package="anthropic@^1"
prompt = { system = "hi" }
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::MissingAgentModel { .. }),
            "expected MissingAgentModel, got: {err:?}"
        );
    }

    #[test]
    fn unknown_judge_model_alias_is_refused() {
        let toml = r#"
[project]
name="p"

[models]
haiku = { backend="anthropic", model="claude-haiku-4-5" }

[agents.writer]
display_name="Writer"
package="anthropic@^1"
model = "haiku"
produces = ["/workspace/report.md"]
tool_refs = ["write_file"]

[tools.write_file]
native = "WriteFile"
capabilities = [{ kind = "fs.write", paths = ["/workspace/**"] }]

[[pipeline.steps]]
id = "writer"
run = "agent:writer"

[deliverables.report]
path         = "/workspace/report.md"
must_satisfy = "A coherent summary."
judge_model  = "unknown_model"
"#;
        let err = toml::from_str::<UncheckedProjectConfig>(toml)
            .unwrap()
            .validate()
            .unwrap_err();
        assert!(
            matches!(err, ProjectConfigError::UnknownModelAlias { ref referrer, .. } if referrer.contains("deliverable") && referrer.contains("judge")),
            "expected UnknownModelAlias with deliverable/judge referrer, got: {err:?}"
        );
    }

    #[test]
    fn durable_accepts_per_tool_call() {
        let toml = r#"
            [project]
            name = "p"
            [models.m]
            backend = "x"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "x@^0.1"
            model = "m"
            [agents.a.durable]
            checkpoint = "per_tool_call"
            store = "file"
        "#;
        let cfg = parse(toml).expect("valid per_tool_call durable");
        let agent = cfg.agents.get("a").unwrap();
        match agent.durable.as_ref().expect("durable present") {
            DurableEntry::Explicit { checkpoint, .. } => {
                assert_eq!(checkpoint, "per_tool_call");
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
    }

    #[test]
    fn durable_accepts_intent_string() {
        let toml = r#"
            [project]
            name = "p"
            [models.m]
            backend = "p"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            durable = "survive-restarts"
        "#;
        let cfg = parse(toml).expect("valid intent durable");
        let agent = cfg.agents.get("a").expect("agent a");
        match agent.durable.as_ref().expect("durable present") {
            DurableEntry::Intent(s) => assert_eq!(s, "survive-restarts"),
            other => panic!("expected Intent, got {other:?}"),
        }
    }

    #[test]
    fn durable_rejects_unknown_intent_string() {
        let toml = r#"
            [project]
            name = "p"
            [models.m]
            backend = "p"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            durable = "make-it-immortal"
        "#;
        let err = parse(toml).expect_err("unknown intent must fail");
        assert!(
            format!("{err}").contains("survive-restarts"),
            "error should name the accepted intent, got: {err}"
        );
    }

    #[test]
    fn durable_explicit_table_still_parses() {
        let toml = r#"
            [project]
            name = "p"
            [models.m]
            backend = "p"
            model = "m"
            [agents.a]
            display_name = "A"
            package = "p@^0.1"
            model = "m"
            [agents.a.durable]
            checkpoint = "per_turn"
            store = "file"
        "#;
        let cfg = parse(toml).expect("valid explicit durable");
        let agent = cfg.agents.get("a").expect("agent a");
        match agent.durable.as_ref().expect("durable present") {
            DurableEntry::Explicit { checkpoint, store } => {
                assert_eq!(checkpoint, "per_turn");
                assert_eq!(store, "file");
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Generate a name that's a valid TOML key (alphanumeric + underscore).
    fn ident_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]{0,15}"
    }

    /// Generate a non-empty free-form string (no quotes, no backslashes).
    /// Excludes whitespace-only outputs since the validator trims and
    /// rejects empty strings — strategy must produce values that survive
    /// `.trim()` non-empty.
    fn safe_string_strategy() -> impl Strategy<Value = String> {
        "[A-Za-z0-9.]{1,30}"
    }

    fn agent_entry_strategy() -> impl Strategy<Value = (String, String, UncheckedAgent)> {
        (
            ident_strategy(),
            safe_string_strategy(), // display_name
            ident_strategy(),       // package name
        )
            .prop_map(|(id, dn, pkg)| {
                // Use "default" as the model alias; the models table is built
                // from the package name in the proptest body.
                let alias = "default".to_string();
                (
                    id,
                    pkg.clone(),
                    UncheckedAgent {
                        display_name: dn,
                        package: format!("{pkg}@^0.1"),
                        requires: None,
                        capabilities: Vec::new(),
                        config: None,
                        prompt: None,
                        context: None,
                        durable: None,
                        model: Some(alias),
                        tool_refs: Vec::new(),
                        max_turns: None,
                        max_tokens: None,
                        produces: Vec::new(),
                        credentials: Vec::new(),
                        output_schema: None,
                    },
                )
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Round-trip: serialize an UncheckedProjectConfig to TOML, parse-and-validate,
        /// validate the resulting ProjectConfig has the same agent ids.
        #[test]
        fn round_trip_preserves_agent_ids(
            project_name in safe_string_strategy(),
            agents in proptest::collection::vec(agent_entry_strategy(), 0..=3)
        ) {
            // Deduplicate ids (TOML can't have duplicate keys; UncheckedProjectConfig uses BTreeMap).
            let mut agent_map: std::collections::BTreeMap<String, UncheckedAgent> =
                std::collections::BTreeMap::new();
            // Build a [models] table: for each unique package name, add an
            // entry so the "default" alias has a valid declared backend.
            let mut models_map: BTreeMap<String, RawModelEntry> = BTreeMap::new();
            for (id, pkg_name, agent) in agents {
                agent_map.insert(id, agent);
                // Register the package name as the backend for "default".
                // Last one wins; all agents use "default" alias so any
                // declared package is sufficient as long as each agent's
                // package name appears. We insert all to keep the declared
                // set complete, but only one alias ("default") is referenced.
                models_map.insert(
                    "default".to_string(),
                    RawModelEntry { backend: pkg_name, model: "model-v1".to_string() },
                );
            }

            let original = UncheckedProjectConfig {
                project: UncheckedProject {
                    name: project_name.clone(),
                    description: String::new(),
                },
                agents: agent_map.clone(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
                triggers: BTreeMap::new(),
                pipeline: None,
                goals: BTreeMap::new(),
                deliverables: BTreeMap::new(),
                models: models_map,
                packages: Vec::new(),
            };

            let toml_str = toml::to_string(&original).unwrap();

            let parsed: UncheckedProjectConfig = toml::from_str(&toml_str).unwrap();
            let validated = parsed.validate().unwrap();

            prop_assert_eq!(validated.project_name, project_name);
            prop_assert_eq!(
                validated.agents.keys().cloned().collect::<Vec<_>>(),
                agent_map.keys().cloned().collect::<Vec<_>>()
            );
        }
    }
}
