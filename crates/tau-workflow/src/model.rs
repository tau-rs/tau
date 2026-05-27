//! Workflow definition types + TOML parsing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::WorkflowError;

/// A parsed-and-validated workflow definition.
///
/// # Examples
///
/// Parse a minimal workflow from TOML source and inspect its structure:
///
/// ```
/// use tau_workflow::Workflow;
/// use tau_workflow::StepKind;
/// use std::path::Path;
///
/// let src = r#"
/// [workflow]
/// description = "summarise research"
///
/// [[steps]]
/// id = "gather"
/// kind = "agent.run"
/// agent = "researcher"
/// input = "${input}"
///
/// [[steps]]
/// id = "summarise"
/// kind = "agent.run"
/// agent = "summariser"
/// input = "${steps.gather.output}"
/// "#;
///
/// let wf = Workflow::from_str(src, Path::new("workflows/research.toml"))
///     .expect("valid workflow TOML should parse");
///
/// assert_eq!(wf.name, "research");
/// assert_eq!(wf.description.as_deref(), Some("summarise research"));
/// assert_eq!(wf.steps.len(), 2);
/// assert_eq!(wf.steps[0].id, "gather");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    /// Workflow name derived from the file stem (e.g. `research-pipeline`
    /// from `workflows/research-pipeline.toml`).
    pub name: String,
    /// Source file path (preserved for diagnostics).
    pub source_path: PathBuf,
    /// Free-form description from `[workflow].description`.
    pub description: Option<String>,
    /// Optional agent id whose capability grants apply to every
    /// `tool.call` step. Required when any `tool.call` step is present.
    pub default_agent: Option<String>,
    /// Ordered steps; runs sequentially.
    pub steps: Vec<Step>,
}

/// A single workflow step.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Step identifier; must be unique within the workflow.
    pub id: String,
    /// The kind-specific payload.
    pub kind: StepKind,
}

/// What a step does.
///
/// # Examples
///
/// Pattern-match on a parsed `StepKind` to inspect its fields:
///
/// ```
/// use tau_workflow::{StepKind, Workflow};
/// use std::path::Path;
///
/// let src = r#"
/// [[steps]]
/// id = "run-agent"
/// kind = "agent.run"
/// agent = "researcher"
/// input = "${input}"
/// "#;
///
/// let wf = Workflow::from_str(src, Path::new("workflows/example.toml"))
///     .expect("valid TOML should parse");
///
/// match &wf.steps[0].kind {
///     StepKind::AgentRun { agent, input } => {
///         assert_eq!(agent, "researcher");
///         assert_eq!(input, "${input}");
///     }
///     other => panic!("expected AgentRun, got {other:?}"),
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    /// Run an agent declared in tau.toml.
    AgentRun {
        /// Agent id (`[agents.<agent>]` in tau.toml).
        agent: String,
        /// Input template; evaluated against `${input}` + prior step outputs.
        input: String,
    },
    /// Invoke a tool directly without going through an LLM.
    ToolCall {
        /// Tool id (`[plugins.<tool>]` in tau.toml).
        tool: String,
        /// Args object passed verbatim to the tool's `invoke`.
        args: serde_json::Value,
    },
}

#[derive(Deserialize)]
struct RawWorkflow {
    workflow: Option<RawHeader>,
    #[serde(default)]
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
struct RawHeader {
    description: Option<String>,
    #[serde(rename = "default-agent")]
    default_agent: Option<String>,
}

#[derive(Deserialize)]
struct RawStep {
    id: String,
    kind: String,
    agent: Option<String>,
    input: Option<String>,
    tool: Option<String>,
    args: Option<toml::Value>,
}

impl Workflow {
    /// Parse a workflow from a TOML file. Validates step-id uniqueness,
    /// kind-specific required fields, and `default-agent` requirement
    /// when `tool.call` steps are present.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tempfile::tempdir;
    /// # use std::fs;
    /// use tau_workflow::Workflow;
    ///
    /// let dir = tempdir().expect("tempdir should be created");
    /// let path = dir.path().join("my-pipeline.toml");
    /// fs::write(&path, r#"
    /// [[steps]]
    /// id = "step1"
    /// kind = "agent.run"
    /// agent = "helper"
    /// input = "${input}"
    /// "#).expect("write should succeed");
    ///
    /// let wf = Workflow::from_path(&path).expect("file should parse");
    /// assert_eq!(wf.name, "my-pipeline");
    /// assert_eq!(wf.steps.len(), 1);
    /// ```
    pub fn from_path(path: &Path) -> Result<Self, WorkflowError> {
        let bytes = std::fs::read_to_string(path).map_err(|e| WorkflowError::ParseFailed {
            path: path.to_path_buf(),
            message: format!("read failed: {e}"),
        })?;
        Self::from_str(&bytes, path)
    }

