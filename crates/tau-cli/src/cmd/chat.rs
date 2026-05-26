//! `tau chat` — interactive REPL with an agent.
//!
//! Per spec §3.13 (chat row) and brainstorm Section 8.
//!
//! Mirrors `cmd::run`'s setup (project tau.toml → resolved
//! `(AgentDefinition, PackageManifest)` → spawned plugin processes →
//! [`tau_runtime::Runtime`]), then enters a rustyline-driven REPL. Each user prompt
//! is sent through [`tau_runtime::Runtime::run_with_history`] so the conversation
//! accumulates across turns; tool dispatch and capability filtering
//! are inherited from the kernel, not re-implemented here.
//!
//! # Plugin lifecycle
//!
//! Per spec §11: plugin processes spawn once at REPL entry and stay
//! alive for the duration of the session (multiplexed long-lived
//! lifecycle). On `/exit` (or EOF / Ctrl-D), the [`tau_runtime::Runtime`] is
//! dropped, which drops the `Arc<dyn DynLlmBackend>` and per-tool
//! `Arc<dyn DynTool>`, which drops the underlying `PluginProcess` —
//! and `kill_on_drop` ensures the plugin subprocess exits cleanly.
//!
//! # Slash commands
//!
//! See [`SlashCommand`] for the recognised set. Parsing is intentionally
//! strict: only the four documented commands are slash-handled, and
//! anything starting with `/` that isn't a known command is forwarded
//! to the LLM as a normal prompt. This keeps the surface predictable
//! and lets users send messages that happen to start with `/`.
//!
//! # Failure handling
//!
//! - `Ok(RunOutcome::Failed { .. })`: the kernel reported a typed
//!   agent-level failure (e.g. `OutOfResources`). The REPL renders the
//!   detail in red, accumulates partial history + tokens, and continues
//!   — the user can ask a follow-up. (No process exit on agent failure
//!   here, unlike `tau run`; chat is interactive.)
//! - `Err(RuntimeError)`: kernel/operational error. The REPL prints the
//!   error and aborts with the marker bubbling up to `run_main`, which
//!   maps it to [`crate::ExitCode::Error`] (2).
//!
//! # JSON
//!
//! `--json` is rejected at handler entry: a streaming, terminal-driven
//! REPL has no useful JSON projection. (See spec §3.6.)

use std::io::{self, Write as _};
use std::path::PathBuf;

use anyhow::Context;
use futures_core::stream::Stream as _;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use tau_domain::{Address, AgentInstanceId, Message, MessagePayload};
use tau_plugin_protocol::handshake::TraceContext;
use tau_runtime::stream::RunEvent;
use tau_runtime::{RunOptions, RunOutcome, TokenUsage};

use crate::cli::ChatArgs;
use crate::cmd::plugin_loader;
use crate::config::ProjectConfig;
use crate::output::Output;

/// What a single line of REPL input represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplInput {
    /// A recognised slash command.
    Slash(SlashCommand),
    /// Free-form text to forward to the agent. Whitespace-preserved.
    Prompt(String),
}

/// Slash commands recognised in the REPL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommand {
    /// End the session and print the token-usage summary.
    Exit,
    /// Print the slash-command cheat sheet.
    Help,
    /// Deprecated: previously dropped in-memory conversation history.
    Clear,
    /// Render the current conversation history.
    History,
    /// Print information about the current session.
    Info,
}

/// Parse a single line of REPL input.
///
/// Trims the line for the slash-command match (so `"  /exit "` still
/// counts) but, for `Prompt`, returns the original line unchanged —
/// trailing whitespace can be load-bearing for some prompts.
///
/// Unknown slash forms (`/foo`) fall through to `Prompt`, intentionally:
/// see the module docs for the rationale.
pub fn parse_input(line: &str) -> ReplInput {
    let trimmed = line.trim();
    match trimmed {
        "/exit" => ReplInput::Slash(SlashCommand::Exit),
        "/help" => ReplInput::Slash(SlashCommand::Help),
        "/clear" => ReplInput::Slash(SlashCommand::Clear),
        "/history" => ReplInput::Slash(SlashCommand::History),
        "/info" => ReplInput::Slash(SlashCommand::Info),
        _ => ReplInput::Prompt(line.to_string()),
    }
}

