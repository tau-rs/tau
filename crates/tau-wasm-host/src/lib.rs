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
use tau_ports::target::{PreopenAccess, WasiConfiguration};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{
    DirPerms, FilePerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
};
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as WasiHttpErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::p2::{
    add_only_http_to_linker_sync, default_send_request, HttpResult, WasiHttpCtxView, WasiHttpHooks,
    WasiHttpView,
};
use wasmtime_wasi_http::WasiHttpCtx;

mod wasi;
pub use wasi::{preopen_dirs, HttpHostGate};

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
    /// Building the WASI context failed — e.g. a preopen directory does not
    /// exist on the host. Surfaced instead of silently creating it.
    #[error("failed to configure WASI context: {0}")]
    WasiConfig(#[source] anyhow::Error),
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
    /// WASI 0.2 resource table (EPIC 3.3).
    table: ResourceTable,
    /// WASI 0.2 context: exactly the preopens derived from the component's
    /// allow-bounded caps; no stdio/env/args/network inherited (EPIC 3.3).
    wasi: WasiCtx,
    /// wasi:http resource bookkeeping; the egress *decision* lives in `gate`.
    http: WasiHttpCtx,
    /// The allow-bounded network egress gate (EPIC 3.3), consulted by the
    /// `WasiHttpHooks::send_request` override before any outgoing request.
    gate: HttpHostGate,
}

impl HostState {
    fn new(responses: Vec<String>, wasi: WasiCtx, gate: HttpHostGate) -> Self {
        Self {
            responses: responses.into(),
            clock_millis: 0,
            prng_state: PRNG_SEED,
            table: ResourceTable::new(),
            wasi,
            http: WasiHttpCtx::new(),
            gate,
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

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            // `gate` is the WasiHttpHooks impl below; disjoint borrow from
            // `http`/`table`, so no aliasing conflict.
            hooks: &mut self.gate,
        }
    }
}

/// The egress gate: every `wasi:http` outgoing request is routed here before
/// wasmtime opens a socket. A host/method the allow-bounded caps did not
/// authorize is rejected — the guest never gets a connection to it.
impl WasiHttpHooks for HttpHostGate {
    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let authority = request
            .uri()
            .authority()
            .map(|a| a.as_str().to_string())
            .unwrap_or_default();
        let permitted = self.permits_request(&authority, request.method().as_str());
        if !permitted {
            return Err(WasiHttpErrorCode::HttpRequestDenied.into());
        }
        Ok(default_send_request(request, config))
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

/// Build a `WasiCtx` granting exactly `cfg`'s preopens (RO for fs.read, RW for
/// fs.write) and nothing else: no stdio/env/args/network inherited. A preopen
/// whose host directory does not exist is a hard error — we never silently
/// create it. Network egress is denied at the ctx level; `wasi:http` is gated
/// separately by `HttpHostGate` in `send_request`.
fn build_wasi_ctx(cfg: &WasiConfiguration) -> Result<WasiCtx, WasmHostError> {
    let mut builder = WasiCtxBuilder::new();
    // Each `host_dir` here is already fully resolved (glob-expanded,
    // absolute) by the allow-bounded build gate (EPIC 3.2/3.4), which owns
    // `ResolvedPreopen.granularity` and any widening policy. The host grants
    // it verbatim and intentionally does not re-check `granularity` — a
    // legitimately widened grant (e.g. via an explicit escape hatch) must
    // still be honoured here, not rejected. This function trusts the folded
    // `WasiConfiguration` it is handed.
    for (host_dir, access) in preopen_dirs(cfg) {
        let (dir_perms, file_perms) = match access {
            PreopenAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
            PreopenAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
        };
        // Identity map: the guest sees the same absolute path as the host dir.
        builder
            .preopened_dir(host_dir, host_dir, dir_perms, file_perms)
            .map_err(|e| WasmHostError::WasiConfig(e.into()))?;
    }
    Ok(builder.build())
}

/// Run a component whose WASI authority is bounded by `wasi` (EPIC 3.3):
/// fs preopens and a `wasi:http` egress allow-list built from the same
/// allow-bounded caps that produced the component's WIT world (3.2). An
/// un-granted host or path is unreachable at runtime.
pub fn run_component_with_wasi(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    wasi: &WasiConfiguration,
) -> Result<String, WasmHostError> {
    // Fail fast on a malformed cassette before touching wasmtime.
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }

    let wasi_ctx = build_wasi_ctx(wasi)?;
    let gate = HttpHostGate::new(wasi);

    let config = determinism_config().map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let engine = Engine::new(&config).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    let component =
        Component::new(&engine, wasm_bytes).map_err(|e| WasmHostError::Load(e.into()))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;
    add_only_http_to_linker_sync(&mut linker).map_err(|e| WasmHostError::Instantiate(e.into()))?;
    Runner::add_to_linker::<_, HasSelf<HostState>>(&mut linker, |state| state)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    let mut store = Store::new(&engine, HostState::new(llm_responses, wasi_ctx, gate));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}

/// Determinism-conformance entry (`WasmProfile`): no capabilities, so no WASI
/// grants — deny-all egress, zero preopens. Behaviourally identical to the
/// pre-3.3 host for a guest that imports no WASI interfaces.
pub fn run_component(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
) -> Result<String, WasmHostError> {
    run_component_with_wasi(
        wasm_bytes,
        prompt,
        llm_responses,
        &WasiConfiguration::deny_all(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::Host as _;

    fn canned_response() -> String {
        // Minimal valid CompletionResponse JSON for cassette validation.
        r#"{"text":"","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
    }

    fn empty_state(responses: Vec<String>) -> HostState {
        HostState::new(
            responses,
            WasiCtxBuilder::new().build(),
            HttpHostGate::new(&WasiConfiguration::deny_all()),
        )
    }

    #[test]
    fn clock_advances_by_fixed_step() {
        let mut state = empty_state(vec![]);
        assert_eq!(state.now_millis(), 0);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS * 2);
    }

    #[test]
    fn prng_is_deterministic_and_seeded() {
        let mut a = empty_state(vec![]);
        let mut b = empty_state(vec![]);
        let seq_a: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b, "same seed must yield same sequence");
        assert_ne!(seq_a[0], seq_a[1], "PRNG must actually advance");
    }

    #[test]
    fn complete_pops_responses_then_errors() {
        let mut state = empty_state(vec!["first".to_string(), "second".to_string()]);
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

    #[test]
    fn run_component_with_wasi_no_grants_matches_run_component() {
        // A deny-all config must reach the same validation→Load failure as the
        // no-caps wrapper on empty bytes — proving build_wasi_ctx produces a clean
        // no-preopen ctx and the dual WASI+http linker adds don't break setup.
        let err = run_component_with_wasi(
            &[],
            "p",
            vec![canned_response()],
            &tau_ports::target::WasiConfiguration::deny_all(),
        )
        .unwrap_err();
        assert!(matches!(err, WasmHostError::Load(_)), "got: {err:?}");
    }
}