    /// Parse from a string + a synthetic source path (for tests + in-memory
    /// callers).
    ///
    /// # Examples
    ///
    /// Parse a valid two-step workflow:
    ///
    /// ```
    /// use tau_workflow::Workflow;
    /// use std::path::Path;
    ///
    /// let src = r#"
    /// [workflow]
    /// description = "two-step pipeline"
    ///
    /// [[steps]]
    /// id = "a"
    /// kind = "agent.run"
    /// agent = "researcher"
    /// input = "${input}"
    ///
    /// [[steps]]
    /// id = "b"
    /// kind = "agent.run"
    /// agent = "summariser"
    /// input = "${steps.a.output}"
    /// "#;
    ///
    /// let wf = Workflow::from_str(src, Path::new("workflows/two-step.toml"))
    ///     .expect("valid TOML should parse");
    /// assert_eq!(wf.name, "two-step");
    /// assert_eq!(wf.steps.len(), 2);
    /// ```
    ///
    /// Invalid TOML returns a `ParseFailed` error:
    ///
    /// ```
    /// use tau_workflow::{Workflow, WorkflowError};
    /// use std::path::Path;
    ///
    /// let err = Workflow::from_str("[[[[invalid", Path::new("workflows/bad.toml"))
    ///     .expect_err("invalid TOML should fail");
    /// assert!(matches!(err, WorkflowError::ParseFailed { .. }));
    /// ```
    pub fn from_str(toml_src: &str, source_path: &Path) -> Result<Self, WorkflowError> {
        let raw: RawWorkflow =
            toml::from_str(toml_src).map_err(|e| WorkflowError::ParseFailed {
                path: source_path.to_path_buf(),
                message: format!("toml parse: {e}"),
            })?;

        let name = source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| WorkflowError::ParseFailed {
                path: source_path.to_path_buf(),
                message: "could not derive name from file path stem".into(),
            })?
            .to_string();

        let description = raw.workflow.as_ref().and_then(|h| h.description.clone());
        let default_agent = raw.workflow.as_ref().and_then(|h| h.default_agent.clone());

        let mut seen_ids: BTreeMap<&str, ()> = BTreeMap::new();
        let mut has_tool_call = false;
        let mut steps = Vec::with_capacity(raw.steps.len());

        for raw_step in &raw.steps {
            if seen_ids.insert(raw_step.id.as_str(), ()).is_some() {
                return Err(WorkflowError::ParseFailed {
                    path: source_path.to_path_buf(),
                    message: format!("duplicate step id {:?}", raw_step.id),
                });
            }

            let kind = match raw_step.kind.as_str() {
                "agent.run" => {
                    let agent =
                        raw_step
                            .agent
                            .clone()
                            .ok_or_else(|| WorkflowError::ParseFailed {
                                path: source_path.to_path_buf(),
                                message: format!(
                                    "step {:?}: agent.run requires `agent` field",
                                    raw_step.id
                                ),
                            })?;
                    let input = raw_step.input.clone().unwrap_or_default();
                    StepKind::AgentRun { agent, input }
                }
                "tool.call" => {
                    has_tool_call = true;
                    let tool = raw_step
                        .tool
                        .clone()
                        .ok_or_else(|| WorkflowError::ParseFailed {
                            path: source_path.to_path_buf(),
                            message: format!(
                                "step {:?}: tool.call requires `tool` field",
                                raw_step.id
                            ),
                        })?;
                    let args = raw_step
                        .args
                        .as_ref()
                        .and_then(|v| serde_json::to_value(v).ok())
                        .unwrap_or(serde_json::Value::Null);
                    StepKind::ToolCall { tool, args }
                }
                other => {
                    return Err(WorkflowError::ParseFailed {
                        path: source_path.to_path_buf(),
                        message: format!(
                            "step {:?}: unknown kind {:?} (expected agent.run or tool.call)",
                            raw_step.id, other
                        ),
                    });
                }
            };

            steps.push(Step {
                id: raw_step.id.clone(),
                kind,
            });
        }

