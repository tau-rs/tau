//! β.7.5 wasm host — a std [`wasmtime`] embedder for the `tau-wasm-guest`
//! WASI 0.2 component.
//!
//! The guest (built for `wasm32-wasip2`, see the `tau-wasm-guest` crate)
//! exports `run(prompt) -> result<string, string>` from the `tau:host/runner`
//! world and imports four host ports it cannot satisfy in-wasm:
//!
//! - `complete(request-json) -> result<string, string>` — delegated
//!   inference (credentials live host-side, β.5).
//! - `now-millis() -> u64` — wall clock.
//! - `next-u64() -> u64` — randomness.
//! - `emit-event(event-json: string)` — streams one `RunEvent` at a time
//!   (fire-and-forget; see [`run_component`]'s return value).
//!
//! This crate satisfies those imports with **deterministic** stubs so the
//! same `(component, prompt, llm_responses)` triple always yields the same
//! bytes — the property β.6 conformance (`WasmProfile`) depends on. Time is
//! a fixed-step counter, randomness is a seeded SplitMix64 PRNG, and
//! inference replays a caller-supplied queue of canned `CompletionResponse`
//! JSON strings (cassette-style).
//!
//! [`run_component`] is the whole public surface: feed it the guest bytes
//! and it instantiates, drives `run`, and hands back the guest's payload
//! (an empty sentinel — events flow via `emit-event`) plus the `RunEvent`
//! JSON strings streamed via `emit-event`, in order.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tau_ports::llm::{CompletionRequest, CompletionResponse};
use tau_ports::target::wasi_map::PreopenAccess;
use tau_ports::target::{resolve_wasi_config, WasiConfiguration};
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

pub mod embed;
mod wasi;
pub use wasi::EgressPolicy;

wasmtime::component::bindgen!({
    path: "../../wit/tau-host.wit",
    world: "runner",
});

use tau::host::host;

/// Deterministic step (ms) added to the clock on every `now-millis` call.
const CLOCK_STEP_MILLIS: u64 = 1;
/// Seed for the [`HostState`] PRNG. Fixed so randomness is reproducible.
const PRNG_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// The four host functions a product supplies to run a tau component —
/// the Rust face of the `tau:host/runner` WIT imports (EPIC 7.2).
///
/// Ports are owned by the run (`Box<dyn EmbedPorts>`; wasmtime's
/// `Store<T: 'static>` forbids borrows). Impls that need state back after
/// the run hold an `Arc` handle and keep a clone outside.
pub trait EmbedPorts: Send {
    /// Live inference. Typed: the host layer (de)serializes across the WIT
    /// boundary; the `Err` string crosses into the guest's `result` error
    /// arm. The WIT import is synchronous — an async product client blocks
    /// inside its impl.
    fn complete(&mut self, req: CompletionRequest) -> Result<CompletionResponse, String>;
    /// Wall clock in milliseconds.
    fn now_millis(&mut self) -> u64;
    /// Next value from the product's entropy source.
    fn next_u64(&mut self) -> u64;
    /// One serialized `RunEvent` per call, in order, live as the guest emits
    /// it. Raw JSON so this crate needs no tau-runtime-core dependency;
    /// products deserialize with their own dep if they want typed events.
    fn on_event(&mut self, event_json: &str);
}

/// The conformance ports: canned-cassette LLM, fixed-step clock, seeded
/// SplitMix64 — exactly the behavior `run_component` has always had, now as
/// one `EmbedPorts` impl.
pub(crate) struct DeterministicPorts {
    responses: VecDeque<String>,
    clock_millis: u64,
    prng_state: u64,
    emitted: Arc<Mutex<Vec<String>>>,
}

impl DeterministicPorts {
    /// Returns the ports plus the shared event-buffer handle the caller
    /// drains after the run.
    pub(crate) fn new(responses: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                responses: responses.into(),
                clock_millis: 0,
                prng_state: PRNG_SEED,
                emitted: Arc::clone(&emitted),
            },
            emitted,
        )
    }
}

