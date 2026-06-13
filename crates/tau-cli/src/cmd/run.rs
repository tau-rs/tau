//! `tau run` — invoke an agent one-shot.
//!
//! Per spec §3.13 (run row) and §3.9.
//!
//! Reads the project `tau.toml`, resolves the named agent to an
//! `(AgentDefinition, PackageManifest)` pair, resolves the agent's
//! LLM-backend + tool plugins from the per-scope lockfile, spawns
//! each plugin process via `tau_runtime_tokio::plugin_host::load_*`, builds
//! a [`tau_runtime_tokio::Runtime`] populated with the resulting `Arc<dyn Dyn*>` shims,
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
use tau_runtime_tokio::{RunOptions, RunOutcome};

use crate::cli::RunArgs;
use crate::cmd::plugin_loader;
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

/// Marker error: `tau run --bundle` verification failed.
///
/// Carries the structured-renderer output and the spec §C.3 exit code so
/// `run_main` can print the guided message (no generic "error:" prefix)
/// and exit with the mapped code — instead of the command body calling
/// `process::exit` and bypassing the shared error path (D9).
#[derive(Debug, thiserror::Error)]
#[error("bundle verification failed")]
pub(crate) struct BundleVerifyFailed {
    /// Process exit code per spec §C.3 (2 / 3 / 70).
    pub(crate) code: i32,
    /// Pre-rendered guided message (already through `render_verify_error`).
    pub(crate) rendered: String,
}

