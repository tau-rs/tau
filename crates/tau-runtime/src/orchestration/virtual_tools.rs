//! Virtual-tool resolver intercepted before plugin dispatch.
//!
//! When an agent calls a tool whose name starts with `task.` / `run.` /
//! `agent.<kind>.spawn`, the runtime resolves it here instead of forwarding
//! to a plugin host. Result is returned synchronously as a tool_result.

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tau_domain::Capability;
use tau_ports::{AgentId, Task, TaskListFilter, TaskStatus};

use crate::orchestration::error::OrchestrationError;
use crate::orchestration::run_state::RunState;

/// Returns true iff `tool_name` is handled by the virtual-tool resolver.
pub fn is_virtual(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "task.create"
            | "task.claim"
            | "task.heartbeat"
            | "task.release"
            | "task.update"
            | "task.complete"
            | "task.fail"
            | "task.discard"
            | "task.list"
            | "task.get"
            | "run.note"
            | "run.plan"
    ) || (tool_name.starts_with("agent.") && tool_name.ends_with(".spawn"))
        || tool_name
            .strip_prefix("skill.")
            .and_then(|s| s.strip_suffix(".spawn"))
            .map(|name| !name.is_empty())
            .unwrap_or(false)
}

/// Capability requirement for a given virtual tool. Used by the dispatch
/// path to gate the call before invoking the handler.
pub fn required_capability(tool_name: &str) -> Capability {
    match tool_name {
        "task.list" | "task.get" => Capability::TaskList {
            mode: "read".into(),
        },
        "task.create" | "task.claim" | "task.heartbeat" | "task.release" | "task.update"
        | "task.complete" | "task.fail" => Capability::TaskList {
            mode: "write".into(),
        },
        "task.discard" => Capability::TaskList {
            mode: "manage".into(),
        },
        "run.note" => Capability::Plan {
            mode: "write".into(),
        },
        "run.plan" => Capability::Plan {
            mode: "read".into(),
        },
        s if s.starts_with("agent.") && s.ends_with(".spawn") => {
            // The Spawn capability's allowed_kinds list is checked in
            // validate_agent_spawn (Task 9), not here.
            // Use serde round-trip to construct because AgentCapability::Spawn is #[non_exhaustive].
            serde_json::from_value(serde_json::json!({"kind": "agent.spawn", "allowed_kinds": []}))
                .unwrap_or(Capability::Custom {
                    name: "agent.spawn".into(),
                    params: Default::default(),
                })
        }
        s if s.starts_with("skill.") && s.ends_with(".spawn") => {
            // The Spawn capability's allowed_skills list is checked in
            // validate_skill_spawn, not here.
            // Use serde round-trip to construct because SkillCapability::Spawn is #[non_exhaustive].
            serde_json::from_value(serde_json::json!({"kind": "skill.spawn", "allowed_skills": []}))
                .unwrap_or(Capability::Custom {
                    name: "skill.spawn".into(),
                    params: Default::default(),
                })
        }
        _ => Capability::Custom {
            name: tool_name.into(),
            params: Default::default(),
        },
    }
}

/// Dispatch a virtual tool call. Returns the JSON result body (the caller
/// wraps it in a normal tool_result envelope).
///
/// Note: `agent.<kind>.spawn` is NOT dispatched here — it requires recursive
/// Runtime::run invocation at the kernel layer (Task 12). Callers must use
/// `validate_agent_spawn` instead for those tools.
pub fn dispatch(
    tool_name: &str,
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    match tool_name {
        "task.create" => handle_task_create(args, agent_id, state),
        "task.claim" => handle_task_claim(args, agent_id, state),
        "task.heartbeat" => handle_task_heartbeat(args, agent_id, state),
        "task.release" => handle_task_release(args, agent_id, state),
        "task.update" => handle_task_update(args, agent_id, state),
        "task.complete" => handle_task_complete(args, agent_id, state),
        "task.fail" => handle_task_fail(args, agent_id, state),
        "task.discard" => handle_task_discard(args, agent_id, state),
        "task.list" => handle_task_list(args, state),
        "task.get" => handle_task_get(args, state),
        "run.note" => handle_run_note(args, agent_id, state),
        "run.plan" => handle_run_plan(state),
        s if s.starts_with("agent.") && s.ends_with(".spawn") => {
            Err(OrchestrationError::ArgError {
                tool: tool_name.into(),
                detail: "agent.spawn dispatched via virtual_tools::validate_agent_spawn + kernel; not via dispatch()".into(),
            })
        }
        _ => Err(OrchestrationError::ArgError {
            tool: tool_name.into(),
            detail: format!("unknown virtual tool: {tool_name}"),
        }),
    }
}

