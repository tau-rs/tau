//! `tau run` — invoke an agent one-shot.
//!
//! Per spec §3.13 (run row) and §3.9.
//!
//! Reads the project `tau.toml`, resolves the named agent to an
//! `(AgentDefinition, PackageManifest)` pair, resolves the agent's
//! LLM-backend + tool plugins from the per-scope lockfile, spawns
//! each plugin process via `tau_runtime::plugin_host::load_*`, builds
//! a [`tau_runtime::Runtime`] populated with the resulting `Arc<dyn Dyn*>` shims,
//! then builds an initial [`Message`] from `--prompt` / stdin, runs the
//! agent, and maps the resulting [`RunOutcome`] to stdout/stderr + an
//! `ExitCode`.
//!
//! # Plugin loading
//!
//! Per spec §11 (migration): the old `--features test-mock` knob has
//! been retired. The agent's `llm_backend` package and each
//! `requires.tools` entry must be installed via `tau install` and
//! carry a `[plugin]` table in their `tau.toml`; the host then spawns
//! the recorded binary at run time. See
//! [`crate::cmd::plugin_loader`] for the resolution logic.

use std::io::Read;
use std::path::PathBuf;

use anyhow::Context;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_plugin_protocol::handshake::TraceContext;
use tau_runtime::{RunOptions, RunOutcome};

use crate::cli::RunArgs;
use crate::cmd::plugin_loader;
use crate::config::ProjectConfig;
use crate::output::Output;

/// Marker error: the agent ran but reported [`RunOutcome::Failed`].
///
/// Threaded through the existing `anyhow::Result<()>` dispatch so
/// `lib::run_main` can downcast it and map to
/// [`crate::ExitCode::AgentFailed`] (exit code 1) — distinct from
/// kernel/CLI errors that map to [`crate::ExitCode::Error`] (exit
/// code 2). See the docstring on [`crate::ExitCode`] and ADR-0006 for
/// the Outcome/Error dichotomy.
#[derive(Debug, thiserror::Error)]
#[error("agent failed (exit code 1)")]
pub(crate) struct AgentFailed;