/// Run `tau chat`.
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
    args: &ChatArgs,
    record_protocol: Option<PathBuf>,
    force_passthrough: bool,
    force_adapter_kind: Option<tau_runtime::sandbox::registry::RegistryKind>,
    output: &mut Output,
) -> anyhow::Result<()> {
    if output.is_json() {
        anyhow::bail!("tau chat does not support --json (REPL is interactive)");
    }

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
        // Dry-run skips plugin spawn entirely (per spec §3.9: no
        // process side-effects for a preview).
        emit_dry_run_preview(entry, &agent_def, &manifest, &options, output)?;
        return Ok(());
    }

    // ---- Plugin loading -----------------------------------------------------
    //
    // Spawn the LLM backend + tool plugins once for the entire REPL.
    // The runtime owns the Arc<dyn Dyn*> shims; on /exit (or EOF), the
    // runtime is dropped, which drops the shims, which drops the
    // PluginProcess (kill_on_drop ensures the child exits).

    let run_id = format!(
        "tau-chat-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
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

    // ---- Resume vs fresh-session branch ------------------------------------
    let result = if let Some(id_or_prefix) = args.resume.as_deref() {
        let sessions_dir = scope.state_path().join("sessions");
        let id = crate::session::resolve_id_prefix(&sessions_dir, id_or_prefix)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let path = sessions_dir.join(format!("{}.jsonl", id.as_str()));
        let (header, entries) =
            crate::session::SessionReader::read(&path).map_err(|e| anyhow::anyhow!("{e}"))?;

        // Drift validation (skipped if --force).
        if !args.force {
            check_session_drift(&id, &header, &agent_def, &manifest)?;
        } else {
            warn_drift_if_any(&header, &agent_def, &manifest, output);
        }

        // Seed history from entries.
        let history: Vec<Message> = entries
            .iter()
            .filter_map(|e| match e {
                crate::session::SessionEntry::Message(m) => Some(m.clone()),
                _ => None,
            })
            .collect();

        // Count turns from TurnSummary entries.
        let turn_count = entries
            .iter()
            .filter(|e| matches!(e, crate::session::SessionEntry::TurnSummary { .. }))
            .count();

        // Reopen the file in append mode.
        let writer = crate::session::SessionWriter::open_append(&path)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Print the resume banner.
        output.status(format!(
            "Resumed session {} ({} turns)",
            id.short(),
            turn_count
        ))?;

        run_repl(
            entry,
            &agent_def,
            &manifest,
            &options,
            args.no_stream,
            id,
            Some(writer),
            history,
            turn_count as u32,
            &runtime,
            output,
        )
        .await
    } else {
        // Fresh session: mint an id and create the writer now (before
        // entering the REPL) so the call site mirrors the resume path.
        let fresh_id = crate::session::mint();
        let fresh_writer: Option<crate::session::SessionWriter> = if args.ephemeral {
            output.status("Ephemeral session — not saved to disk")?;
            None
        } else {
            let sessions_dir = scope.state_path().join("sessions");
            let header = crate::session::SessionHeader::new(
                &fresh_id,
                entry.id.clone(),
                crate::session::SessionPackage {
                    name: manifest.name().to_string(),
                    version: manifest.version().to_string(),
                    resolved_commit: String::new(),
                },
                agent_def.llm_backend.to_string(),
            );
            let writer = crate::session::SessionWriter::create(&sessions_dir, &fresh_id, &header)?;
            output.status(format!("Session: {} (will be saved)", fresh_id.short()))?;
            Some(writer)
        };

        run_repl(
            entry,
            &agent_def,
            &manifest,
            &options,
            args.no_stream,
            fresh_id,
            fresh_writer,
            Vec::new(),
            0,
            &runtime,
            output,
        )
        .await
    };

    // Drop the runtime before flushing recorders so every plugin
    // process is reaped and the host-side write task is quiescent.
    drop(runtime);
    plugin_loader::flush_recorders().await;

    result
}