// ─── task.* handlers ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TaskCreateArgs {
    description: String,
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    parent_task_id: Option<String>,
}

fn handle_task_create(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskCreateArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "task.create".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    let id = state.task_list.create(
        a.description,
        agent_id.clone(),
        a.parent_task_id,
        a.owner_id,
        Utc::now(),
    )?;
    Ok(serde_json::json!({"task_id": id}))
}

#[derive(Deserialize)]
struct TaskIdArg {
    task_id: String,
}

fn handle_task_claim(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskIdArg = serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
        tool: "task.claim".into(),
        detail: format!("arg parse error: {e}"),
    })?;
    state
        .task_list
        .claim(&a.task_id, agent_id.clone(), Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

fn handle_task_heartbeat(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskIdArg = serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
        tool: "task.heartbeat".into(),
        detail: format!("arg parse error: {e}"),
    })?;
    state
        .task_list
        .heartbeat(&a.task_id, agent_id, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

fn handle_task_release(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskIdArg = serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
        tool: "task.release".into(),
        detail: format!("arg parse error: {e}"),
    })?;
    state.task_list.release(&a.task_id, agent_id, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct TaskUpdateArgs {
    task_id: String,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    notes: Option<String>,
}

fn handle_task_update(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskUpdateArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "task.update".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    state
        .task_list
        .update(&a.task_id, agent_id, a.status, a.notes, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct TaskCompleteArgs {
    task_id: String,
    result_summary: String,
}

fn handle_task_complete(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskCompleteArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "task.complete".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    state
        .task_list
        .complete(&a.task_id, agent_id, a.result_summary, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct TaskFailArgs {
    task_id: String,
    error: String,
}

fn handle_task_fail(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskFailArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "task.fail".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    state
        .task_list
        .fail(&a.task_id, agent_id, a.error, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct TaskDiscardArgs {
    task_id: String,
    reason: String,
}

fn handle_task_discard(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: TaskDiscardArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "task.discard".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    state
        .task_list
        .discard(&a.task_id, agent_id, a.reason, Utc::now())?;
    Ok(serde_json::json!({"ok": true}))
}

fn handle_task_list(args: Value, state: &mut RunState) -> Result<Value, OrchestrationError> {
    let filter: TaskListFilter = serde_json::from_value(args).unwrap_or_default();
    let tasks: Vec<Task> = state.task_list.list(&filter);
    Ok(serde_json::json!({"tasks": tasks}))
}

fn handle_task_get(args: Value, state: &mut RunState) -> Result<Value, OrchestrationError> {
    let a: TaskIdArg = serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
        tool: "task.get".into(),
        detail: format!("arg parse error: {e}"),
    })?;
    let task = state.task_list.get(&a.task_id).cloned();
    Ok(serde_json::json!({"task": task}))
}

// ─── run.* handlers ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RunNoteArgs {
    text: String,
}

fn handle_run_note(
    args: Value,
    agent_id: &AgentId,
    state: &mut RunState,
) -> Result<Value, OrchestrationError> {
    let a: RunNoteArgs =
        serde_json::from_value(args).map_err(|e| OrchestrationError::ArgError {
            tool: "run.note".into(),
            detail: format!("arg parse error: {e}"),
        })?;
    let _ = agent_id; // kept for future use (audit trail)
    state.append_plan_note(&a.text);
    Ok(serde_json::json!({"ok": true}))
}

fn handle_run_plan(state: &mut RunState) -> Result<Value, OrchestrationError> {
    Ok(serde_json::json!({"plan": state.plan}))
}

// ─── agent.<kind>.spawn validation ──────────────────────────────────────────

/// Check the capability subset law: every capability in `child_grant` must
/// be present in `parent_grant`. v1 uses strict equality on the JSON-serialized
/// form; future versions may relax to allow narrowing.
pub fn check_capability_subset(
    parent_grant: &[Capability],
    child_grant: &[Capability],
) -> Result<(), OrchestrationError> {
    let parent_keys: std::collections::BTreeSet<String> = parent_grant
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect();
    let extras: Vec<String> = child_grant
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .filter(|k| !parent_keys.contains(k))
        .collect();
    if extras.is_empty() {
        Ok(())
    } else {
        Err(OrchestrationError::GrantNotSubset { extras })
    }
}

#[derive(Deserialize)]
struct AgentSpawnArgs {
    /// Capabilities to grant the child (must be ⊆ parent's grant).
    #[serde(default)]
    grant: Vec<Capability>,
    /// Initial user message for the child.
    message: String,
    /// Optional system prompt for the child. When `None`, the child
    /// inherits the parent's system_prompt. When `Some`, replaces it.
    /// This is the v1.2 foundation for per-kind agent definitions
    /// (a "skill" = `(system_prompt, grant, optional tools)`).
    #[serde(default)]
    system_prompt: Option<String>,
    // `kind` is in the tool name, not here — ignore any `kind` field in args.
}

/// A validated agent.spawn request, ready for the runtime kernel to
/// transform into a recursive `Runtime::run` invocation.
#[derive(Debug, Clone)]
pub struct AgentSpawnRequest {
    /// The agent kind parsed from the tool name.
    pub kind: String,
    /// Capabilities granted to the child (verified ⊆ parent's grant).
    pub grant: Vec<Capability>,
    /// Initial user message for the child.
    pub message: String,
    /// Optional system prompt override for the child. `None` ⇒ inherit
    /// parent's. v1.2: foundation for per-kind agent definitions.
    pub system_prompt: Option<String>,
}

/// Validate an `agent.<kind>.spawn` virtual tool call.
///
/// 1. Parses `kind` from the tool name.
/// 2. Parses args.
/// 3. Checks spawn authorization (parent's `Agent::Spawn { allowed_kinds }` must include `kind`).
/// 4. Checks capability subset law (`child.grant ⊆ parent.grant`).
///
/// Returns a validated [`AgentSpawnRequest`] the kernel uses to invoke a
/// recursive `Runtime::run`.
pub fn validate_agent_spawn(
    tool_name: &str,
    args: &Value,
    parent: &AgentId,
    parent_grant: &[Capability],
) -> Result<AgentSpawnRequest, OrchestrationError> {
    let kind = tool_name
        .strip_prefix("agent.")
        .and_then(|s| s.strip_suffix(".spawn"))
        .ok_or_else(|| OrchestrationError::ArgError {
            tool: tool_name.into(),
            detail: "malformed virtual tool name".into(),
        })?;

    let a: AgentSpawnArgs =
        serde_json::from_value(args.clone()).map_err(|e| OrchestrationError::ArgError {
            tool: tool_name.into(),
            detail: format!("arg parse error: {e}"),
        })?;

    // Spawn-authorization check: parent must have Agent(Spawn) granting `kind`.
    let allowed = parent_grant.iter().any(|c| match c {
        Capability::Agent(tau_domain::AgentCapability::Spawn { allowed_kinds, .. }) => {
            allowed_kinds.iter().any(|k| k == kind)
        }
        _ => false,
    });
    if !allowed {
        return Err(OrchestrationError::SpawnNotAuthorized {
            parent: parent.clone(),
            kind: kind.into(),
        });
    }

    // Capability subset law: child.grant ⊆ parent.grant.
    check_capability_subset(parent_grant, &a.grant)?;

    Ok(AgentSpawnRequest {
        kind: kind.into(),
        grant: a.grant,
        message: a.message,
        system_prompt: a.system_prompt,
    })
}

// ─── skill.<name>.spawn validation ─────────────────────────────────────────

/// Validate a `skill.<name>.spawn` virtual tool call.
///
/// 1. Parses `name` from the tool name.
/// 2. Parses args (`message`, optional `system_prompt`, optional `scope_paths`).
/// 3. Checks spawn authorization (parent's
///    `Capability::Skill(SkillCapability::Spawn { allowed_skills })` must
///    include `name`).
/// 4. Looks up the installed skill + substitutes `${SKILL_DIR}` +
///    applies `scope_paths` + verifies capability subset law via
///    [`crate::orchestration::resolve_skill_for_spawn`].
///
/// Returns a fully validated [`crate::orchestration::SkillSpawnRequest`]
/// the kernel uses to invoke a recursive `Runtime::run`.
pub fn validate_skill_spawn(
    tool_name: &str,
    args: &Value,
    parent: &AgentId,
    parent_grant: &[Capability],
    scope: &tau_pkg::Scope,
) -> Result<crate::orchestration::SkillSpawnRequest, OrchestrationError> {
    let name = tool_name
        .strip_prefix("skill.")
        .and_then(|s| s.strip_suffix(".spawn"))
        .ok_or_else(|| OrchestrationError::ArgError {
            tool: tool_name.into(),
            detail: "malformed skill.<name>.spawn tool name".into(),
        })?;

    #[derive(serde::Deserialize)]
    struct SkillSpawnArgsRaw {
        message: String,
        #[serde(default)]
        system_prompt: Option<String>,
        #[serde(default)]
        scope_paths: Option<Vec<String>>,
    }
    let a: SkillSpawnArgsRaw =
        serde_json::from_value(args.clone()).map_err(|e| OrchestrationError::ArgError {
            tool: tool_name.into(),
            detail: format!("skill.<name>.spawn args: {e}"),
        })?;

    // Spawn authorization: parent must have Skill(Spawn) granting `name`.
    let allowed = parent_grant.iter().any(|c| match c {
        Capability::Skill(tau_domain::SkillCapability::Spawn { allowed_skills, .. }) => {
            allowed_skills.iter().any(|s| s == name)
        }
        _ => false,
    });
    if !allowed {
        return Err(OrchestrationError::SkillSpawnNotAuthorized {
            parent: parent.clone(),
            name: name.into(),
        });
    }

    let spawn_args = crate::orchestration::SkillSpawnArgs {
        message: a.message,
        system_prompt: a.system_prompt,
        scope_paths: a.scope_paths,
    };
    crate::orchestration::resolve_skill_for_spawn(name, &spawn_args, parent_grant, scope)
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tau_ports::RunBudget;

    fn new_state() -> RunState {
        RunState::new("r".into(), "root".into(), RunBudget::default(), Utc::now())
    }

    // ── Task 7: task.* classification + lifecycle ────────────────────────────

    #[test]
    fn is_virtual_recognizes_task_family() {
        assert!(is_virtual("task.create"));
        assert!(is_virtual("task.complete"));
        assert!(is_virtual("task.list"));
        assert!(!is_virtual("fs.read"));
    }

    #[test]
    fn is_virtual_recognizes_agent_spawn() {
        assert!(is_virtual("agent.researcher.spawn"));
        assert!(is_virtual("agent.writer.spawn"));
        assert!(!is_virtual("agent.researcher"));
    }

    #[test]
    fn task_create_then_get_round_trip() {
        let mut state = new_state();
        let create_args = serde_json::json!({"description": "do thing"});
        let result = dispatch("task.create", create_args, &"agent_x".into(), &mut state).unwrap();
        let task_id = result["task_id"].as_str().unwrap().to_string();

        let get_args = serde_json::json!({"task_id": task_id});
        let get_result = dispatch("task.get", get_args, &"agent_x".into(), &mut state).unwrap();
        assert!(get_result["task"].is_object());
        assert_eq!(get_result["task"]["description"], "do thing");
    }

    #[test]
    fn task_claim_then_complete_full_lifecycle() {
        let mut state = new_state();
        let res = dispatch(
            "task.create",
            serde_json::json!({"description": "x"}),
            &"agent_x".into(),
            &mut state,
        )
        .unwrap();
        let id = res["task_id"].as_str().unwrap().to_string();

        dispatch(
            "task.claim",
            serde_json::json!({"task_id": id}),
            &"agent_x".into(),
            &mut state,
        )
        .unwrap();
        dispatch(
            "task.complete",
            serde_json::json!({"task_id": id, "result_summary": "done"}),
            &"agent_x".into(),
            &mut state,
        )
        .unwrap();

        let g = dispatch(
            "task.get",
            serde_json::json!({"task_id": id}),
            &"agent_x".into(),
            &mut state,
        )
        .unwrap();
        assert_eq!(g["task"]["status"], "done");
    }

    #[test]
    fn required_capability_maps_correctly() {
        assert_matches!(
            required_capability("task.list"),
            Capability::TaskList { mode } => assert_eq!(mode, "read")
        );
        assert_matches!(
            required_capability("task.create"),
            Capability::TaskList { mode } => assert_eq!(mode, "write")
        );
        assert_matches!(
            required_capability("task.discard"),
            Capability::TaskList { mode } => assert_eq!(mode, "manage")
        );
    }

    // ── Task 8: run.* ────────────────────────────────────────────────────────

    #[test]
    fn run_note_appends_to_plan() {
        let mut state = new_state();
        dispatch(
            "run.note",
            serde_json::json!({"text": "first thought"}),
            &"a".into(),
            &mut state,
        )
        .unwrap();
        dispatch(
            "run.note",
            serde_json::json!({"text": "second thought"}),
            &"a".into(),
            &mut state,
        )
        .unwrap();
        let plan = dispatch("run.plan", Value::Null, &"a".into(), &mut state).unwrap();
        let text = plan["plan"].as_str().unwrap();
        assert!(text.contains("first thought"));
        assert!(text.contains("second thought"));
    }

    // ── Task 9: agent.spawn + capability subset law ──────────────────────────

    #[test]
    fn validate_agent_spawn_rejects_unauthorized_kind() {
        // Use serde construction to avoid #[non_exhaustive] block on AgentCapability::Spawn
        let parent_grant: Vec<Capability> = vec![serde_json::from_value(serde_json::json!({
            "kind": "agent.spawn",
            "allowed_kinds": ["researcher"]
        }))
        .unwrap()];
        let args = serde_json::json!({"message": "hi"});
        let err = validate_agent_spawn("agent.writer.spawn", &args, &"p".into(), &parent_grant)
            .unwrap_err();
        assert!(matches!(err, OrchestrationError::SpawnNotAuthorized { .. }));
    }

    #[test]
    fn validate_agent_spawn_accepts_authorized_kind() {
        // Use serde construction to avoid #[non_exhaustive] block on AgentCapability::Spawn
        let parent_grant: Vec<Capability> = vec![serde_json::from_value(serde_json::json!({
            "kind": "agent.spawn",
            "allowed_kinds": ["researcher"]
        }))
        .unwrap()];
        let args = serde_json::json!({"message": "hi"});
        let req = validate_agent_spawn("agent.researcher.spawn", &args, &"p".into(), &parent_grant)
            .unwrap();
        assert_eq!(req.kind, "researcher");
        assert_eq!(req.message, "hi");
        assert!(req.system_prompt.is_none());
    }

    #[test]
    fn validate_agent_spawn_passes_system_prompt_through() {
        // v1.2: spawn args may carry an optional system_prompt override
        // so the orchestrator can spawn semantically different children
        // (the foundation for skills — see ROADMAP §"Skills as packages").
        let parent_grant: Vec<Capability> = vec![serde_json::from_value(serde_json::json!({
            "kind": "agent.spawn",
            "allowed_kinds": ["critic"]
        }))
        .unwrap()];
        let args = serde_json::json!({
            "message": "review this draft",
            "system_prompt": "You are a strict editor. Flag every claim that lacks a source."
        });
        let req =
            validate_agent_spawn("agent.critic.spawn", &args, &"p".into(), &parent_grant).unwrap();
        assert_eq!(req.kind, "critic");
        assert_eq!(
            req.system_prompt.as_deref(),
            Some("You are a strict editor. Flag every claim that lacks a source.")
        );
    }

    #[test]
    fn capability_subset_rejects_extras() {
        let parent = vec![Capability::TaskList {
            mode: "read".into(),
        }];
        let child = vec![
            Capability::TaskList {
                mode: "read".into(),
            },
            Capability::TaskList {
                mode: "write".into(),
            }, // not in parent
        ];
        let err = check_capability_subset(&parent, &child).unwrap_err();
        assert!(matches!(err, OrchestrationError::GrantNotSubset { .. }));
    }

    #[test]
    fn capability_subset_allows_exact_subset() {
        let parent = vec![
            Capability::TaskList {
                mode: "read".into(),
            },
            Capability::TaskList {
                mode: "write".into(),
            },
        ];
        let child = vec![Capability::TaskList {
            mode: "read".into(),
        }];
        check_capability_subset(&parent, &child).unwrap();
    }

    // ── Task 5: skill.<name>.spawn classification + required_capability ──────

    #[test]
    fn is_virtual_recognizes_skill_spawn() {
        assert!(is_virtual("skill.critic.spawn"));
        assert!(is_virtual("skill.fact-checker.spawn"));
        // Malformed: missing name segment
        assert!(!is_virtual("skill.spawn"));
        // Malformed: missing .spawn suffix
        assert!(!is_virtual("skill.critic"));
    }

    #[test]
    fn required_capability_for_skill_spawn_is_skill_variant() {
        let cap = required_capability("skill.critic.spawn");
        assert!(matches!(
            cap,
            Capability::Skill(tau_domain::SkillCapability::Spawn { .. })
        ));
    }
}