/// Run `tau run`.
///
/// `record_protocol` is the optional `--record-protocol <path>` global
/// flag (Task 20 / spec §9 debug tier). When `Some`, every plugin
/// frame in either direction is mirrored to the JSONL file at `path`;
/// the recorders are flushed after the runtime drops to ensure
/// pending writes reach disk.
///
/// `force_passthrough` and `force_adapter_kind` come from the global
/// `--no-sandbox` / `--sandbox <kind>` flags (Task 7).
pub async fn run(
    args: &RunArgs,
    record_protocol: Option<PathBuf>,
    force_passthrough: bool,
    force_adapter_kind: Option<tau_runtime::sandbox::registry::RegistryKind>,
    output: &mut Output,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let project_path = cwd.join("tau.toml");
    let project = ProjectConfig::from_path(&project_path)
        .with_context(|| format!("project tau.toml required at {project_path:?}"))?;

    let entry = project.agents.get(&args.agent_id).ok_or_else(|| {
        anyhow::anyhow!(
            "agent id {:?} not found in project tau.toml (declare it under [agents.{}])",
            args.agent_id,
            args.agent_id
        )
    })?;

    let scope = tau_pkg::Scope::resolve(&cwd).context("resolving package scope")?;

    crate::cmd::resolve_helpers::resolve_and_install_for_agent(
        entry,
        &scope,
        args.no_install,
        output,
    )?;

    let (agent_def, manifest) = crate::config::build_agent_definition(entry, &cwd, &scope)
        .with_context(|| format!("resolving agent {:?}", args.agent_id))?;

    let mut options = RunOptions::default();
    if let Some(n) = args.max_turns {
        options.max_turns = n;
    }
    options.project_override = entry.capability_overrides.clone();

    if args.dry_run {
        emit_dry_run(
            entry,
            &agent_def,
            &manifest,
            &options,
            args.prompt.as_deref(),
            output,
        )?;
        return Ok(());
    }

    // ---- Plugin loading -----------------------------------------------------
    //
    // Now that test-mock is gone, every `tau run` invocation spawns the
    // real LLM-backend + tool plugin processes recorded in the lockfile.
    // Failures (plugin not installed / handshake timeout / wrong port)
    // surface as `RuntimeError`s mapped to ExitCode::Error.

    // run_id: per-invocation identifier so plugin-side traces can group
    // events back to this run. Plain UUID — no need to coordinate with
    // the kernel's own internal id (the kernel mints its own
    // AgentInstanceId for the loop; the plugin's run_id is host-side).
    //
    // We intentionally avoid a uuid dep here and use a timestamp-based
    // string: the only contract is uniqueness within a host process,
    // and humantime-style strings are easier to read in logs.
    let run_id = format!(
        "tau-run-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let trace_context = TraceContext::new(run_id, args.agent_id.clone(), "root".to_string());
    let (host_options, _ledger) = plugin_loader::build_host_options(
        record_protocol.as_deref(),
        force_passthrough,
        force_adapter_kind,
    );

    let loaded = plugin_loader::load_plugins(entry, &scope, trace_context, host_options).await?;
    let recorder_ledger = loaded.recorder_ledger.clone();

    let runtime = loaded
        .builder
        .build()
        .context("failed to build runtime from spawned plugins")?;

    // Build the initial user message from --prompt or stdin.
    let prompt_text = match &args.prompt {
        Some(s) => s.clone(),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("reading initial prompt from stdin")?;
            buf
        }
    };

    let initial = Message::new(
        Address::User,
        // The kernel mints its own AgentInstanceId per run; this
        // recipient placeholder gets replaced once the loop assigns its
        // own. Using a fresh id keeps the typed Address::Agent variant
        // happy without leaking implementation detail.
        Address::Agent(AgentInstanceId::new()),
        MessagePayload::Text {
            content: prompt_text,
        },
    );

    // ---- Multi-agent dispatch -----------------------------------------------
    //
    // Trigger: the agent's package manifest declares either an
    // `Agent::Spawn` capability or a `TaskList::*` capability. When
    // either is present, route through `Runtime::spawn_root_agent` instead
    // of the single-agent flow. The single-agent flow is preserved as the
    // default for backward compatibility.
    let is_multi_agent = manifest.capabilities().iter().any(|c| {
        matches!(
            c,
            tau_domain::Capability::Agent(tau_domain::AgentCapability::Spawn { .. })
                | tau_domain::Capability::TaskList { .. }
        )
    });

    if is_multi_agent {
        use crate::cmd::output_orchestration::{print_summary, AgentStats};

        let scope_root = scope.path().to_path_buf();
        // v1.1: spawn_root_agent takes `Arc<Runtime>` so the in-stream
        // agent.<kind>.spawn intercept can recursively invoke run_with_history
        // on the same kernel. Single-agent runs below stay on the non-arc path.
        let budget = tau_ports::RunBudget {
            max_total_tokens: args.max_total_tokens,
            max_total_duration_secs: args.max_total_duration_secs,
            max_total_agents: args.max_total_agents,
        };
        let runtime_arc = std::sync::Arc::new(runtime);
        let snapshot = runtime_arc
            .clone()
            .spawn_root_agent(agent_def, manifest, initial, budget, scope_root)
            .await
            .with_context(|| format!("multi-agent run for agent {:?}", args.agent_id))?;

        drop(runtime_arc);
        plugin_loader::flush_recorders(recorder_ledger).await;

        // Print snapshot summary. Live trace rendering is deferred;
        // v1 reads the JSONL log post-hoc. Stats are empty here because
        // the printer was not wired to the trace stream — that wiring is
        // a follow-up (requires subscribe-handle plumbing through
        // spawn_root_agent).
        let stats: std::collections::BTreeMap<String, AgentStats> =
            std::collections::BTreeMap::new();
        print_summary(&snapshot, &stats);

        return if matches!(snapshot.status, tau_ports::RunStatus::Completed) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "multi-agent run failed (status: {:?})",
                snapshot.status
            ))
        };
    }

    if args.stream {
        let result =
            run_streaming_path(&runtime, agent_def, manifest, initial, options, output).await;
        drop(runtime);
        plugin_loader::flush_recorders(recorder_ledger).await;
        result
    } else {
        let run_outcome = runtime.run(agent_def, manifest, initial, options).await;

        // Drop the runtime explicitly before flushing recorders. This
        // ensures every plugin process is reaped and every host-side write
        // task has completed, so `flush_recorders` drains a quiescent set
        // of file buffers rather than racing with in-flight `record(...)`.
        drop(runtime);
        plugin_loader::flush_recorders(recorder_ledger).await;

        let outcome = run_outcome.context("running agent")?;

        match outcome {
            RunOutcome::Completed {
                ref final_message,
                total_turns,
                ref token_usage,
                ..
            } => {
                if output.is_json() {
                    let payload = serde_json::json!({
                        "outcome": "completed",
                        "final_message": format_message_text(&final_message.payload),
                        "total_turns": total_turns,
                        "token_usage": {
                            "input_tokens": token_usage.input_tokens,
                            "output_tokens": token_usage.output_tokens,
                        },
                    });
                    output.json(&payload)?;
                } else {
                    let text = format_message_text(&final_message.payload);
                    output.human(&text)?;
                }
                Ok(())
            }
            RunOutcome::Failed {
                ref status,
                total_turns,
                ref token_usage,
                ..
            } => {
                if output.is_json() {
                    let payload = serde_json::json!({
                        "outcome": "failed",
                        "status": format!("{status:?}"),
                        "total_turns": total_turns,
                        "token_usage": {
                            "input_tokens": token_usage.input_tokens,
                            "output_tokens": token_usage.output_tokens,
                        },
                    });
                    output.json(&payload)?;
                } else {
                    output.error(format!("agent failed: {status:?}"))?;
                }
                // Marker error → ExitCode::AgentFailed (1) via downcast in
                // crate::run_main. Kernel/CLI errors map to ExitCode::Error
                // (2); this case is the explicit Outcome::Failed bucket.
                Err(AgentFailed.into())
            }
            // RunOutcome is `#[non_exhaustive]`; cross-crate match needs a
            // wildcard. Any future variant should be classified explicitly
            // by an ADR amendment; for now, treat unknown outcomes as a
            // kernel error.
            _ => Err(anyhow::anyhow!("unknown RunOutcome variant")),
        }
    }
}

