//! Per-request executor for runtime.run and runtime.run_streaming.
//!
//! Wires JSON-RPC requests to tau_runtime::Runtime::run and
//! Runtime::run_streaming. Streaming emits one runtime.event
//! notification per RunEvent, correlated by the request id, then
//! a final result response.

use super::dispatch::Dispatcher;
use super::error_codes;
use super::error_map::from_runtime_error;
use super::methods;
use super::protocol::{Request, RequestId};
use futures::StreamExt;
use serde_json::{json, Value};
use tau_domain::{Address, Message, MessagePayload};
use tau_runtime::{RunEvent, RunOptions};

/// Execute a runtime.run or runtime.run_streaming request.
pub async fn execute(disp: Dispatcher, req: Request, streaming: bool) {
    // 1. Parse params.
    let params = match req.params.as_ref() {
        Some(v) => v,
        None => {
            disp.send_err(
                req.id,
                error_codes::INVALID_PARAMS,
                "params missing".into(),
                None,
            )
            .await;
            return;
        }
    };
    let agent_id = match params.get("agent").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            disp.send_err(
                req.id,
                error_codes::INVALID_PARAMS,
                "params.agent missing or not a string".into(),
                None,
            )
            .await;
            return;
        }
    };
    let prompt = match params.get("prompt").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            disp.send_err(
                req.id,
                error_codes::INVALID_PARAMS,
                "params.prompt missing or not a string".into(),
                None,
            )
            .await;
            return;
        }
    };

    // 2. Pre-check unknown agent (typed path; avoids brittle string matching).
    if !disp.project.config.agents.contains_key(&agent_id) {
        disp.send_err(
            req.id,
            error_codes::UNKNOWN_AGENT,
            format!("agent_id not found: {}", agent_id),
            Some(json!({"agent_id": agent_id})),
        )
        .await;
        return;
    }

    // 3. Resolve the agent (manifest + AgentDefinition).
    let (agent_def, manifest) = match disp.project.resolve(&agent_id) {
        Ok(pair) => pair,
        Err(e) => {
            // Resolve can fail for reasons OTHER than unknown-agent
            // (manifest invalid, package not installed at the requested version, etc.).
            disp.send_err(
                req.id,
                error_codes::RUNTIME_ERROR,
                format!("agent resolution failed: {}", e),
                Some(json!({"agent_id": agent_id})),
            )
            .await;
            return;
        }
    };

    // 4. Build initial Message. tau_domain::Message has no Message::user()
    // constructor outside tau-domain (non_exhaustive + struct-literal blocked).
    // Use Message::new with Address::User sender/recipient and a Text payload.
    // The runtime overwrites the recipient address internally.
    let initial = Message::new(
        Address::User,
        Address::User,
        MessagePayload::Text { content: prompt },
    );

    let opts = RunOptions::default();
    let cancel = disp.cancel_reg.register(req.id.clone());

    let result: Result<(), tau_runtime::RuntimeError> = if streaming {
        execute_streaming(
            &disp,
            req.id.clone(),
            agent_def,
            manifest,
            initial,
            opts,
            cancel,
        )
        .await
    } else {
        execute_batch(
            &disp,
            req.id.clone(),
            agent_def,
            manifest,
            initial,
            opts,
            cancel,
        )
        .await
    };

    disp.cancel_reg.forget(&req.id);

    if let Err(err) = result {
        let obj = from_runtime_error(&err);
        disp.send_err(req.id, obj.code, obj.message, obj.data).await;
    }
}

async fn execute_batch(
    disp: &Dispatcher,
    id: RequestId,
    agent_def: tau_domain::AgentDefinition,
    manifest: tau_domain::PackageManifest,
    initial: Message,
    opts: RunOptions,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), tau_runtime::RuntimeError> {
    use tokio::select;
    // Runtime::run signature (verified in Task 11 reconciliation):
    //   pub async fn run(&self, agent_def, package_manifest, initial_message, options)
    let fut = disp.runtime.run(agent_def, manifest, initial, opts);
    select! {
        outcome = fut => {
            let outcome = outcome?;
            // RunOutcome does not implement Serialize in tau-app's dep graph
            // (serde feature not enabled). Manually construct the JSON body.
            let body = outcome_to_json(outcome);
            disp.send_ok(id, body).await;
            Ok(())
        }
        _ = cancel.cancelled() => {
            disp.send_err(
                id,
                error_codes::CANCELLED,
                "Cancelled by client".into(),
                None,
            ).await;
            Ok(())
        }
    }
}

/// Manually serialize a `RunOutcome` to JSON. `RunOutcome` only derives
/// `Serialize` under the `serde` feature of `tau-domain`/`tau-runtime`,
/// which `tau-app` does not enable. We manually extract the fields we
/// expose in the serve-mode protocol response.
fn outcome_to_json(outcome: tau_runtime::RunOutcome) -> Value {
    match outcome {
        tau_runtime::RunOutcome::Completed {
            final_message,
            total_turns,
            token_usage,
            ..
        } => {
            let final_text = match &final_message.payload {
                MessagePayload::Text { content } => content.clone(),
                other => format!("{:?}", other),
            };
            json!({
                "status": "completed",
                "final_message": final_text,
                "total_turns": total_turns,
                "token_usage": token_usage_to_json(token_usage),
            })
        }
        tau_runtime::RunOutcome::Failed {
            status,
            total_turns,
            token_usage,
            ..
        } => {
            json!({
                "status": "failed",
                "agent_status": format!("{:?}", status),
                "total_turns": total_turns,
                "token_usage": token_usage_to_json(token_usage),
            })
        }
        // RunOutcome is #[non_exhaustive]; guard for future variants.
        _ => json!({"status": "unknown"}),
    }
}

