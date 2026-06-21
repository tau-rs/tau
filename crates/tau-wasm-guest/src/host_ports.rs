//! Adapters mapping the three `tau:run/host` WIT imports onto the core ports
//! the interpreter consumes: LLM inference, clock, and randomness. All three
//! cross the wasm boundary because credentials (β.5) and determinism live
//! host-side.

extern crate alloc;

use alloc::string::ToString;

use tau_ports::llm::{batch_to_stream, CompletionRequest, CompletionResponse, CompletionStream};
use tau_ports::{Clock, LlmBackend, LlmError, RandomSource};

// The WIT-generated host imports are re-exported by `guest.rs` as
// `crate::wit_host` (see the `wit_host` module there). Using a re-export
// avoids coupling to the exact generated path (`crate::guest::tau::run::host`
// for wit-bindgen guest, vs `tau::run::host` on the wasmtime host side).
use crate::wit_host as host;

/// `LlmBackend` backed by the host `complete` import (cassette in conformance).
pub struct HostLlmBackend;

impl LlmBackend for HostLlmBackend {
    fn name(&self) -> &str {
        "wasm-host"
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let req_json = serde_json::to_string(&req).map_err(|e| LlmError::Internal {
            message: e.to_string(),
        })?;
        let resp_json = host::complete(&req_json).map_err(|e| LlmError::Internal { message: e })?;
        serde_json::from_str(&resp_json).map_err(|e| LlmError::Internal {
            message: e.to_string(),
        })
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        // The interpreter streams; replay the whole completion as one batch.
        let resp = self.complete(req).await?;
        Ok(batch_to_stream(resp))
    }
}

/// `Clock` backed by the host `now-millis` import.
pub struct HostClock;

impl Clock for HostClock {
    fn now(&self) -> i64 {
        host::now_millis() as i64
    }
}

/// `RandomSource` backed by the host `next-u64` import.
pub struct HostRandom;

impl RandomSource for HostRandom {
    fn fill(&self, dest: &mut [u8]) {
        let mut i = 0;
        while i < dest.len() {
            let bytes = host::next_u64().to_le_bytes();
            let take = core::cmp::min(8, dest.len() - i);
            dest[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
}