/// Build a [`BundleVerifyFailed`] from a `VerifyError`: structured
/// renderer output paired with the §C.3 exit-code mapping.
pub(crate) fn bundle_verify_failure(e: &tau_pkg::bundle::VerifyError) -> BundleVerifyFailed {
    BundleVerifyFailed {
        code: bundle_verify_exit_code(e),
        rendered: crate::cmd::error_render::render_verify_error(e),
    }
}

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
    force_adapter_kind: Option<tau_runtime_tokio::process_gate::registry::RegistryKind>,
    output: &mut Output,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // §C.3: when --bundle is set, verify the cwd matches the sealed
    // bundle before doing anything else. On success the cwd's tau.toml
    // is provably the bundle's source, so the rest of `run` proceeds
    // unchanged.
    //
    // β.2.6: when the verified bundle carries an `ir_payload` (v2
    // bundle), short-circuit through the IR-interpreter path
    // ([`crate::cmd::ir_dispatcher::run_via_ir`]) instead of the
    // cwd-based agent loop. v1 bundles (no `ir_payload`) keep using
    // the cwd path below — `verify_bundle` already proved the cwd
    // matches the bundle's recorded source bytes, so the cwd path is
    // equivalent at v1.
    if let Some(bundle_path) = &args.bundle {
        match verify_bundle_against_source(&cwd, bundle_path) {
            Err(e) => {
                // D9: route through the shared renderer + exit.rs mapping
                // in run_main rather than printing + process::exit here.
                return Err(bundle_verify_failure(&e).into());
            }
            Ok(report) => {
                if let Some(ir) = report.manifest.ir_payload.as_ref() {
                    let bytes = ir.canonical_ir_bytes().map_err(|e| {
                        anyhow::anyhow!("decoding bundle's canonical_ir_bytes_hex: {e:?}")
                    })?;
                    let module = tau_ir::from_canonical_bytes(&bytes)
                        .map_err(|e| anyhow::anyhow!("decoding IR module from bundle: {e:?}"))?;

                    if args.dry_run {
                        tracing::info!(
                            ir_format = %ir.ir_format,
                            "v2 bundle (ir_payload present); --dry-run requested \
                             — IR-interpreter path skipped, falling through to \
                             cwd-based dry-run preview"
                        );
                        // Fall through into the legacy cwd path below: the
                        // cwd's tau.toml was just proven byte-clean, so its
                        // dry-run preview is equivalent to what the IR
                        // payload would render.
                    } else {
                        return crate::cmd::ir_dispatcher::run_via_ir(
                            module,
                            args,
                            record_protocol,
                            force_passthrough,
                            force_adapter_kind,
                            output,
                        )
                        .await;
                    }
                }
            }
        }
    }

    let crate::cmd::project_load::LoadedProject { project, .. } =
        crate::cmd::project_load::load_project(&cwd).with_context(|| {
            format!("loading project from {cwd:?} (expected tau.toml or a .ts file)",)
        })?;

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
    if !entry.capability_overrides.is_empty() {
        options.capability_resolver = Some(
            tau_runtime_tokio::capability_resolver_impl::resolver_from_overrides(
                entry.capability_overrides.clone(),
            ),
        );
    }

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
    // events back to this run. No need to coordinate with the kernel's
    // own internal id (the kernel mints its own AgentInstanceId for the
    // loop; the plugin's run_id is host-side).
    let run_id = mint_run_id();
    let trace_context = TraceContext::new(run_id, args.agent_id.clone(), "root".to_string());
    let host_options = plugin_loader::build_host_options(
        record_protocol.as_deref(),
        force_passthrough,
        force_adapter_kind,
    );

    let loaded = plugin_loader::load_plugins(entry, &scope, trace_context, host_options).await?;

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
            content: prompt_text.clone(),
        },
    );

    // ---- Pipeline dispatch (Plan 1 Task 11) ---------------------------------
    //
    // Trigger: the project lowers to an `IrModule` whose
    // `workflow.pipeline` is `Some`. When a `[[pipeline.steps]]` block is
    // declared, the engine sequences the whole pipeline via
    // `run_pipeline`, threading each step's output to later steps. When no
    // pipeline is declared (the overwhelming majority of projects), this
    // returns `None` and the single-agent path below runs BYTE-FOR-BYTE
    // unchanged.
    if let Some(result) = try_run_pipeline(&project, &runtime, prompt_text, output).await {
        drop(runtime);
        plugin_loader::flush_recorders().await;
        return result;
    }

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
        let snapshot = tau_runtime_tokio::drive(
            runtime_arc.clone(),
            agent_def,
            manifest,
            initial,
            budget,
            scope_root,
        )
        .await
        .with_context(|| format!("multi-agent run for agent {:?}", args.agent_id))?;

        drop(runtime_arc);
        plugin_loader::flush_recorders().await;

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
        plugin_loader::flush_recorders().await;
        result
    } else {
        let run_outcome = runtime.run(agent_def, manifest, initial, options).await;

        // Drop the runtime explicitly before flushing recorders. This
        // ensures every plugin process is reaped and every host-side write
        // task has completed, so `flush_recorders` drains a quiescent set
        // of file buffers rather than racing with in-flight `record(...)`.
        drop(runtime);
        plugin_loader::flush_recorders().await;

        let outcome = run_outcome.context("running agent")?;

        render_outcome(outcome, output)
    }
}