/// REPL inner loop, factored out of [`run`] so the caller can wrap it
/// with `--record-protocol` flush coordination after the runtime is
/// dropped. Borrows the runtime so the chat session and the recording
/// flush share a single ownership chain in `run`.
///
/// `session_id` is either freshly-minted (new session) or the resolved
/// id from `--resume`. `session_writer` is `Some(writer)` when the
/// session should be persisted (pre-created or opened-for-append by the
/// caller) and `None` for ephemeral sessions. `initial_history` is empty
/// for fresh sessions and pre-populated on resume. `initial_turn_counter`
/// is 0 for fresh sessions and the turn count read from the session file
/// on resume.
#[allow(clippy::too_many_arguments)]
async fn run_repl(
    entry: &crate::config::AgentEntry,
    agent_def: &tau_domain::AgentDefinition,
    manifest: &tau_domain::PackageManifest,
    options: &RunOptions,
    no_stream: bool,
    session_id: crate::session::SessionId,
    session_writer_arg: Option<crate::session::SessionWriter>,
    initial_history: Vec<Message>,
    initial_turn_counter: u32,
    runtime: &tau_runtime::Runtime,
    output: &mut Output,
) -> anyhow::Result<()> {
    let is_resumed = !initial_history.is_empty();

    // Welcome banner.
    if is_resumed {
        output.status(format!(
            "Welcome back to tau chat with agent '{}' ({}@{}). Type /help for commands, /exit or Ctrl-D to quit.",
            entry.id,
            manifest.name(),
            manifest.version()
        ))?;
    } else {
        output.status(format!(
            "Welcome to tau chat with agent '{}' ({}@{}). Type /help for commands, /exit or Ctrl-D to quit.",
            entry.id,
            manifest.name(),
            manifest.version()
        ))?;
    }

    // The caller is responsible for writer creation/opening and
    // printing the appropriate status message (Session: ... or Ephemeral).
    let mut session_writer: Option<crate::session::SessionWriter> = session_writer_arg;

    let session_started_at = std::time::SystemTime::now();
    let mut turn_counter: u32 = initial_turn_counter;

    let mut editor = DefaultEditor::new().context("initialising rustyline editor")?;
    let mut history: Vec<Message> = initial_history;
    let mut total_tokens = TokenUsage::default();

    loop {
        let line = match editor.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => {
                close_session_and_summary(
                    session_writer.take(),
                    &session_id,
                    &entry.id,
                    total_tokens,
                    output,
                )?;
                return Ok(());
            }
            Err(e) => {
                return Err(anyhow::Error::from(e).context("readline failure"));
            }
        };

        // Best-effort: rustyline returns Err if the history is full. We
        // don't surface that — it's pure input ergonomics, not state.
        let _ = editor.add_history_entry(line.as_str());

        match parse_input(&line) {
            ReplInput::Slash(SlashCommand::Exit) => {
                close_session_and_summary(
                    session_writer.take(),
                    &session_id,
                    &entry.id,
                    total_tokens,
                    output,
                )?;
                return Ok(());
            }
            ReplInput::Slash(SlashCommand::Help) => {
                print_help(output)?;
            }
            ReplInput::Slash(SlashCommand::Clear) => {
                // /clear was removed — in-memory history clear is incoherent
                // with persistence (clearing in-memory leaves the file intact,
                // so resume would surface the cleared messages). Print a
                // deprecation message and continue.
                output.status(
                    "/clear was removed. Exit (/exit) and re-run `tau chat <agent>` for a fresh session.",
                )?;
            }
            ReplInput::Slash(SlashCommand::History) => {
                render_history(&history, output)?;
            }
            ReplInput::Slash(SlashCommand::Info) => {
                let path_label = session_writer
                    .as_ref()
                    .map(|w| w.path().display().to_string())
                    .unwrap_or_else(|| "(ephemeral; not saved)".to_string());
                let started = humantime::format_rfc3339_seconds(session_started_at).to_string();
                output.status(format!(
                    "Session: {}\nFile: {}\nTurns: {}\nStarted: {}\nAgent: {} ({}@{})",
                    session_id.as_str(),
                    path_label,
                    turn_counter,
                    started,
                    entry.id,
                    manifest.name(),
                    manifest.version()
                ))?;
            }
            ReplInput::Prompt(text) if text.trim().is_empty() => {
                // Empty / whitespace-only input — no-op. Skip the round
                // trip rather than send an empty prompt.
                continue;
            }
            ReplInput::Prompt(text) => {
                let prev_history_len = history.len();
                let initial = Message::new(
                    Address::User,
                    // The kernel mints its own per-run AgentInstanceId;
                    // this recipient placeholder is overwritten by the
                    // loop. Fresh id keeps the typed Address::Agent
                    // variant honest.
                    Address::Agent(AgentInstanceId::new()),
                    MessagePayload::Text { content: text },
                );

                if no_stream {
                    // ---- Batch (non-streaming) path ----
                    let outcome = runtime
                        .run_with_history(
                            agent_def.clone(),
                            manifest.clone(),
                            history.clone(),
                            initial,
                            options.clone(),
                        )
                        .await;

                    match outcome {
                        Ok(RunOutcome::Completed {
                            final_message,
                            all_messages,
                            token_usage,
                            ..
                        }) => {
                            render_final_message(&final_message, output)?;
                            accumulate_tokens(&mut total_tokens, &token_usage);
                            // Persist new messages before updating history.
                            if let Some(writer) = session_writer.as_mut() {
                                let new_messages = &all_messages[prev_history_len..];
                                if let Err(e) = writer.append_messages(new_messages) {
                                    tracing::warn!(
                                        name = "session.write_failed",
                                        error = %e,
                                        "session write failed; continuing"
                                    );
                                } else {
                                    turn_counter += 1;
                                    let stop_str = "Completed".to_string();
                                    let _ = writer.append_turn_summary(
                                        turn_counter,
                                        &stop_str,
                                        Some(token_usage.input_tokens),
                                        Some(token_usage.output_tokens),
                                    );
                                }
                            }
                            history = all_messages;
                        }
                        Ok(RunOutcome::Failed {
                            status,
                            all_messages,
                            token_usage,
                            ..
                        }) => {
                            // Agent-level failure: render in red and let the
                            // user retry. (Distinct from a kernel error,
                            // which aborts the REPL below.)
                            output.error(format!("agent failed: {status:?}"))?;
                            accumulate_tokens(&mut total_tokens, &token_usage);
                            // Persist partial messages even on failure.
                            if let Some(writer) = session_writer.as_mut() {
                                let new_messages = &all_messages[prev_history_len..];
                                if let Err(e) = writer.append_messages(new_messages) {
                                    tracing::warn!(
                                        name = "session.write_failed",
                                        error = %e,
                                        "session write failed; continuing"
                                    );
                                } else {
                                    turn_counter += 1;
                                    let stop_str = format!("Failed({status:?})");
                                    let _ = writer.append_turn_summary(
                                        turn_counter,
                                        &stop_str,
                                        Some(token_usage.input_tokens),
                                        Some(token_usage.output_tokens),
                                    );
                                }
                            }
                            history = all_messages;
                        }
                        Ok(_) => {
                            return Err(anyhow::anyhow!("unknown RunOutcome variant"));
                        }
                        Err(e) => {
                            // Kernel/operational error: print and abort the
                            // REPL with a kernel-error exit. Token usage so
                            // far is still surfaced.
                            output.error(format!("kernel error: {e}"))?;
                            return Err(anyhow::Error::from(e));
                        }
                    }
                } else {
                    // ---- Streaming path (default) ----
                    //
                    // Two-pass rendering per spec §4.5:
                    //   Pass 1 (during streaming): text deltas via raw
                    //     print!/flush (typewriter UX); tool annotations
                    //     to stderr so stdout stays the agent's text.
                    //   Pass 2 (on RunCompleted): re-render via termimad
                    //     (render_final_message) so the user sees final
                    //     formatted markdown after the rough-draft pass.
                    let stream_result = runtime
                        .run_streaming_with_history(
                            agent_def.clone(),
                            manifest.clone(),
                            history.clone(),
                            initial,
                            options.clone(),
                        )
                        .await;

                    let stream = match stream_result {
                        Ok(s) => s,
                        Err(e) => {
                            output.error(format!("kernel error: {e}"))?;
                            return Err(anyhow::Error::from(e));
                        }
                    };
                    let mut stream = Box::pin(stream);
                    let mut stdout = io::stdout();

                    loop {
                        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
                        match next {
                            Some(RunEvent::TextDelta { delta }) => {
                                // Pass 1: typewriter output — raw print to
                                // stdout, bypassing Output's writer, so the
                                // text appears inline without a trailing newline
                                // until the full message is assembled.
                                print!("{delta}");
                                let _ = stdout.flush();
                            }
                            Some(RunEvent::ToolCallStarted { name, .. }) => {
                                // Route tool annotations to stderr so stdout
                                // remains the agent's clean text stream.
                                eprintln!("-> calling {name}...");
                            }
                            Some(RunEvent::ToolCallCompleted { name, result, .. }) => {
                                match result {
                                    Ok(_) => eprintln!("✓ {name} completed"),
                                    Err(reason) => eprintln!("✗ {name} failed: {reason}"),
                                }
                            }
                            Some(RunEvent::TurnCompleted { .. }) => {
                                // Nothing during streaming; re-render on
                                // RunCompleted below.
                            }
                            Some(RunEvent::FatalError { kind, detail, .. }) => {
                                // Kernel-level fatal error: mirror the batch
                                // path's Err(RuntimeError) handling — print
                                // and abort the REPL.
                                println!(); // end the typewriter line
                                output.error(format!("kernel error ({kind}): {detail}"))?;
                                return Err(anyhow::anyhow!(
                                    "streaming fatal error ({kind}): {detail}"
                                ));
                            }
                            Some(RunEvent::RunCompleted { outcome }) => {
                                // Pass 2: close typewriter line, then
                                // re-render the final message with termimad
                                // markdown formatting.
                                println!();
                                match outcome {
                                    RunOutcome::Completed {
                                        final_message,
                                        all_messages,
                                        token_usage,
                                        ..
                                    } => {
                                        render_final_message(&final_message, output)?;
                                        accumulate_tokens(&mut total_tokens, &token_usage);
                                        // Persist new messages before updating history.
                                        if let Some(writer) = session_writer.as_mut() {
                                            let new_messages = &all_messages[prev_history_len..];
                                            if let Err(e) = writer.append_messages(new_messages) {
                                                tracing::warn!(
                                                    name = "session.write_failed",
                                                    error = %e,
                                                    "session write failed; continuing"
                                                );
                                            } else {
                                                turn_counter += 1;
                                                let stop_str = "Completed".to_string();
                                                let _ = writer.append_turn_summary(
                                                    turn_counter,
                                                    &stop_str,
                                                    Some(token_usage.input_tokens),
                                                    Some(token_usage.output_tokens),
                                                );
                                            }
                                        }
                                        history = all_messages;
                                    }
                                    RunOutcome::Failed {
                                        status,
                                        all_messages,
                                        token_usage,
                                        ..
                                    } => {
                                        output.error(format!("agent failed: {status:?}"))?;
                                        accumulate_tokens(&mut total_tokens, &token_usage);
                                        // Persist partial messages even on failure.
                                        if let Some(writer) = session_writer.as_mut() {
                                            let new_messages = &all_messages[prev_history_len..];
                                            if let Err(e) = writer.append_messages(new_messages) {
                                                tracing::warn!(
                                                    name = "session.write_failed",
                                                    error = %e,
                                                    "session write failed; continuing"
                                                );
                                            } else {
                                                turn_counter += 1;
                                                let stop_str = format!("Failed({status:?})");
                                                let _ = writer.append_turn_summary(
                                                    turn_counter,
                                                    &stop_str,
                                                    Some(token_usage.input_tokens),
                                                    Some(token_usage.output_tokens),
                                                );
                                            }
                                        }
                                        history = all_messages;
                                    }
                                    _ => {
                                        return Err(anyhow::anyhow!(
                                            "unknown RunOutcome variant in streaming path"
                                        ));
                                    }
                                }
                                break;
                            }
                            None => {
                                // Stream exhausted without RunCompleted — should
                                // not happen per stream invariants, but handle
                                // defensively.
                                println!();
                                break;
                            }
                            // RunEvent is #[non_exhaustive]; ignore unknown
                            // future variants so forward-compatibility is
                            // preserved.
                            Some(_) => {}
                        }
                    }
                }
            }
        }
    }
}