/// Consume the `run_streaming` stream and render events as they arrive.
///
/// Human mode: text deltas appear inline on stdout (raw `print!` / flush
/// for typewriter UX); tool annotations go to stderr so stdout stays the
/// agent's text. A closing newline is printed on `RunCompleted`.
///
/// JSON mode (`--json`): one JSON object per event line via
/// `Output::json`. Shape: `{"event":"<discriminator>", ...}`.
///
/// The caller is responsible for `drop(runtime)` and
/// `flush_recorders` after this function returns (streaming path or
/// batch path — the drop + flush sequence is the same).
async fn run_streaming_path(
    runtime: &tau_runtime::Runtime,
    agent_def: tau_domain::AgentDefinition,
    manifest: tau_domain::PackageManifest,
    initial: tau_domain::Message,
    options: tau_runtime::RunOptions,
    output: &mut Output,
) -> anyhow::Result<()> {
    use futures_core::stream::Stream as _;
    use std::io::Write as _;
    use tau_runtime::{RunEvent, RunOutcome};

    let stream = runtime
        .run_streaming(agent_def, manifest, initial, options)
        .await
        .context("running agent (streaming)")?;
    let mut stream = Box::pin(stream);

    let mut final_outcome: Option<RunOutcome> = None;
    let mut fatal: Option<(String, String)> = None;

    loop {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            Some(RunEvent::TextDelta { delta }) => {
                if output.is_json() {
                    output.json(&serde_json::json!({
                        "event": "text_delta",
                        "delta": delta,
                    }))?;
                } else {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                }
            }
            Some(RunEvent::ToolCallStarted { id, name, args }) => {
                if output.is_json() {
                    output.json(&serde_json::json!({
                        "event": "tool_call_started",
                        "id": id,
                        "name": name,
                        "args": args,
                    }))?;
                } else {
                    eprintln!("→ calling {name}...");
                }
            }
            Some(RunEvent::ToolCallCompleted { id, name, result }) => {
                if output.is_json() {
                    let result_json = match &result {
                        Ok(_) => serde_json::json!({"ok": true}),
                        Err(reason) => serde_json::json!({"ok": false, "error": reason}),
                    };
                    output.json(&serde_json::json!({
                        "event": "tool_call_completed",
                        "id": id,
                        "name": name,
                        "result": result_json,
                    }))?;
                } else {
                    match &result {
                        Ok(_) => eprintln!("✓ {name} completed"),
                        Err(reason) => eprintln!("✗ {name} failed: {reason}"),
                    }
                }
            }
            Some(RunEvent::TurnCompleted {
                stop_reason,
                usage,
                turn,
            }) => {
                if output.is_json() {
                    let usage_json = match usage {
                        Some(u) => serde_json::json!({
                            "input_tokens": u.input_tokens,
                            "output_tokens": u.output_tokens,
                        }),
                        None => serde_json::Value::Null,
                    };
                    output.json(&serde_json::json!({
                        "event": "turn_completed",
                        "stop_reason": format!("{stop_reason:?}"),
                        "usage": usage_json,
                        "turn": turn,
                    }))?;
                }
                // Human mode: no-op (text deltas already rendered inline).
            }
            Some(RunEvent::RunCompleted { outcome }) => {
                final_outcome = Some(outcome);
                break;
            }
            Some(RunEvent::FatalError { kind, detail, .. }) => {
                fatal = Some((kind, detail));
                break;
            }
            Some(_) => {
                // Forward-compatibility for #[non_exhaustive] RunEvent.
            }
            None => {
                return Err(anyhow::anyhow!(
                    "stream ended without RunCompleted or FatalError"
                ));
            }
        }
    }

    if let Some((kind, detail)) = fatal {
        return Err(anyhow::anyhow!("kernel error ({kind}): {detail}"));
    }
    let outcome = final_outcome.expect("RunCompleted yielded final_outcome");

    // After streaming completes, emit the same closing summary the batch path emits.
    match outcome {
        RunOutcome::Completed {
            ref final_message,
            total_turns,
            ref token_usage,
            ..
        } => {
            if output.is_json() {
                output.json(&serde_json::json!({
                    "event": "run_completed",
                    "outcome": {
                        "outcome": "completed",
                        "final_message": format_message_text(&final_message.payload),
                        "total_turns": total_turns,
                        "token_usage": {
                            "input_tokens": token_usage.input_tokens,
                            "output_tokens": token_usage.output_tokens,
                        },
                    },
                }))?;
            } else {
                // Human mode: text already streamed; print closing newline.
                println!();
            }
            Ok(())
        }
        RunOutcome::Failed {
            ref status,
            total_turns,
            ref token_usage,
            ..
        } => {
            if output.is_json() {
                output.json(&serde_json::json!({
                    "event": "run_completed",
                    "outcome": {
                        "outcome": "failed",
                        "status": format!("{status:?}"),
                        "total_turns": total_turns,
                        "token_usage": {
                            "input_tokens": token_usage.input_tokens,
                            "output_tokens": token_usage.output_tokens,
                        },
                    },
                }))?;
            } else {
                output.error(format!("agent failed: {status:?}"))?;
            }
            Err(AgentFailed.into())
        }
        _ => Err(anyhow::anyhow!("unknown RunOutcome variant")),
    }
}