/// If `project` declares a `[[pipeline.steps]]` block, drive the whole
/// pipeline and render the final step's output; otherwise return `None`
/// so the caller falls through to the single-agent path unchanged.
///
/// Lowering the cwd project to an `IrModule` is what makes the pipeline
/// addressable: `module.workflow.pipeline` is `Some` exactly when the
/// project declares pipeline steps. A project that does NOT declare a
/// pipeline (or that fails to lower for any reason) returns `None`, so
/// the legacy multi-agent / streaming / batch flow is byte-for-byte
/// preserved as the default.
///
/// The dispatcher reuses the SAME plugin `runtime` the single-agent path
/// built (one `echo-llm`-style backend + the tools recorded in the
/// lockfile), forwarding `invoke(tool_id, args)` to the host's plugin
/// registry via [`crate::cmd::ir_dispatcher::ForwardingDispatcher`]. The
/// run input is the SAME `prompt_text` the single-agent path feeds its
/// initial message.
async fn try_run_pipeline(
    project: &tau_pkg::project::ProjectConfig,
    runtime: &tau_runtime_tokio::Runtime,
    prompt_text: String,
    output: &mut Output,
) -> Option<anyhow::Result<()>> {
    // Lower the cwd project to an IR module so we can inspect its
    // `workflow.pipeline`. Mirror `build::lower_ir`'s caches: a SHA-256
    // stand-in for native-tool content hashes, an empty MCP cache, and
    // no skill resolution. A lowering failure is NOT fatal here — it just
    // means "no pipeline path", so we fall through to the legacy flow
    // (which has its own, independent validation/errors).
    let caches = tau_ir::lower::Caches {
        native_tool: &crate::cmd::build::native_tool_hash,
        mcp_contract: &|_url| None,
        skill: &|_name| None,
    };
    let module = match tau_ir::lower::lower_project(
        project,
        &tau_ports::target::TargetTriple::host(),
        &caches,
    ) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("pipeline lowering skipped (project did not lower): {e}");
            return None;
        }
    };

    // No `[[pipeline.steps]]` → single-agent path. Return `None` so the
    // caller's existing flow runs unchanged.
    let pipeline = module.workflow.pipeline.as_ref()?;

    // The id of the LAST NON-CHECK pipeline step — its stored output is the
    // run's final result. Trailing `StepRun::Check` steps (auto-appended by
    // the lowerer for every `[goals.*]` / `[deliverables.*]` declaration)
    // evaluate postconditions but store NO output in `OutputStore`. Selecting
    // one as `last_step_id` would make `render_pipeline_result` call
    // `store.get(check_step_id) → None` and return the "interpreter invariant
    // violated" error even when all checks pass. Skip trailing check steps and
    // find the last step that actually produces output.
    //
    // If the pipeline consists entirely of check steps (unusual but possible),
    // return `None` to fall back to the single-agent path — there is no
    // output to render.
    let last_step_id = match pipeline
        .steps
        .iter()
        .rev()
        .find(|s| !matches!(s.run, tau_ir::pipeline::StepRun::Check(_)))
    {
        Some(s) => s.id.0.clone(),
        None => return None,
    };

    // Build the forwarding dispatcher from the runtime the single-agent
    // path already built: one LLM backend + every tool the IR references
    // that the runtime actually loaded. Mirrors `ir_dispatcher::run_via_ir`
    // steps 5/5b (minus MCP, which pipeline v0 does not wire).
    let llm_backend = match runtime.llm_backends().values().next().cloned() {
        Some(b) => b,
        None => {
            return Some(Err(anyhow::anyhow!(
                "runtime has no LLM backend after plugin load (pipeline run)"
            )))
        }
    };
    let mut tools_by_id = std::collections::BTreeMap::new();
    for ir_tool_id in module.workflow.tools.keys() {
        if let Some(handle) = runtime.tools().get(&ir_tool_id.0) {
            tools_by_id.insert(ir_tool_id.clone(), handle.clone());
        }
    }
    let dispatcher = std::sync::Arc::new(crate::cmd::ir_dispatcher::ForwardingDispatcher::new(
        llm_backend,
        tools_by_id,
    ));

    // Drive the pipeline, reusing the SAME input (`prompt_text`) and the
    // SAME dispatcher.
    let store = match tau_runtime_core::interpreter::pipeline::run_pipeline(
        std::sync::Arc::new(module),
        prompt_text,
        dispatcher,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return Some(Err(anyhow::Error::new(e).context("running pipeline"))),
    };

    Some(render_pipeline_result(&store, &last_step_id, output))
}

