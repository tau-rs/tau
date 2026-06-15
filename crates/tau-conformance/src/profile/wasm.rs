//! Compiled-wasm profile. STUB — unblocks at β.7.5 (`tau build wasm`).
//! TODO(β.7.5): build the fan-monitor as a wasm component, run it in
//! wasmtime, and harvest the guest's [`ConformanceEvent`] stream.

use crate::event::ConformanceEvent;
use crate::profile::{Profile, ProfileError};
use crate::scenario::Scenario;

/// Placeholder for the compiled-wasm profile. See the module docs.
pub struct WasmProfile;

#[async_trait::async_trait(?Send)]
impl Profile for WasmProfile {
    fn name(&self) -> &str {
        "wasm"
    }

    async fn run(&self, _scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError> {
        unimplemented!(
            "TODO(β.7.5): drive tau build wasm artifact in wasmtime, harvest guest ConformanceEvents"
        )
    }
}