impl EmbedPorts for DeterministicPorts {
    fn complete(&mut self, _req: CompletionRequest) -> Result<CompletionResponse, String> {
        let raw = self
            .responses
            .pop_front()
            .ok_or_else(|| "tau-wasm-host: no canned completion response left".to_string())?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("tau-wasm-host: invalid canned CompletionResponse: {e}"))
    }

    fn now_millis(&mut self) -> u64 {
        let now = self.clock_millis;
        self.clock_millis = self.clock_millis.wrapping_add(CLOCK_STEP_MILLIS);
        now
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64 — identical to the legacy HostState sequence.
        self.prng_state = self.prng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.prng_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn on_event(&mut self, event_json: &str) {
        self.emitted
            .lock()
            .expect("event buffer lock")
            .push(event_json.to_string());
    }
}

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
    /// Building the WASI context (e.g. a preopen dir failed to open, or a
    /// resolved preopen would escape the sandbox root).
    #[error("failed to configure WASI context: {0}")]
    WasiConfig(#[source] anyhow::Error),
}

/// Store data backing the `tau:host/host` imports. One instance per
/// [`run_component_with_ports`] call.
///
/// No `#[derive(Debug)]`: the WASI fields (`WasiCtx`/`WasiHttpCtx`, EPIC 3.3)
/// are not `Debug`, and this struct is never formatted.
struct HostState {
    /// The four host functions, supplied by the caller (EPIC 7.2). The
    /// conformance entrypoints pass `DeterministicPorts`.
    ports: Box<dyn EmbedPorts>,
    /// WASI 0.2 resource table (EPIC 3.3).
    table: ResourceTable,
    /// WASI 0.2 host context: exactly the preopens/network derived from the
    /// component's allow-bounded caps (EPIC 3.3).
    wasi: WasiCtx,
    /// wasi:http resource bookkeeping (EPIC 3.3); the egress *decision* lives
    /// in `egress`, not here.
    http: WasiHttpCtx,
    /// Network egress policy folded from the component's allow-bounded caps
    /// (EPIC 3.3), sourced from the canonical `resolve_wasi_config`. Consulted
    /// by the `WasiHttpHooks::send_request` override below before any outgoing
    /// request is sent.
    egress: EgressPolicy,
}

impl HostState {
    fn new(ports: Box<dyn EmbedPorts>, wasi: WasiCtx, egress: EgressPolicy) -> Self {
        Self {
            ports,
            table: ResourceTable::new(),
            wasi,
            http: WasiHttpCtx::new(),
            egress,
        }
    }
}

impl host::Host for HostState {
    fn complete(&mut self, request_json: String) -> Result<String, String> {
        let req: CompletionRequest = serde_json::from_str(&request_json)
            .map_err(|e| format!("tau-wasm-host: malformed CompletionRequest from guest: {e}"))?;
        let resp = self.ports.complete(req)?;
        serde_json::to_string(&resp)
            .map_err(|e| format!("tau-wasm-host: failed to serialize CompletionResponse: {e}"))
    }

    fn now_millis(&mut self) -> u64 {
        self.ports.now_millis()
    }

    fn next_u64(&mut self) -> u64 {
        self.ports.next_u64()
    }

    fn emit_event(&mut self, event_json: String) {
        self.ports.on_event(&event_json);
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
            // `egress` doubles as the `WasiHttpHooks` impl (below) that gates
            // every outgoing request; disjoint field borrow from `http`/`table`
            // above so no aliasing conflict.
            hooks: &mut self.egress,
        }
    }
}

/// The egress gate: every `wasi:http` outgoing request is routed through
/// `WasiHttpHooks::send_request` before wasmtime opens a socket. An authority
/// or HTTP method that the allow-bounded caps didn't authorize is rejected
/// here — the guest never gets a connection to it.
impl WasiHttpHooks for EgressPolicy {
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
        let method = request.method().as_str();
        if !self.permits(&authority, method) {
            return Err(WasiHttpErrorCode::HttpRequestDenied.into());
        }
        Ok(default_send_request(request, config))
    }
}