/// Render the LAST NON-CHECK pipeline step's output as the run result,
/// matching the single-agent path's `RunOutcome::Completed` output style:
/// human mode prints the value's text form; `--json` emits the same
/// `{"outcome":"completed", ...}` shape with the final step's text as
/// `final_message` (token usage / turn counts are pipeline-level concepts
/// not yet aggregated — reported as zero/empty at v0).
///
/// Trailing `StepRun::Check` steps (lowered from `[goals.*]` /
/// `[deliverables.*]`) evaluate postconditions but store no output — the
/// caller (`try_run_pipeline`) is responsible for passing the id of the last
/// step that actually produced output (i.e., the last non-check step).
fn render_pipeline_result(
    store: &tau_runtime_core::interpreter::output_store::OutputStore,
    last_step_id: &str,
    output: &mut Output,
) -> anyhow::Result<()> {
    let value = store.get(last_step_id).ok_or_else(|| {
        anyhow::anyhow!(
            "pipeline completed but the final step {last_step_id:?} recorded no output \
             (interpreter invariant violated)"
        )
    })?;
    // A `Value::String` renders as its inner text (the agent's assistant
    // text / a tool's plain-text output); any structured value renders as
    // compact JSON — symmetric with `OutputStore::template_map`.
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    if output.is_json() {
        let payload = serde_json::json!({
            "outcome": "completed",
            "final_message": text,
        });
        output.json(&payload)?;
    } else {
        output.human(&text)?;
    }
    Ok(())
}

