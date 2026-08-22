//! Curated Variant B embedding surface (EPIC 7.1).
//!
//! A product embeds tau by (1) decoding baked IR bytes with
//! [`from_canonical_bytes`], (2) resolving the entry agent via
//! [`IrModule::entry_agent`] (sole-agent contract) or an explicit
//! [`AgentId`], (3) implementing [`ToolDispatcher`] — tool execution,
//! [`DynLlmBackend`] resolution, and the mandatory [`Clock`] +
//! [`RandomSource`] ports — and (4) driving [`run_ir`] (single outcome)
//! or [`run_ir_streaming`] ([`RunEvent`] stream; the terminal
//! `RunEvent::RunCompleted` fires exactly once).
//!
//! Everything here is a re-export: this module pins *which* items form
//! the supported embedding API. See `docs/how-to/embed-rust-native.md`
//! for the worked example (`crates/tau-embed-example`).

pub use crate::builder::DynLlmBackend;
pub use crate::error::RuntimeError;
pub use crate::interpreter::tool_dispatch::{ToolDispatcher, ToolInvocationResult};
pub use crate::interpreter::{run_ir, run_ir_streaming};
pub use crate::outcome::RunOutcome;
pub use crate::stream::RunEvent;
pub use tau_ir::{from_canonical_bytes, AgentId, EntryAgentError, IrModule};
pub use tau_ports::{
    batch_to_stream, Clock, CompletionRequest, CompletionResponse, CompletionStream, LlmBackend,
    LlmError, RandomSource, StopReason,
};

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::String;
    use std::vec::Vec;

    #[test]
    fn embed_prelude_reexports_link() {
        // Type-level smoke test: naming the items proves the re-exports
        // resolve; constructing a response proves the embedder-facing
        // constructor is reachable through the prelude.
        let r = super::CompletionResponse::new(
            String::new(),
            Vec::new(),
            super::StopReason::EndTurn,
            None,
        );
        assert!(r.text.is_empty());
        fn _dyn_surface(_: &dyn super::ToolDispatcher, _: &super::RunEvent) {}
    }
}