fn token_usage_to_json(usage: tau_runtime::TokenUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens,
    })
}

async fn execute_streaming(
    disp: &Dispatcher,
    id: RequestId,
    agent_def: tau_domain::AgentDefinition,
    manifest: tau_domain::PackageManifest,
    initial: Message,
    opts: RunOptions,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), tau_runtime::RuntimeError> {
    use tokio::select;
    let stream = disp
        .runtime
        .run_streaming(agent_def, manifest, initial, opts)
        .await?;
    // run_streaming returns an `impl Stream + 'static` that is NOT Unpin.
    // Pin it to the stack so StreamExt::next() is callable in the select! loop.
    tokio::pin!(stream);

    let mut last_token_usage: Option<Value> = None;
    let mut stop_reason: Option<String> = None;

    loop {
        select! {
            biased;
            _ = cancel.cancelled() => {
                disp.send_err(
                    id,
                    error_codes::CANCELLED,
                    "Cancelled by client".into(),
                    None,
                ).await;
                return Ok(());
            }
            event = stream.next() => {
                match event {
                    None => break,
                    Some(ev) => emit_event(disp, &id, &ev, &mut last_token_usage, &mut stop_reason).await,
                }
            }
        }
    }

    let body = json!({
        "final": true,
        "token_usage": last_token_usage,
        "stop_reason": stop_reason,
    });
    disp.send_ok(id, body).await;
    Ok(())
}

async fn emit_event(
    disp: &Dispatcher,
    id: &RequestId,
    event: &RunEvent,
    last_token_usage: &mut Option<Value>,
    stop_reason: &mut Option<String>,
) {
    // Field names verified against crates/tau-runtime/src/stream.rs in
    // Task 11 reconciliation. Adjusted from plan template:
    //   TextDelta:         delta (not text)
    //   ToolCallStarted:   id, name, args (not tool/call_id)
    //   ToolCallCompleted: id, name, result: Result<ToolResult,String> (not tool/call_id)
    //   TurnCompleted:     stop_reason: StopReason, usage: Option<TokenUsage>, turn: u32
    //   RunCompleted:      outcome: RunOutcome (not token_usage directly)
    //   FatalError:        kind, detail, context_json, tool_error_variant
    //
    // StopReason, ToolResult, TokenUsage do NOT implement Serialize in tau-app's
    // dep graph (serde feature not enabled). Use Debug formatting / manual extraction.
    let (kind, data) = match event {
        RunEvent::TextDelta { delta } => ("TextDelta", json!({"text": delta})),
        RunEvent::ToolCallStarted {
            id: call_id,
            name,
            args,
        } => (
            "ToolCallStarted",
            json!({"tool": name, "args": args, "call_id": call_id}),
        ),
        RunEvent::ToolCallCompleted {
            id: call_id,
            name,
            result,
        } => {
            let result_json = match result {
                Ok(tool_result) => {
                    // ToolResult has no Serialize; extract content manually.
                    let content: Vec<Value> = tool_result
                        .content
                        .iter()
                        .map(|c| match c {
                            tau_ports::ToolContent::Text { text } => {
                                json!({"type": "text", "text": text})
                            }
                            tau_ports::ToolContent::Json { data } => {
                                json!({"type": "json", "data": data})
                            }
                            // ToolContent is #[non_exhaustive].
                            _ => json!({"type": "unknown"}),
                        })
                        .collect();
                    json!({"ok": true, "content": content, "is_error": tool_result.is_error})
                }
                Err(reason) => json!({"ok": false, "error": reason}),
            };
            (
                "ToolCallCompleted",
                json!({"tool": name, "call_id": call_id, "result": result_json}),
            )
        }
        RunEvent::TurnCompleted {
            stop_reason: sr,
            usage,
            turn,
        } => {
            let sr_str = format!("{:?}", sr);
            *stop_reason = Some(sr_str.clone());
            // TurnCompleted.usage is Option<tau_ports::TokenUsage> which has
            // only input_tokens and output_tokens (no total_tokens field).
            let usage_json = usage
                .as_ref()
                .map(|u| {
                    json!({
                        "input_tokens": u.input_tokens,
                        "output_tokens": u.output_tokens,
                    })
                })
                .unwrap_or(Value::Null);
            (
                "TurnCompleted",
                json!({"turn": turn, "stop_reason": sr_str, "usage": usage_json}),
            )
        }
        RunEvent::RunCompleted { outcome } => {
            // Extract token_usage from the outcome for the final summary.
            let tu = match outcome {
                tau_runtime::RunOutcome::Completed { token_usage, .. } => {
                    token_usage_to_json(*token_usage)
                }
                tau_runtime::RunOutcome::Failed { token_usage, .. } => {
                    token_usage_to_json(*token_usage)
                }
                _ => Value::Null,
            };
            *last_token_usage = Some(tu.clone());
            ("RunCompleted", json!({"token_usage": tu}))
        }
        RunEvent::FatalError {
            kind,
            detail,
            context_json,
            tool_error_variant,
        } => (
            "FatalError",
            json!({
                "kind": kind,
                "detail": detail,
                "context_json": context_json,
                "tool_error_variant": tool_error_variant,
            }),
        ),
        // RunEvent is #[non_exhaustive]; guard for future variants.
        _ => ("Unknown", json!({})),
    };
    disp.send_notification(
        methods::RUNTIME_EVENT,
        json!({"id": id, "kind": kind, "data": data}),
    )
    .await;
}
