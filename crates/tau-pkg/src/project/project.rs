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
    /// LLM backend identifier; resolved at lookup time.
    pub llm_backend: String,
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

// ----- IR lowering structs (β.2.2) -----

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

// ----- Validated shapes -----

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
///     "anthropic".to_string(),
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
    /// LLM backend identifier.
    pub llm_backend: String,
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
        llm_backend: String,
        requires: RequiresEntry,
        config: BTreeMap<String, toml::Value>,
        prompt: PromptEntry,
        capability_overrides: Vec<crate::capability_override::CapabilityOverride>,
    ) -> Self {
        Self {
            id,
            display_name,
            package,
            llm_backend,
            requires,
            config,
            prompt,
            capability_overrides,
            model: String::new(),
            tool_refs: Vec::new(),
            max_turns: None,
            max_tokens: None,
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

        Ok(ProjectConfig {
            project_name: self.project.name,
            description: self.project.description,
            agents,
            tools,
            steps,
        })
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
    if raw.llm_backend.trim().is_empty() {
        return Err(ProjectConfigError::AgentValidation {
            id,
            message: "llm_backend must be non-empty".into(),
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

    Ok(AgentEntry {
        id,
        display_name: raw.display_name,
        package: raw.package,
        llm_backend: raw.llm_backend,
        requires,
        config,
        prompt,
        capability_overrides,
        model: raw.model.unwrap_or_default(),
        tool_refs: raw.tool_refs,
        max_turns: raw.max_turns,
        max_tokens: raw.max_tokens,
    })
}

fn validate_tool(name: String, raw: UncheckedTool) -> Result<ToolEntry, ProjectConfigError> {
    // Currently no semantic validation beyond what serde already enforced.
    // Placeholder: future validation (e.g. non-empty native fn_name) lives here.
    Ok(ToolEntry {
        name,
        body: raw.body,
        description: raw.description,
        input_schema: raw.input_schema,
        capabilities: raw.capabilities,
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

            [agents.reviewer]
            display_name = "Code Reviewer"
            package      = "code-reviewer@^0.1"
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

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
            llm_backend  = "anthropic"

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
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"
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
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

            [agents.r.prompt]
            system = "be helpful"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert!(matches!(&agent.prompt, PromptEntry::Inline(s) if s == "be helpful"));
    }

    #[test]
    fn validate_accepts_prompt_with_only_system_file() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"
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
            llm_backend  = "anthropic"
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
            llm_backend  = "anthropic"
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::AgentValidation { message, .. }) = result else {
            panic!()
        };
        assert!(message.contains("package"));
    }

    #[test]
    fn validate_rejects_empty_llm_backend() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = ""
        "#;
        let result = parse(toml_str);
        let Err(ProjectConfigError::AgentValidation { message, .. }) = result else {
            panic!()
        };
        assert!(message.contains("llm_backend"));
    }

    #[test]
    fn validate_rejects_bare_string_tools_entry() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

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

            [agents.alpha]
            display_name = "Alpha"
            package      = "p@^0.1"
            llm_backend  = "anthropic"

            [agents.beta]
            display_name = "Beta"
            package      = "q@^0.1"
            llm_backend  = "openai"
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
    fn parse_agent_ir_fields() {
        let toml_str = r#"
            [project]
            name = "x"

            [agents.monitor]
            display_name = "Monitor"
            package      = "p@^0.1"
            llm_backend  = "anthropic"
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

            [agents.r]
            display_name = "R"
            package      = "p@^0.1"
            llm_backend  = "anthropic"
        "#;
        let cfg = parse(toml_str).unwrap();
        let agent = cfg.agents.get("r").unwrap();
        assert_eq!(agent.model, "");
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

    fn agent_entry_strategy() -> impl Strategy<Value = (String, UncheckedAgent)> {
        (
            ident_strategy(),
            safe_string_strategy(), // display_name
            ident_strategy(),       // package name
            ident_strategy(),       // llm_backend
        )
            .prop_map(|(id, dn, pkg, llm)| {
                (
                    id,
                    UncheckedAgent {
                        display_name: dn,
                        package: format!("{pkg}@^0.1"),
                        llm_backend: llm,
                        requires: None,
                        capabilities: Vec::new(),
                        config: None,
                        prompt: None,
                        model: None,
                        tool_refs: Vec::new(),
                        max_turns: None,
                        max_tokens: None,
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
            for (id, agent) in agents {
                agent_map.insert(id, agent);
            }

            let original = UncheckedProjectConfig {
                project: UncheckedProject {
                    name: project_name.clone(),
                    description: String::new(),
                },
                agents: agent_map.clone(),
                tools: BTreeMap::new(),
                steps: BTreeMap::new(),
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
