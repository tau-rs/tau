//! Per-agent loop driver.
//!
//! Routes through the existing `Runtime::run_with_history` (kernel
//! agent loop) by constructing a `Runtime` configured with the agent
//! node's tools, prompt, model, and budget. The ToolDispatcher trait
//! call (Task 4.3) is what each tool reaches when invoked — the
//! `Runtime`'s tool registry is wired with a thin wrapper that delegates
//! to the dispatcher.

use alloc::sync::Arc;
use alloc::vec::Vec;

use tau_domain::Message;
use tau_ir::{Agent, IrModule};

use crate::error::RuntimeError;
use crate::interpreter::tool_dispatch::ToolDispatcher;
use crate::outcome::RunOutcome;
use crate::options::TokenUsage;

/// Execute one `Agent` node end-to-end.
///
/// # TODO (β.2.4 Step 3)
///
/// Replace this stub with the real agent-loop wiring:
///
/// 1. Build a `RuntimeBuilder` with `dispatcher.llm_backend()`.
/// 2. For each `ToolId` in `agent.tool_refs`, look up the `Tool` in
///    `module.workflow.tools` for its spec. Wrap with a
///    `tau_ports::Tool` impl that delegates `invoke` to
///    `dispatcher.invoke(tool_id, args)`. Register via `with_tool`.
/// 3. Build the Runtime; call `run_with_history(...)`.
/// 4. Return the `RunOutcome` unchanged.
pub async fn run_agent<D>(
    module: &IrModule,
    agent: &Agent,
    _dispatcher: Arc<D>,
    initial_messages: Vec<Message>,
) -> Result<RunOutcome, RuntimeError>
where
    D: ToolDispatcher + Send + Sync + 'static,
{
    let _ = (module, agent);
    // Use the first supplied message as the final_message stub, or synthesise
    // a placeholder so the smoke test can pattern-match on Completed.
    let final_message = initial_messages
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            use tau_domain::{Address, MessagePayload};
            Message::new(
                Address::System,
                Address::System,
                MessagePayload::Text {
                    content: alloc::string::String::new(),
                },
            )
        });
    Ok(RunOutcome::Completed {
        final_message,
        all_messages: alloc::vec![],
        total_turns: 0,
        token_usage: TokenUsage::default(),
    })
}