/// Close the session writer (if any) and emit the end-of-session summary.
///
/// Prints a resume hint for non-ephemeral sessions or "Session discarded."
/// for ephemeral ones. Called from both exit paths (`/exit` and EOF/Ctrl-D).
fn close_session_and_summary(
    session_writer: Option<crate::session::SessionWriter>,
    session_id: &crate::session::SessionId,
    agent_id: &str,
    total_tokens: TokenUsage,
    output: &mut Output,
) -> anyhow::Result<()> {
    emit_session_summary(total_tokens, output)?;
    if let Some(writer) = session_writer {
        writer.close()?;
        output.status(format!(
            "Session saved. Resume with: tau chat {} --resume {}",
            agent_id,
            session_id.short()
        ))?;
    } else {
        output.status("Session discarded.")?;
    }
    Ok(())
}

/// Render the final message of a completed turn. Text payloads go
/// through termimad for markdown rendering; non-text payloads degrade
/// to a `Debug` preview through the standard human channel.
fn render_final_message(msg: &Message, output: &mut Output) -> anyhow::Result<()> {
    match &msg.payload {
        MessagePayload::Text { content } => {
            // termimad writes directly to stdout via crossterm; this
            // bypasses `Output`'s writer abstraction, but the REPL only
            // runs in non-JSON mode (rejected at entry) and stdout is
            // exactly where the agent's text response belongs.
            let skin = termimad::MadSkin::default_dark();
            skin.print_text(content);
            Ok(())
        }
        other => output
            .human(&format!("{other:?}"))
            .context("writing non-text final message"),
    }
}

