//! β.7.5 wasm host — a std [`wasmtime`] embedder for the `tau-wasm-guest`
//! WASI 0.2 component.
//!
//! The guest (built for `wasm32-wasip2`, see the `tau-wasm-guest` crate)
//! exports `run(prompt) -> result<string, string>` from the `tau:host/runner`
//! world and imports three host ports it cannot satisfy in-wasm:
//!
//! - `complete(request-json) -> result<string, string>` — delegated
//!   inference (credentials live host-side, β.5).
//! - `now-millis() -> u64` — wall clock.
//! - `next-u64() -> u64` — randomness.
//!
//! This crate satisfies those imports with **deterministic** stubs so the
//! same `(component, prompt, llm_responses)` triple always yields the same
//! bytes — the property β.6 conformance (`WasmProfile`) depends on. Time is
//! a fixed-step counter, randomness is a seeded SplitMix64 PRNG, and
//! inference replays a caller-supplied queue of canned `CompletionResponse`
//! JSON strings (cassette-style).
//!
//! [`run_component`] is the whole public surface: feed it the guest bytes
//! and it instantiates, drives `run`, and hands back the JSON string the
//! guest returned.

use std::collections::VecDeque;

use tau_ports::llm::CompletionResponse;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};

mod wasi;
pub use wasi::{wasi_grants_from_caps, HostAccess, PreopenGrant, WasiGrants};

wasmtime::component::bindgen!({
    path: "../../wit/tau-host.wit",
    world: "runner",
});

use tau::host::host;

/// Deterministic step (ms) added to the clock on every `now-millis` call.
const CLOCK_STEP_MILLIS: u64 = 1;
/// Seed for the [`HostState`] PRNG. Fixed so randomness is reproducible.
const PRNG_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Errors surfaced by [`run_component`]. `thiserror` at the crate boundary;
/// `wasmtime`/`serde_json` failures are wrapped, never leaked as `anyhow`.
#[derive(Debug, thiserror::Error)]
pub enum WasmHostError {
    /// The supplied bytes are not a loadable component.
    #[error("failed to load wasm component: {0}")]
    Load(#[source] anyhow::Error),
    /// Linking the host imports or instantiating the component failed.
    #[error("failed to instantiate component: {0}")]
    Instantiate(#[source] anyhow::Error),
    /// The guest trapped (panicked / unreachable) while running `run`.
    #[error("guest `run` trapped: {0}")]
    Trap(#[source] anyhow::Error),
    /// The guest's `run` completed but returned its error arm.
    #[error("guest `run` returned an error: {0}")]
    Guest(String),
    /// A caller-supplied canned completion response is not valid
    /// `CompletionResponse` JSON. Caught up-front so a malformed cassette
    /// fails before instantiation rather than mid-run.
    #[error("invalid canned completion response JSON: {0}")]
    InvalidResponse(#[source] serde_json::Error),
    /// A capability maps to `Disposition::Unsupported` on wasm (should have
    /// been rejected at `tau build wasm`; belt-and-suspenders at host time).
    #[error("capability unsupported on wasm: {reason}")]
    UnsupportedCap { reason: String },
}

/// Store data backing the three `tau:host/host` imports with deterministic
/// behaviour. One instance per [`run_component`] call.
struct HostState {
    /// Queue of canned `CompletionResponse` JSON strings, popped front-first
    /// on each `complete` call. Empty queue → `complete` returns its error
    /// arm so a guest that over-calls fails loudly rather than hangs.
    responses: VecDeque<String>,
    /// Monotonic millisecond counter; advances by [`CLOCK_STEP_MILLIS`].
    clock_millis: u64,
    /// SplitMix64 state, seeded from [`PRNG_SEED`].
    prng_state: u64,
}

impl HostState {
    fn new(responses: Vec<String>) -> Self {
        Self {
            responses: responses.into(),
            clock_millis: 0,
            prng_state: PRNG_SEED,
        }
    }
}

impl host::Host for HostState {
    fn complete(&mut self, _request_json: String) -> Result<String, String> {
        match self.responses.pop_front() {
            Some(resp) => Ok(resp),
            None => Err("tau-wasm-host: no canned completion response left".to_string()),
        }
    }

    fn now_millis(&mut self) -> u64 {
        let now = self.clock_millis;
        self.clock_millis = self.clock_millis.wrapping_add(CLOCK_STEP_MILLIS);
        now
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64 — small, fast, fully deterministic from a seed.
        self.prng_state = self.prng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.prng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Build the determinism `wasmtime::Config` (spec §7): canonicalise NaNs and
/// turn off the nondeterministic relaxed-SIMD lowerings so float/SIMD codegen
/// is bit-stable across hosts.
fn determinism_config() -> wasmtime::Result<Config> {
    let mut config = Config::new();
    config.cranelift_nan_canonicalization(true);
    config.wasm_relaxed_simd(false);
    Ok(config)
}

/// Load `wasm_bytes` as a component, satisfy the three `tau:host/host` imports
/// with deterministic stubs, instantiate it under a determinism `Config`,
/// and drive the exported `run(prompt)`.
///
/// `llm_responses` is the cassette: each `complete` call pops the next entry
/// (validated as `CompletionResponse` JSON up-front). Returns the JSON string
/// the guest's `run` produced on its `ok` arm.
///
/// Determinism contract: for fixed inputs the returned bytes are identical
/// across calls and hosts — the property `WasmProfile` conformance relies on.
pub fn run_component(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
) -> Result<String, WasmHostError> {
    // Fail fast on a malformed cassette before touching wasmtime.
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }

    let config = determinism_config().map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let engine = Engine::new(&config).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let component =
        Component::new(&engine, wasm_bytes).map_err(|e| WasmHostError::Load(e.into()))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    Runner::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    let mut store = Store::new(&engine, HostState::new(llm_responses));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::Host as _;

    fn canned_response() -> String {
        // Minimal valid CompletionResponse JSON for cassette validation.
        r#"{"text":"","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
    }

    #[test]
    fn clock_advances_by_fixed_step() {
        let mut state = HostState::new(vec![]);
        assert_eq!(state.now_millis(), 0);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS * 2);
    }

    #[test]
    fn prng_is_deterministic_and_seeded() {
        let mut a = HostState::new(vec![]);
        let mut b = HostState::new(vec![]);
        let seq_a: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b, "same seed must yield same sequence");
        assert_ne!(seq_a[0], seq_a[1], "PRNG must actually advance");
    }

    #[test]
    fn complete_pops_responses_then_errors() {
        let mut state = HostState::new(vec!["first".to_string(), "second".to_string()]);
        assert_eq!(state.complete(String::new()), Ok("first".to_string()));
        assert_eq!(state.complete(String::new()), Ok("second".to_string()));
        assert!(
            state.complete(String::new()).is_err(),
            "exhausted queue errors"
        );
    }

    #[test]
    fn malformed_cassette_rejected_before_wasm() {
        let err = run_component(&[], "p", vec!["not json".to_string()]).unwrap_err();
        assert!(matches!(err, WasmHostError::InvalidResponse(_)));
    }

    #[test]
    fn well_formed_cassette_passes_validation_then_fails_at_load() {
        // Empty bytes are a valid cassette but not a loadable component:
        // proves validation precedes the wasmtime load path.
        let err = run_component(&[], "p", vec![canned_response()]).unwrap_err();
        assert!(matches!(err, WasmHostError::Load(_)), "got: {err:?}");
    }
}