/// Render a [`RunOutcome`] from the batch (non-streaming) path.
///
/// Promoted from an inline `match` so both the cwd-based path and the
/// IR-bundle path ([`crate::cmd::ir_dispatcher`]) emit byte-identical
/// `{"outcome":"completed", ...}` / `{"outcome":"failed", ...}` JSON
/// shapes — the streaming path in [`run_streaming_and_render`] wraps
/// the same outcome inside an `{"event":"run_completed", "outcome":{...}}`
/// envelope, so it is kept separate by design.
///
/// Returns `Ok(())` on a `Completed`, `Err(AgentFailed)` on `Failed`,
/// and an `anyhow::Error` for any unknown `#[non_exhaustive]` variant.
pub(super) fn render_outcome(outcome: RunOutcome, output: &mut Output) -> anyhow::Result<()> {
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
    runtime: &tau_runtime_tokio::Runtime,
    agent_def: tau_domain::AgentDefinition,
    manifest: tau_domain::PackageManifest,
    initial: tau_domain::Message,
    options: tau_runtime_tokio::RunOptions,
    output: &mut Output,
) -> anyhow::Result<()> {
    use futures_core::stream::Stream as _;
    use std::io::Write as _;
    use tau_runtime_tokio::{RunEvent, RunOutcome};

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

/// Re-lower the cwd source and verify `bundle_path` against it.
///
/// Computes the canonical IR hash of the local `tau.toml` and hands it to
/// [`tau_pkg::bundle::verify_bundle`] so the bundle's embedded IR can be
/// cross-checked against the source it claims to come from (verify step
/// 10). Bundles are host-sealed (verify step 5 rejects a foreign target),
/// so lowering for the host target is correct: a foreign-target bundle is
/// rejected by step 5 before the divergence check is reached.
///
/// `lower_ir` returning `None` (the source no longer lowers) flows through
/// as `recomputed_ir_hash: None`, which verify_bundle turns into a
/// fail-closed `IrSourceUnverifiable` for a v2 bundle.
///
/// MCP caveat (fail-closed): re-lowering here uses an **empty** MCP
/// contract cache, whereas `tau build` resolves MCP contracts (live or
/// pinned) and embeds the expanded server-tool nodes. A v2 bundle built
/// from a project that uses MCP tools therefore re-lowers to a *different*
/// hash and is conservatively rejected with `IrSourceDivergence` — it
/// cannot currently be run via `--bundle`. This is the same limitation
/// `tau verify --bundle` has (see `cmd::verify::run_reproducibility_check`)
/// and is safe (fail-closed, never fail-open). Wiring the pinned MCP cache
/// (`build::resolve_mcp_cache(cwd, /*offline=*/true)`) into this re-lowering
/// so honest MCP bundles verify is a tracked follow-up.
fn verify_bundle_against_source(
    cwd: &std::path::Path,
    bundle_path: &std::path::Path,
) -> Result<tau_pkg::bundle::VerifyReport, tau_pkg::bundle::VerifyError> {
    // Empty MCP cache — see the "MCP caveat" in this fn's doc-comment.
    let empty_mcp_cache = std::collections::BTreeMap::new();
    let recomputed_ir_hash = crate::cmd::build::lower_ir(
        cwd,
        &tau_ports::target::TargetTriple::host(),
        &empty_mcp_cache,
        None,
    )
    .map(|p| p.canonical_ir_hash);

    tau_pkg::bundle::verify_bundle(tau_pkg::bundle::VerifyOptions {
        bundle_path: bundle_path.to_path_buf(),
        project_root: cwd.to_path_buf(),
        recomputed_ir_hash,
    })
}

/// Maps a [`tau_pkg::bundle::VerifyError`] to its CLI exit code per
/// spec §C.3: bad-input/config/parse → 2, integrity/install-state → 3,
/// internal/IO → 70.
fn bundle_verify_exit_code(e: &tau_pkg::bundle::VerifyError) -> i32 {
    use tau_pkg::bundle::VerifyError as V;
    match e {
        V::BundleRead { .. }
        | V::BundleParse { .. }
        | V::ProjectTomlRead { .. }
        | V::UnsupportedSchemaVersion { .. } => 2,
        V::SelfHashMismatch { .. }
        | V::TargetMismatch { .. }
        | V::TauTomlDrift { .. }
        | V::PackageMissing { .. }
        | V::PackageDrift { .. }
        | V::AgentPromptDrift { .. }
        | V::AgentSetMismatch { .. }
        | V::IrPayloadDrift { .. }
        | V::IrSourceDivergence { .. }
        | V::IrSourceUnverifiable => 3,
        V::PackageTreeHash { .. } | V::AgentPromptResolve { .. } => 70,
    }
}

/// Mint a per-invocation run id.
///
/// A ULID (Crockford base32, 26 chars) — lexicographically sortable and
/// collision-resistant. Matches `run_main`'s workflow-run-id minting
/// (`ulid::Ulid::new()`) and replaces the old bespoke `tau-run-<nanos>`
/// string, which could collide under fast successive runs or a backward
/// clock adjustment (D8). The only contract is uniqueness within a host
/// process; the kernel still mints its own `AgentInstanceId`.
pub(super) fn mint_run_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Project a [`MessagePayload`] to a single text string for display.
/// Non-text payloads degrade to a `Debug`-formatted preview.
pub(super) fn format_message_text(payload: &MessagePayload) -> String {
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
    fn bundle_verify_failure_carries_renderer_output_and_mapped_code() {
        use tau_pkg::bundle::VerifyError;
        // integrity/install-state → 3
        let drift = VerifyError::SelfHashMismatch {
            claimed: "a".into(),
            computed: "b".into(),
        };
        let f = super::bundle_verify_failure(&drift);
        assert_eq!(f.code, 3);
        assert!(
            f.rendered.contains("bundle verification failed"),
            "got: {}",
            f.rendered
        );
        // bad-input/parse → 2
        let parse = VerifyError::UnsupportedSchemaVersion {
            found: 99,
            supported: 2,
        };
        assert_eq!(super::bundle_verify_failure(&parse).code, 2);
    }

    #[test]
    fn mint_run_id_is_a_unique_ulid_across_fast_successive_calls() {
        let a = super::mint_run_id();
        let b = super::mint_run_id();
        // 26-char Crockford base32 ULID.
        assert_eq!(a.len(), 26, "ulid is 26 chars, got {a:?}");
        assert!(
            a.chars().all(|c| c.is_ascii_alphanumeric()),
            "ulid is alphanumeric, got {a:?}"
        );
        // Unique even back-to-back (the old tau-run-<nanos> scheme could
        // collide under same-nanosecond calls / clock adjustment).
        assert_ne!(a, b, "two run-ids minted back-to-back must differ");
        assert!(ulid::Ulid::from_string(&a).is_ok(), "must parse as ULID");
    }

    #[test]
    fn agent_failed_renders_error_message() {
        let err: anyhow::Error = AgentFailed.into();
        let s = format!("{err}");
        assert!(s.contains("agent failed"), "got: {s}");
    }
}

#[cfg(test)]
mod bundle_source_xcheck_tests {
    use super::verify_bundle_against_source;
    use std::collections::BTreeMap;
    use tau_pkg::bundle::{build, BuildOptions, IrPayload, VerifyError};
    use tau_ports::target::TargetTriple;

    /// Write a native-tool project whose single tool declares `caps`
    /// (a TOML capabilities array, e.g. `[]` or `[{ kind = "net.http" }]`).
    /// The capability set is what makes two otherwise-identical projects
    /// lower to different IR hashes.
    fn write_project(root: &std::path::Path, name: &str, caps: &str) {
        std::fs::write(
            root.join("tau.toml"),
            format!(
                r#"
[project]
name = "{name}"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "{name}@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"

[tools.read_temp]
native = "ReadTemp"
description = "reads the temperature"
capabilities = {caps}
"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("tau.lock"),
            "schema_version = 6\ngenerated_by_tau_version = \"0.1.0\"\ngenerated_at = \"2024-01-01T00:00:00Z\"\n",
        )
        .unwrap();
    }

    fn lower(root: &std::path::Path) -> IrPayload {
        let empty = BTreeMap::new();
        crate::cmd::build::lower_ir(root, &TargetTriple::host(), &empty, None)
            .expect("native-tool project must lower to Some(IrPayload)")
    }

    /// Build a bundle in `src_root` but embed `ir_payload`. When
    /// `ir_payload` comes from a *different* source tree, the bundle's
    /// recorded tau.toml hash still matches `src_root` (so verify steps
    /// 6/8 pass) while its IR diverges — exactly the S3 attack shape.
    fn build_bundle(
        src_root: &std::path::Path,
        ir_payload: Option<IrPayload>,
    ) -> std::path::PathBuf {
        build(BuildOptions {
            project_root: src_root.to_path_buf(),
            target: TargetTriple::host(),
            output_path: None,
            agent_filter: None,
            ir_payload,
        })
        .expect("build must succeed")
        .path
    }

    #[test]
    fn run_bundle_rejects_ir_lowered_from_a_different_source() {
        // A: the source the user ships + inspects (no extra caps).
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        // B: a divergent source the attacker lowered the IR from.
        let b = tempfile::tempdir().unwrap();
        write_project(b.path(), "proj", "[{ kind = \"net.http\" }]");

        let ir_b = lower(b.path());
        // Bundle records A's tau.toml hash, but carries B's IR.
        let bundle = build_bundle(a.path(), Some(ir_b.clone()));

        let err = verify_bundle_against_source(a.path(), &bundle).unwrap_err();
        match err {
            VerifyError::IrSourceDivergence {
                bundle_hash,
                source_hash,
            } => {
                assert_eq!(bundle_hash, ir_b.canonical_ir_hash, "bundle carries B's IR");
                assert_ne!(source_hash, bundle_hash, "A's re-lowered hash must differ");
            }
            other => panic!("expected IrSourceDivergence, got {other:?}"),
        }
    }

    #[test]
    fn run_bundle_accepts_genuine_ir() {
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        let ir_a = lower(a.path());
        let bundle = build_bundle(a.path(), Some(ir_a));
        verify_bundle_against_source(a.path(), &bundle).expect("genuine bundle must verify");
    }

    #[test]
    fn run_bundle_v1_unaffected() {
        // A v1 bundle (no ir_payload) verifies regardless of re-lowering.
        let a = tempfile::tempdir().unwrap();
        write_project(a.path(), "proj", "[]");
        let bundle = build_bundle(a.path(), None);
        verify_bundle_against_source(a.path(), &bundle).expect("v1 bundle must verify");
    }
}