/// Accumulate per-turn token usage into the session-wide running total.
fn accumulate_tokens(total: &mut TokenUsage, delta: &TokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    // `total_tokens` is the backend-reported unified count when present;
    // sum it when both sides are Some, otherwise leave the existing
    // value. Most backends report only input/output, so this is largely
    // a courtesy.
    total.total_tokens = match (total.total_tokens, delta.total_tokens) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
}

/// Render a one-line summary of every message in the session history.
fn render_history(history: &[Message], output: &mut Output) -> anyhow::Result<()> {
    if history.is_empty() {
        output.human("(no history yet)")?;
        return Ok(());
    }
    for (i, msg) in history.iter().enumerate() {
        let preview = match &msg.payload {
            MessagePayload::Text { content } => {
                let p: String = content.chars().take(80).collect();
                if content.chars().count() > 80 {
                    format!("{p}...")
                } else {
                    p
                }
            }
            other => format!("{other:?}"),
        };
        output.human(&format!(
            "[{i}] {:?} -> {:?}: {preview}",
            msg.sender, msg.recipient
        ))?;
    }
    Ok(())
}

/// Print the slash-command cheat sheet to stdout (so `/help` shows up
/// in piped runs without a `--quiet`-able status channel suppressing it).
fn print_help(output: &mut Output) -> anyhow::Result<()> {
    output.human("Slash commands:")?;
    output.human("  /exit      End the session.")?;
    output.human("  /help      Show this help.")?;
    output.human("  /info      Show session info (id, file, turns, agent).")?;
    output.human("  /history   Show conversation history.")?;
    Ok(())
}

