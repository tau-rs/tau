//! tau-embed-example — EPIC 7.1 Variant B reference host.
//!
//! A "product" that links tau as a library: it bakes the IR of a governed
//! workflow (`fixtures/trivial.ir.json`, lowered from `project/tau.toml` — a
//! drift test in tau-cli keeps them byte-equal), implements the four ports
//! (LLM, tools, clock, entropy), and drives `run_ir_streaming`, printing
//! every `RunEvent` as a JSON line.
#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::Value;
use tau_ir::ToolId;
use tau_runtime_core::embed::{
    batch_to_stream, from_canonical_bytes, run_ir_streaming, Clock, CompletionRequest,
    CompletionResponse, CompletionStream, DynLlmBackend, LlmBackend, LlmError, RandomSource,
    RunEvent, RuntimeError, StopReason, ToolDispatcher, ToolInvocationResult,
};

/// Canonical IR of `project/tau.toml`, baked at compile time exactly like a
/// `tau build --target rust-lib` artifact bakes `TAU_IR`.
const TAU_IR: &[u8] = include_bytes!("../fixtures/trivial.ir.json");

/// The product's inference client. This example answers offline with a
/// canned completion; a real product calls its LLM service here.
struct EchoBackend;

impl LlmBackend for EchoBackend {
    fn name(&self) -> &str {
        "embed-example-echo"
    }

    async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse::new(
            "embed-example reply".to_string(),
            Vec::new(),
            StopReason::EndTurn,
            None,
        ))
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        // UFCS: the DynLlmBackend blanket impl makes `self.complete(..)` ambiguous.
        Ok(batch_to_stream(LlmBackend::complete(self, req).await?))
    }
}

/// Wall clock in ms since the Unix epoch.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// xorshift64* seeded from the clock — NOT cryptographic; a real product
/// supplies OS entropy here.
struct XorShiftRandom(AtomicU64);

impl RandomSource for XorShiftRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let mut x = self.0.load(Ordering::Relaxed);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0.store(x, Ordering::Relaxed);
            let bytes = x.wrapping_mul(0x2545F4914F6CDD1D).to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
}

/// The product's port surface: tools + LLM backend + mandatory clock/random.
struct ProductDispatcher {
    backend: Arc<EchoBackend>,
    clock: Arc<SystemClock>,
    random: Arc<XorShiftRandom>,
}

impl ProductDispatcher {
    fn new() -> Self {
        let seed = SystemClock.now() as u64 | 1;
        Self {
            backend: Arc::new(EchoBackend),
            clock: Arc::new(SystemClock),
            random: Arc::new(XorShiftRandom(AtomicU64::new(seed))),
        }
    }
}

impl ToolDispatcher for ProductDispatcher {
    fn invoke<'a>(
        &'a self,
        tool_id: &'a ToolId,
        _args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolInvocationResult, RuntimeError>> + Send + 'a>> {
        // The example workflow declares no tools; a real product routes
        // tool_id to its own implementations here.
        Box::pin(async move {
            Err(RuntimeError::ToolNotRegistered {
                tool_name: tool_id.0.clone(),
                registered: Vec::new(),
            })
        })
    }

    fn llm_backend_for(&self, _backend: &str) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
        Ok(self.backend.clone())
    }

    fn clock(&self) -> Option<Arc<dyn Clock>> {
        Some(self.clock.clone())
    }

    fn random(&self) -> Option<Arc<dyn RandomSource>> {
        Some(self.random.clone())
    }
}

/// Run the baked workflow, calling `on_event` for every event.
async fn run(on_event: &mut dyn FnMut(&RunEvent)) -> Result<(), Box<dyn std::error::Error>> {
    let module = Arc::new(from_canonical_bytes(TAU_IR)?);
    let entry = module.entry_agent()?.clone();
    let stream = run_ir_streaming(
        module,
        &entry,
        Arc::new(ProductDispatcher::new()),
        Vec::new(),
    )
    .await?;
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        on_event(&event);
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures::executor::block_on(run(&mut |event| {
        println!(
            "{}",
            serde_json::to_string(event).expect("RunEvent serializes")
        );
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_baked_workflow_to_completion() {
        let mut saw_completed = false;
        futures::executor::block_on(run(&mut |event| {
            if matches!(event, RunEvent::RunCompleted { .. }) {
                saw_completed = true;
            }
        }))
        .expect("workflow runs");
        assert!(
            saw_completed,
            "terminal RunCompleted must fire exactly once"
        );
    }
}
