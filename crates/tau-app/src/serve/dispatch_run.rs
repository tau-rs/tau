//! Per-request executor for runtime.run and runtime.run_streaming.
//!
//! Wires JSON-RPC requests to tau_runtime_tokio::Runtime::run and
//! Runtime::run_streaming. Streaming emits one runtime.event
//! notification per RunEvent, correlated by the request id, then
//! a final result response.

use super::dispatch::Dispatcher;
use super::error_codes;
use super::error_map::from_runtime_error;
use super::methods;
use super::project::ResolveError;
use super::protocol::{Request, RequestId};
use super::wire;
use futures::StreamExt;
use serde_json::{json, Value};
use tau_domain::{Address, Message, MessagePayload};
use tau_runtime_tokio::{RunEvent, RunOptions};

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

    // 2. Resolve the agent. Unknown agents and package/manifest failures both
    // surface here through one typed path (no string-prefix matching).
    let (agent_def, manifest) = match disp.project.resolve(&agent_id) {
        Ok(pair) => pair,
        Err(ResolveError::AgentNotFound { agent_id, .. }) => {
            disp.send_err(
                req.id,
                error_codes::UNKNOWN_AGENT,
                format!("agent_id not found: {}", agent_id),
                Some(json!({ "agent_id": agent_id })),
            )
            .await;
            return;
        }
        Err(e) => {
            // Manifest invalid, package not installed at the requested
            // version, etc. — anything that isn't an unknown agent id.
            disp.send_err(
                req.id,
                error_codes::RUNTIME_ERROR,
                format!("agent resolution failed: {}", e),
                Some(json!({ "agent_id": agent_id })),
            )
            .await;
            return;
        }
    };

    // 3. Build initial Message. tau_domain::Message has no Message::user()
    // constructor outside tau-domain (non_exhaustive + struct-literal blocked).
    // Use Message::new with Address::User sender/recipient and a Text payload.
    // The runtime overwrites the recipient address internally.
    let initial = Message::new(
        Address::User,
        Address::User,
        MessagePayload::Text { content: prompt },
    );

    // Host-shell contract (`stream.rs::clock_ref`): serve drives the core
    // streaming path directly, so it must inject the production clock +
    // randomness or `run_streaming_inner` panics on the first port use.
    let mut opts = RunOptions::default();
    opts.clock = Some(std::sync::Arc::new(tau_runtime_tokio::TokioClock));
    opts.random = Some(std::sync::Arc::new(tau_runtime_tokio::OsRandom));
    let cancel = disp.cancel_reg.register(req.id.clone());

    let result: Result<(), tau_runtime_tokio::RuntimeError> = if streaming {
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
) -> Result<(), tau_runtime_tokio::RuntimeError> {
    use tokio::select;
    // Runtime::run signature (verified in Task 11 reconciliation):
    //   pub async fn run(&self, agent_def, package_manifest, initial_message, options)
    let fut = disp.runtime.run(agent_def, manifest, initial, opts);
    select! {
        outcome = fut => {
            let outcome = outcome?;
            let body = wire::outcome_to_json(&outcome);
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

async fn execute_streaming(
    disp: &Dispatcher,
    id: RequestId,
    agent_def: tau_domain::AgentDefinition,
    manifest: tau_domain::PackageManifest,
    initial: Message,
    opts: RunOptions,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), tau_runtime_tokio::RuntimeError> {
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
    // The wire-shape projection (including the field renames vs the runtime
    // types) lives in `wire::event_to_wire`, the single source of truth.
    // `wire` carries the streaming side-effect updates the dispatcher must
    // remember for the final summary.
    let w = wire::event_to_wire(event);
    if let Some(sr) = w.stop_reason {
        *stop_reason = Some(sr);
    }
    if let Some(tu) = w.token_usage {
        *last_token_usage = Some(tu);
    }
    disp.send_notification(
        methods::RUNTIME_EVENT,
        json!({ "id": id, "kind": w.kind, "data": w.data }),
    )
    .await;
}