/// Emit the end-of-session token-usage summary to stderr.
fn emit_session_summary(tokens: TokenUsage, output: &mut Output) -> anyhow::Result<()> {
    output.status(format!(
        "session ended; total tokens: {} in / {} out",
        tokens.input_tokens, tokens.output_tokens
    ))?;
    Ok(())
}

/// Render the dry-run preview to stderr. Mirrors `cmd::run`'s preview,
/// minus the `initial message:` row (no prompt yet — we'd be entering
/// the REPL).
fn emit_dry_run_preview(
    entry: &crate::config::AgentEntry,
    agent_def: &tau_domain::AgentDefinition,
    manifest: &tau_domain::PackageManifest,
    options: &RunOptions,
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
    output.dry_run("REPL would start with the above setup.")?;
    output.dry_run("no session opened.")?;
    Ok(())
}

/// Validate that the session header's recorded metadata still matches the
/// current project state. Returns `Err` describing the first mismatch found.
/// Called on `--resume` when `--force` is not set.
fn check_session_drift(
    session_id: &crate::session::SessionId,
    header: &crate::session::SessionHeader,
    agent_def: &tau_domain::AgentDefinition,
    manifest: &tau_domain::PackageManifest,
) -> anyhow::Result<()> {
    let current_agent_id = agent_def.id.to_string();
    let current_pkg_name = manifest.name().to_string();
    let current_pkg_version = manifest.version().to_string();
    let current_llm = agent_def.llm_backend.to_string();

    if header.agent_id != current_agent_id {
        return Err(anyhow::anyhow!(
            "session {} drift: agent_id was {:?}, now {:?}. Use --force to resume anyway.",
            session_id.short(),
            header.agent_id,
            current_agent_id
        ));
    }
    if header.package.name != current_pkg_name {
        return Err(anyhow::anyhow!(
            "session {} drift: package.name was {:?}, now {:?}. Use --force to resume anyway.",
            session_id.short(),
            header.package.name,
            current_pkg_name
        ));
    }
    if header.package.version != current_pkg_version {
        return Err(anyhow::anyhow!(
            "session {} drift: package.version was {:?}, now {:?}. Use --force to resume anyway.",
            session_id.short(),
            header.package.version,
            current_pkg_version
        ));
    }
    if header.llm_backend != current_llm {
        return Err(anyhow::anyhow!(
            "session {} drift: llm_backend was {:?}, now {:?}. Use --force to resume anyway.",
            session_id.short(),
            header.llm_backend,
            current_llm
        ));
    }
    Ok(())
}