/// Build a `WasiCtx` that preopens exactly `cfg.preopens` (each resolved
/// under `sandbox_root`) and denies all network egress by default (wasi:http
/// egress is gated separately by the [`EgressPolicy`] filter; raw
/// `wasi:sockets` stays default-deny).
///
/// `resolve_wasi_config` already dropped any non-G2 / `..`-bearing path, so
/// every `host_dir` is absolute and escape-free; the `starts_with` check is
/// defense-in-depth against a future resolver regression.
fn wasi_ctx_from_config(
    cfg: &WasiConfiguration,
    sandbox_root: &Path,
) -> Result<WasiCtx, WasmHostError> {
    let mut builder = WasiCtxBuilder::new();
    for p in &cfg.preopens {
        // Defense-in-depth: `resolve_wasi_config` already drops non-G2 /
        // `..`-bearing paths, but this is a public API over arbitrary caps, so
        // re-check lexically. Scan segments explicitly — a component-wise
        // `PathBuf::starts_with(sandbox_root)` AFTER `join` does NOT catch
        // `..` (e.g. `<root>/../x` still `starts_with` `<root>`), so it would
        // be a false guard.
        if !p.host_dir.starts_with('/') || p.host_dir.split('/').any(|seg| seg == "..") {
            return Err(WasmHostError::WasiConfig(anyhow::anyhow!(
                "unsafe preopen path (not absolute or contains `..`): {}",
                p.host_dir
            )));
        }
        // Map the guest-visible absolute `host_dir` under the sandbox root.
        let host_path = sandbox_root.join(p.host_dir.trim_start_matches('/'));
        // Ensure the host dir exists so preopen succeeds.
        std::fs::create_dir_all(&host_path).map_err(|e| WasmHostError::WasiConfig(e.into()))?;
        let (dir_perms, file_perms) = match p.access {
            PreopenAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
            PreopenAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
        };
        builder
            .preopened_dir(&host_path, &p.host_dir, dir_perms, file_perms)
            .map_err(|e| WasmHostError::WasiConfig(e.into()))?;
    }
    Ok(builder.build())
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

/// Load `wasm_bytes` as a component, satisfy the four `tau:host/host` imports
/// with deterministic stubs, instantiate it under a determinism `Config`,
/// and drive the exported `run(prompt)`.
///
/// `llm_responses` is the cassette: each `complete` call pops the next entry
/// (validated as `CompletionResponse` JSON up-front). Returns a
/// `(payload, emitted)` pair: `payload` is the JSON string the guest's `run`
/// produced on its `ok` arm — an empty sentinel now that events flow through
/// `emit-event` instead (design D2) — and `emitted` is every `RunEvent` JSON
/// string streamed via `emit-event`, in order.
///
/// Determinism contract: for fixed inputs the returned bytes are identical
/// across calls and hosts — the property `WasmProfile` conformance relies on.
/// EPIC 3.3: run a component whose WASI authority is bounded by `caps`.
/// Filesystem globs resolve to preopens under `sandbox_root`; network egress
/// is denied unless a `net.http` cap authorizes the target host.
pub fn run_component_with_caps(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<(String, Vec<String>), WasmHostError> {
    // Fail fast on a malformed cassette before touching wasmtime.
    for resp in &llm_responses {
        serde_json::from_str::<CompletionResponse>(resp).map_err(WasmHostError::InvalidResponse)?;
    }
    let (ports, emitted) = DeterministicPorts::new(llm_responses);
    let payload = run_component_with_ports(wasm_bytes, prompt, Box::new(ports), caps, sandbox_root)?;
    let emitted = std::mem::take(&mut *emitted.lock().expect("event buffer lock"));
    Ok((payload, emitted))
}

/// Variant A embedding entry (EPIC 7.2): run a built tau component with
/// caller-supplied [`EmbedPorts`]. Events reach `ports.on_event` live, one
/// serialized `RunEvent` per call; the `Ok` value is the guest's payload
/// sentinel (empty today, reserved for forward-compat).
pub fn run_component_with_ports(
    wasm_bytes: &[u8],
    prompt: &str,
    ports: Box<dyn EmbedPorts>,
    caps: &[tau_domain::Capability],
    sandbox_root: &Path,
) -> Result<String, WasmHostError> {
    let cfg = resolve_wasi_config(caps);
    let wasi = wasi_ctx_from_config(&cfg, sandbox_root)?;
    let egress = EgressPolicy::from_config(&cfg);

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

    let mut store = Store::new(&engine, HostState::new(ports, wasi, egress));
    let runner = Runner::instantiate(&mut store, &component, &linker)
        .map_err(|e| WasmHostError::Instantiate(e.into()))?;

    match runner.call_run(&mut store, prompt) {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(guest_err)) => Err(WasmHostError::Guest(guest_err)),
        Err(trap) => Err(WasmHostError::Trap(trap.into())),
    }
}

/// Determinism-conformance entry: no capabilities, so no WASI grants.
/// Returns the same `(payload, emitted)` pair as [`run_component_with_caps`].
pub fn run_component(
    wasm_bytes: &[u8],
    prompt: &str,
    llm_responses: Vec<String>,
) -> Result<(String, Vec<String>), WasmHostError> {
    run_component_with_caps(wasm_bytes, prompt, llm_responses, &[], Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::Host as _;
    use tau_ports::llm::CompletionRequest;

    fn canned_response() -> String {
        // Minimal valid CompletionResponse JSON for cassette validation.
        r#"{"text":"","tool_uses":[],"stop_reason":"EndTurn","usage":null}"#.to_string()
    }

    /// A `WasiCtx` with no preopens and no network — the no-caps baseline.
    fn empty_wasi_ctx() -> WasiCtx {
        WasiCtxBuilder::new().build()
    }

    /// The deny-all egress policy the no-caps path produces (empty cap set →
    /// `resolve_wasi_config` yields `Exact({})` = deny all egress).
    fn empty_egress() -> EgressPolicy {
        let no_caps: [tau_domain::Capability; 0] = [];
        EgressPolicy::from_config(&resolve_wasi_config(&no_caps))
    }

    #[test]
    fn clock_advances_by_fixed_step() {
        let mut state = HostState::new(
            Box::new(DeterministicPorts::new(vec![]).0),
            empty_wasi_ctx(),
            empty_egress(),
        );
        assert_eq!(state.now_millis(), 0);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS);
        assert_eq!(state.now_millis(), CLOCK_STEP_MILLIS * 2);
    }

    #[test]
    fn prng_is_deterministic_and_seeded() {
        let mut a = HostState::new(
            Box::new(DeterministicPorts::new(vec![]).0),
            empty_wasi_ctx(),
            empty_egress(),
        );
        let mut b = HostState::new(
            Box::new(DeterministicPorts::new(vec![]).0),
            empty_wasi_ctx(),
            empty_egress(),
        );
        let seq_a: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b, "same seed must yield same sequence");
        assert_ne!(seq_a[0], seq_a[1], "PRNG must actually advance");
    }

    #[test]
    fn complete_pops_responses_then_errors() {
        // HostState::complete now round-trips JSON across the delegation
        // boundary (EPIC 7.2); the raw cassette-queue behavior it used to
        // implement directly is covered at the `DeterministicPorts` level by
        // `deterministic_ports_pops_cassette_in_order_and_exhausts_with_legacy_error`.
        // This test proves HostState's `host::Host::complete` still delegates
        // correctly end-to-end: valid request JSON in, canned response out,
        // exhausted queue surfaces as an `Err` string.
        let mut state = HostState::new(
            Box::new(DeterministicPorts::new(vec![canned_response(), canned_response()]).0),
            empty_wasi_ctx(),
            empty_egress(),
        );
        let req_json =
            serde_json::to_string(&CompletionRequest::new("m".to_string())).expect("req json");
        assert!(state.complete(req_json.clone()).is_ok());
        assert!(state.complete(req_json.clone()).is_ok());
        assert!(
            state.complete(req_json).is_err(),
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
    fn run_component_with_caps_no_caps_matches_run_component() {
        // Wiring smoke test: an empty caps slice must reach the same
        // validation-then-load failure as the no-caps `run_component`
        // wrapper — proves `resolve_wasi_config` + `wasi_ctx_from_config`
        // build a clean, no-preopen `WasiCtx` and don't blow up on an empty
        // sandbox_root.
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run_component_with_caps(&[], "p", vec![canned_response()], &[], dir.path())
            .unwrap_err();
        assert!(matches!(err, WasmHostError::Load(_)), "got: {err:?}");
    }

    #[test]
    fn preopen_with_dotdot_is_rejected_before_preopen() {
        // Defense-in-depth: `resolve_wasi_config` never emits a `..` host_dir,
        // but a hand-built config with one must be refused (a lexical
        // component scan, since `join(..)` + `starts_with` would NOT catch it).
        use tau_ports::target::{
            PreopenAccess, PreopenGranularity, ResolvedPreopen, WasiConfiguration,
        };
        let cfg = WasiConfiguration {
            allowed_hosts: tau_domain::package::host::HostSet::Exact(Default::default()),
            methods: None,
            preopens: vec![ResolvedPreopen {
                host_dir: "/../etc".to_string(),
                access: PreopenAccess::ReadOnly,
                granularity: PreopenGranularity::Exact,
                from: vec![],
            }],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        // Guard fires BEFORE any create_dir_all/preopen, so nothing is created
        // outside the sandbox.
        let escaped = dir.path().parent().unwrap().join("etc");
        let existed_before = escaped.exists();
        // `WasiCtx` isn't `Debug`, so match rather than `unwrap_err`.
        let err = match wasi_ctx_from_config(&cfg, dir.path()) {
            Ok(_) => panic!("escaping preopen was accepted"),
            Err(e) => e,
        };
        assert!(matches!(err, WasmHostError::WasiConfig(_)), "got: {err:?}");
        assert_eq!(
            escaped.exists(),
            existed_before,
            "guard must not create a dir outside the sandbox"
        );
    }

    /// DeterministicPorts must reproduce HostState's exact sequences —
    /// cross-checked against the legacy impl before Task 2 rewires it.
    #[test]
    fn deterministic_ports_matches_legacy_clock_and_prng() {
        let mut legacy = HostState::new(
            Box::new(DeterministicPorts::new(vec![]).0),
            empty_wasi_ctx(),
            empty_egress(),
        );
        let (mut ports, _emitted) = DeterministicPorts::new(vec![]);
        for _ in 0..4 {
            assert_eq!(ports.now_millis(), legacy.now_millis());
            assert_eq!(ports.next_u64(), legacy.next_u64());
        }
    }

    #[test]
    fn deterministic_ports_pops_cassette_in_order_and_exhausts_with_legacy_error() {
        let first = canned_response();
        let (mut ports, _emitted) = DeterministicPorts::new(vec![first]);
        let req = CompletionRequest::new("m".to_string());
        let resp = ports.complete(req.clone()).expect("first canned response");
        assert_eq!(resp.text, "");
        assert_eq!(
            ports.complete(req).unwrap_err(),
            "tau-wasm-host: no canned completion response left"
        );
    }

    #[test]
    fn deterministic_ports_rejects_malformed_canned_response() {
        let (mut ports, _emitted) = DeterministicPorts::new(vec!["not json".to_string()]);
        let err = ports.complete(CompletionRequest::new("m".to_string())).unwrap_err();
        assert!(err.contains("invalid canned CompletionResponse"), "got: {err}");
    }

    #[test]
    fn deterministic_ports_buffers_events_via_shared_handle() {
        let (mut ports, emitted) = DeterministicPorts::new(vec![]);
        ports.on_event("{\"RunStarted\":null}");
        ports.on_event("{\"RunCompleted\":{}}");
        assert_eq!(
            emitted.lock().unwrap().as_slice(),
            ["{\"RunStarted\":null}", "{\"RunCompleted\":{}}"]
        );
    }
}