        if has_tool_call && default_agent.is_none() {
            return Err(WorkflowError::ParseFailed {
                path: source_path.to_path_buf(),
                message: "workflow has tool.call step(s) but no [workflow].default-agent".into(),
            });
        }

        Ok(Workflow {
            name,
            source_path: source_path.to_path_buf(),
            description,
            default_agent,
            steps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn synth_path(name: &str) -> PathBuf {
        PathBuf::from(format!("workflows/{name}.toml"))
    }

    #[test]
    fn parses_minimal_two_step_workflow() {
        let src = r#"
[workflow]
description = "test"

[[steps]]
id = "a"
kind = "agent.run"
agent = "researcher"
input = "${input}"

[[steps]]
id = "b"
kind = "agent.run"
agent = "summarizer"
input = "${steps.a.output}"
"#;
        let wf = Workflow::from_str(src, &synth_path("two-step")).expect("parses");
        assert_eq!(wf.name, "two-step");
        assert_eq!(wf.description.as_deref(), Some("test"));
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.steps[0].id, "a");
        match &wf.steps[0].kind {
            StepKind::AgentRun { agent, input } => {
                assert_eq!(agent, "researcher");
                assert_eq!(input, "${input}");
            }
            other => panic!("expected AgentRun, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_step_ids() {
        let src = r#"
[[steps]]
id = "x"
kind = "agent.run"
agent = "a"
[[steps]]
id = "x"
kind = "agent.run"
agent = "b"
"#;
        let err = Workflow::from_str(src, &synth_path("dup")).unwrap_err();
        assert!(format!("{err}").contains("duplicate"), "got {err}");
    }

    #[test]
    fn rejects_unknown_kind() {
        let src = r#"
[[steps]]
id = "a"
kind = "shell.exec"
"#;
        let err = Workflow::from_str(src, &synth_path("badkind")).unwrap_err();
        assert!(format!("{err}").contains("unknown kind"), "got {err}");
    }

    #[test]
    fn rejects_agent_run_without_agent_field() {
        let src = r#"
[[steps]]
id = "a"
kind = "agent.run"
input = "hi"
"#;
        let err = Workflow::from_str(src, &synth_path("nokind")).unwrap_err();
        assert!(format!("{err}").contains("requires `agent`"), "got {err}");
    }

    #[test]
    fn rejects_tool_call_without_default_agent() {
        let src = r#"
[[steps]]
id = "a"
kind = "tool.call"
tool = "fs-read"
args = { path = "/tmp/x" }
"#;
        let err = Workflow::from_str(src, &synth_path("notc")).unwrap_err();
        assert!(format!("{err}").contains("default-agent"), "got {err}");
    }

    #[test]
    fn accepts_tool_call_with_default_agent() {
        let src = r#"
[workflow]
default-agent = "researcher"

[[steps]]
id = "a"
kind = "tool.call"
tool = "fs-read"
args = { path = "/tmp/x" }
"#;
        let wf = Workflow::from_str(src, &synth_path("yestc")).expect("parses");
        assert_eq!(wf.default_agent.as_deref(), Some("researcher"));
        match &wf.steps[0].kind {
            StepKind::ToolCall { tool, args } => {
                assert_eq!(tool, "fs-read");
                assert_eq!(args["path"], "/tmp/x");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }
}