/// Emit warnings for any drift detected but do not abort. Called on
/// `--resume --force`.
fn warn_drift_if_any(
    header: &crate::session::SessionHeader,
    agent_def: &tau_domain::AgentDefinition,
    manifest: &tau_domain::PackageManifest,
    output: &mut Output,
) {
    let current_pkg_name = manifest.name().to_string();
    let current_pkg_version = manifest.version().to_string();
    let current_llm = agent_def.llm_backend.to_string();
    let current_agent = agent_def.id.to_string();

    if header.package.version != current_pkg_version || header.package.name != current_pkg_name {
        let _ = output.status(format!(
            "warning: resuming with {}@{} (session was {}@{})",
            current_pkg_name, current_pkg_version, header.package.name, header.package.version
        ));
    }
    if header.llm_backend != current_llm {
        let _ = output.status(format!(
            "warning: resuming with llm_backend {} (session was {})",
            current_llm, header.llm_backend
        ));
    }
    if header.agent_id != current_agent {
        let _ = output.status(format!(
            "warning: resuming with agent_id {} (session was {})",
            current_agent, header.agent_id
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exit() {
        assert_eq!(parse_input("/exit"), ReplInput::Slash(SlashCommand::Exit));
    }

    #[test]
    fn parse_help() {
        assert_eq!(parse_input("/help"), ReplInput::Slash(SlashCommand::Help));
    }

    #[test]
    fn parse_clear() {
        assert_eq!(parse_input("/clear"), ReplInput::Slash(SlashCommand::Clear));
    }

    #[test]
    fn parse_history() {
        assert_eq!(
            parse_input("/history"),
            ReplInput::Slash(SlashCommand::History)
        );
    }

    #[test]
    fn parse_unknown_slash_is_prompt() {
        // Unknown slash commands fall through to `Prompt` so the user
        // can send messages that happen to start with `/` without
        // surprise rejections.
        assert_eq!(parse_input("/foo"), ReplInput::Prompt("/foo".to_string()));
    }

    #[test]
    fn parse_text_is_prompt() {
        assert_eq!(
            parse_input("hello world"),
            ReplInput::Prompt("hello world".to_string())
        );
    }

    #[test]
    fn parse_strips_whitespace_for_slash_match() {
        // Leading/trailing whitespace on a slash command shouldn't
        // defeat recognition — most terminals append a trailing newline
        // and copy/paste often introduces leading whitespace.
        assert_eq!(
            parse_input("  /exit  "),
            ReplInput::Slash(SlashCommand::Exit)
        );
    }

    #[test]
    fn parse_text_preserves_whitespace() {
        // For a text Prompt, preserve the original line — whitespace
        // can be load-bearing in agent prompts (code blocks, indentation).
        let result = parse_input("  hello  ");
        assert_eq!(result, ReplInput::Prompt("  hello  ".to_string()));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn accumulate_tokens_sums_input_and_output() {
        // `TokenUsage` is `#[non_exhaustive]`; struct-literal construction
        // is blocked from this crate. Use default() + field mutation.
        let mut total = TokenUsage::default();
        let mut delta = TokenUsage::default();
        delta.input_tokens = 10;
        delta.output_tokens = 5;
        accumulate_tokens(&mut total, &delta);
        assert_eq!(total.input_tokens, 10);
        assert_eq!(total.output_tokens, 5);
        assert_eq!(total.total_tokens, None);

        let mut delta2 = TokenUsage::default();
        delta2.input_tokens = 3;
        delta2.output_tokens = 7;
        delta2.total_tokens = Some(10);
        accumulate_tokens(&mut total, &delta2);
        assert_eq!(total.input_tokens, 13);
        assert_eq!(total.output_tokens, 12);
        // First delta had None; subsequent Some carries through.
        assert_eq!(total.total_tokens, Some(10));
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn accumulate_tokens_saturates_on_overflow() {
        let mut total = TokenUsage::default();
        total.input_tokens = u64::MAX;
        let mut delta = TokenUsage::default();
        delta.input_tokens = 1;
        accumulate_tokens(&mut total, &delta);
        // saturating_add prevents wrap; the running total stays pinned
        // at u64::MAX rather than rolling to 0.
        assert_eq!(total.input_tokens, u64::MAX);
    }
}