/// Project a [`MessagePayload`] to a single text string for display.
/// Non-text payloads degrade to a `Debug`-formatted preview.
fn format_message_text(payload: &MessagePayload) -> String {
    match payload {
        MessagePayload::Text { content } => content.clone(),
        other => format!("{other:?}"),
    }
}

/// Render the dry-run preview to stderr per spec §3.9.
fn emit_dry_run(
    entry: &crate::config::AgentEntry,
    agent_def: &tau_domain::AgentDefinition,
    manifest: &tau_domain::PackageManifest,
    options: &RunOptions,
    prompt: Option<&str>,
    output: &mut Output,
) -> anyhow::Result<()> {
    output.dry_run(format!(
        "agent:           {} ({})",
        entry.id, entry.display_name
    ))?;
    output.dry_run(format!(
        "package:         {} {}",
        manifest.name(),
        manifest.version()
    ))?;
    output.dry_run(format!("llm backend:     {}", entry.llm_backend))?;
    if let Some(sp) = &agent_def.system_prompt {
        let preview: String = sp.chars().take(80).collect();
        let suffix = if sp.chars().count() > 80 { "..." } else { "" };
        output.dry_run(format!("system prompt:   {preview}{suffix}"))?;
    } else {
        output.dry_run("system prompt:   (none)")?;
    }
    output.dry_run(format!(
        "granted caps:    {}",
        manifest.capabilities().len()
    ))?;
    output.dry_run(format!("max_turns:       {}", options.max_turns))?;
    let preview = prompt.unwrap_or("(stdin)");
    let len = preview.len();
    let trimmed: String = preview.chars().take(80).collect();
    let suffix = if preview.chars().count() > 80 {
        "..."
    } else {
        ""
    };
    output.dry_run(format!(
        "initial message: {:?}{} ({len} bytes)",
        trimmed, suffix
    ))?;
    output.dry_run("no LLM call made.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::Value;

    #[test]
    fn format_message_text_returns_text_content_directly() {
        let payload = MessagePayload::Text {
            content: "hello".into(),
        };
        assert_eq!(format_message_text(&payload), "hello");
    }

    #[test]
    fn format_message_text_falls_back_to_debug_for_non_text() {
        let payload = MessagePayload::ToolResult { body: Value::Null };
        let s = format_message_text(&payload);
        assert!(s.contains("ToolResult"), "got: {s}");
    }

    #[test]
    fn agent_failed_renders_error_message() {
        let err: anyhow::Error = AgentFailed.into();
        let s = format!("{err}");
        assert!(s.contains("agent failed"), "got: {s}");
    }
}
