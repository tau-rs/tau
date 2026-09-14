//! The tool loop of HANDOFF §3.2: model → tool call → resolve, validate,
//! `send`, `recv` → tool result → model, until the model stops for any other
//! reason.

use serde_json::Value;
use tau_kernel::abi::{BudgetError, Capability, Name};
use tau_kernel::bridge::{
    Content, Message, ModelReply, ModelRequest, Role, StopReason, ToolErrorKind, VERSION,
};
use tau_kernel::kernel::KernelError;
use tau_kernel::reducer::Refusal;
use tau_kernel::syscall::Handle;

use crate::infer::{await_reply, infer, InferError};
use crate::toolbox::Toolbox;

/// Why [`tool_loop`] stopped without a reply.
///
/// What is *not* here is as deliberate as what is: an unknown tool, bad
/// arguments, a capability this agent does not hold, and an unreadable tool
/// reply are all fed back to the model as `tool_result` blocks with
/// `is_error` (ADR-0006 §4), not returned. The model self-corrects.
#[derive(Debug, thiserror::Error)]
pub enum ToolLoopError {
    /// The model call itself failed.
    #[error(transparent)]
    Infer(#[from] InferError),
    /// The model stopped for a tool call but asked for none.
    #[error("malformed reply: {reason}")]
    Malformed {
        /// What was wrong.
        reason: String,
    },
    /// A tool `send` was refused on budget: the driver's ceiling cannot be
    /// reserved. Terminal, never fed back — the model would only try again
    /// with the same empty purse.
    #[error("budget refused a tool call: {0}")]
    Budget(#[source] BudgetError),
    /// A tool `send` was refused for a reason that is neither authority nor
    /// budget: the driver's inbox is full, the kernel is closed or faulted.
    #[error("send to `{name}` failed: {source}")]
    Send {
        /// The tool.
        name: Name,
        /// The kernel's answer.
        #[source]
        source: KernelError,
    },
    /// The `recv` for a tool's reply failed.
    #[error("recv from `{name}` failed: {source}")]
    Recv {
        /// The tool.
        name: Name,
        /// The kernel's answer.
        #[source]
        source: KernelError,
    },
    /// This agent was cancelled: a `Notice` arrived while waiting for a
    /// tool, or a `send` was refused because the agent is frozen. `reason`
    /// is the canceller's payload when the notice was seen, empty when only
    /// the refusal was.
    #[error("cancelled during a tool call")]
    Cancelled {
        /// The reason the canceller gave, as bytes.
        reason: Vec<u8>,
    },
}

/// A fresh request: one user turn, no system prompt, no tools, no sampling
/// controls. Set the rest on the value before the loop if you want it.
#[must_use]
pub fn prompt(user: &str, max_tokens: u32) -> ModelRequest {
    ModelRequest {
        v: VERSION,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![Content::Text { text: user.into() }],
        }],
        tools: Vec::new(),
        max_tokens,
        sampling: None,
    }
}

/// Runs the model until it stops for something other than a tool call.
///
/// `request` is the conversation. Its `tools` are replaced by the toolbox's
/// definitions, and every assistant turn and every tool-result turn the loop
/// produces is appended to its `messages`, so after the call it holds the
/// full transcript. The assistant turn is the reply's content *intact*,
/// sealed thinking blocks included and in place: the provider requires
/// them back unchanged (ADR-0007), and the loop is append-only so that the
/// prefix they were signed over stays what it was. The reply returned is the one that ended the loop —
/// `end_turn`, `max_tokens`, `stop_sequence`, `refusal`, or an error from
/// the driver — and what to do about it is the caller's policy.
///
/// Each round: every `tool_call` block in the reply is answered, in order,
/// with one `tool_result` in a single user turn. A call is answered without
/// a `send` when its name is unknown or its input fails the schema, with
/// `denied` when the kernel says this agent does not hold the capability,
/// and with `failed` when the reply cannot be read. Only a budget refusal,
/// a cancellation, or a kernel failure ends the loop early.
///
/// Cancel-safe: the loop borrows the toolbox and mutates the request, both
/// plain memory; no guard is held across an await (ADR-0003).
///
/// # Errors
///
/// See [`ToolLoopError`].
pub async fn tool_loop(
    handle: &Handle,
    model: Capability,
    tools: &Toolbox,
    request: &mut ModelRequest,
) -> Result<ModelReply, ToolLoopError> {
    request.tools = tools.defs();
    loop {
        let reply = infer(handle, model, request).await?;
        if reply.stop != StopReason::ToolCall {
            return Ok(reply);
        }
        let calls: Vec<(String, String, Value)> = reply
            .content
            .iter()
            .filter_map(|block| match block {
                Content::ToolCall { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                // Thinking is carried, never read: the whole reply goes
                // back in the assistant turn below, sealed blocks in place
                // (ADR-0007 §3).
                Content::Text { .. } | Content::ToolResult { .. } | Content::Thinking { .. } => {
                    None
                }
            })
            .collect();
        if calls.is_empty() {
            return Err(ToolLoopError::Malformed {
                reason: "stop is tool_call but the reply carries no tool_call block".into(),
            });
        }
        request.messages.push(Message {
            role: Role::Assistant,
            content: reply.content,
        });
        let mut results = Vec::with_capacity(calls.len());
        for (id, name, input) in calls {
            results.push(call_tool(handle, tools, id, &name, &input).await?);
        }
        request.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
}

/// Answers one tool call: resolve, validate, `send`, `recv`, render.
async fn call_tool(
    handle: &Handle,
    tools: &Toolbox,
    call_id: String,
    name: &str,
    input: &Value,
) -> Result<Content, ToolLoopError> {
    let Some(tool) = tools.resolve(name) else {
        let available: Vec<&str> = tools.names().map(Name::as_str).collect();
        return Ok(error_result(
            call_id,
            ToolErrorKind::UnknownTool,
            format!("unknown tool `{name}`; available: {}", available.join(", ")),
        ));
    };
    if let Err(reason) = tools.validate(tool, input) {
        return Ok(error_result(
            call_id,
            ToolErrorKind::BadArgs,
            format!("bad args for `{name}`: {reason}"),
        ));
    }
    let bytes = serde_json::to_vec(input).map_err(|e| ToolLoopError::Malformed {
        reason: format!("tool_call input for `{name}` could not be encoded: {e}"),
    })?;
    let corr = match handle.send(tool.cap, &bytes) {
        Ok(corr) => corr,
        Err(KernelError::Refused(Refusal::NotHeld { .. })) => {
            return Ok(error_result(
                call_id,
                ToolErrorKind::Denied,
                format!("denied: this agent does not hold `{name}`"),
            ));
        }
        Err(KernelError::Refused(Refusal::Budget(e))) => return Err(ToolLoopError::Budget(e)),
        Err(KernelError::Refused(Refusal::Frozen(_))) => {
            return Err(ToolLoopError::Cancelled { reason: Vec::new() });
        }
        Err(source) => {
            return Err(ToolLoopError::Send {
                name: tool.def.name.clone(),
                source,
            });
        }
    };
    let msg = await_reply(handle, corr).await.map_err(|e| match e {
        InferError::Cancelled { reason } => ToolLoopError::Cancelled { reason },
        InferError::Recv(source) => ToolLoopError::Recv {
            name: tool.def.name.clone(),
            source,
        },
        other => ToolLoopError::Infer(other),
    })?;
    Ok(render_result(call_id, name, handle.read(msg.payload)))
}

/// The `tool_result` for a driver's reply.
///
/// The reply bytes are the content verbatim, as UTF-8, lossily if they are
/// not (ADR-0006 §5). `None` — the reply's payload is not in the blob
/// store — is the one failure the loop can observe on a tool driver today,
/// and is fed back as `failed`. A driver's *own* failure has no channel
/// until the M3 error envelope; until then a tool that fails answers with
/// whatever bytes it chooses, and the model reads them.
#[must_use]
pub fn render_result(call_id: String, name: &str, reply: Option<Vec<u8>>) -> Content {
    match reply {
        Some(bytes) => Content::ToolResult {
            call_id,
            content: String::from_utf8_lossy(&bytes).into_owned(),
            is_error: false,
            error_kind: None,
        },
        None => error_result(
            call_id,
            ToolErrorKind::Failed,
            format!("`{name}`: reply payload unavailable"),
        ),
    }
}

fn error_result(call_id: String, kind: ToolErrorKind, content: String) -> Content {
    Content::ToolResult {
        call_id,
        content,
        is_error: true,
        error_kind: Some(kind),
    }
}
