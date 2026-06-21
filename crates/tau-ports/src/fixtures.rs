//! Test fixtures for tau-ports. Gated behind the `test-fixtures` feature.
//!
//! Downstream crates depend via:
//!
//! ```toml
//! [dev-dependencies]
//! tau-ports = { workspace = true, features = ["test-fixtures"] }
//! ```
//!
//! Each mock implements its corresponding trait with configurable
//! canned responses and recorded invocations. Interior mutability is
//! provided by [`std::sync::Mutex`] so the mocks satisfy `Send + Sync`
//! and can be stored in the runtime's plugin registry.
//!
//! Mocks expose enough surface for tau-runtime, tau-pkg, and future
//! plugin-author tests to verify trait-driven behavior without spinning
//! up real LLM providers or database backends. Production builds
//! (without the feature) do not pull this code.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::sync::Mutex;
use std::time::SystemTime;

use tau_domain::{AgentInstanceId, Value};
use uuid::Uuid;

use tau_domain::{CapabilityShape, CapabilityShapeSet};

use crate::capability_gate::{
    CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe, CapabilityTier,
};
use crate::error::{CapabilityError, LlmError, StorageError, ToolError};
use crate::llm::{
    batch_to_stream, CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream,
    LlmBackend, LlmProviderMessage, StopReason, TokenUsage, ToolChoice, ToolSpec, ToolUse,
};
use crate::storage::{Key, Namespace, Storage};
use crate::tool::{SessionContext, Tool, ToolContent, ToolResult};

// ---------------------------------------------------------------------------
// Construction helpers for `#[non_exhaustive]` types
// ---------------------------------------------------------------------------
//
// External integration tests cannot construct `#[non_exhaustive]` types via
// struct-literal syntax (E0639). These helpers live here so that downstream
// crates (and tau-ports' own integration tests) can build canonical
// `CompletionResponse`, `ToolUse`, and `TokenUsage` values when the
// `test-fixtures` feature is enabled.

/// Build a [`CompletionResponse`] without struct-literal syntax. Used by
/// integration tests that can't construct `#[non_exhaustive]` types.
pub fn make_completion_response(
    text: String,
    tool_uses: Vec<ToolUse>,
    stop_reason: StopReason,
    usage: Option<TokenUsage>,
) -> CompletionResponse {
    CompletionResponse {
        text,
        tool_uses,
        stop_reason,
        usage,
    }
}

/// Build a [`ToolUse`] without struct-literal syntax.
pub fn make_tool_use(id: String, name: String, input: Value) -> ToolUse {
    ToolUse { id, name, input }
}

/// Build a [`TokenUsage`] without struct-literal syntax.
pub fn make_token_usage(input_tokens: u32, output_tokens: u32) -> TokenUsage {
    TokenUsage {
        input_tokens,
        output_tokens,
    }
}

/// Build a minimal [`CompletionRequest`] without struct-literal syntax.
///
/// Optional fields default to `None` / empty / [`ToolChoice::Auto`].
/// Used by integration tests that need to feed canonical requests to a
/// [`MockLlmBackend`] or similar.
pub fn make_completion_request(model: String) -> CompletionRequest {
    CompletionRequest {
        model,
        system: None,
        messages: Vec::<LlmProviderMessage>::new(),
        tools: Vec::<ToolSpec>::new(),
        max_tokens: None,
        temperature: None,
        top_p: None,
        seed: None,
        tool_choice: ToolChoice::Auto,
        stop_sequences: Vec::new(),
        provider_specific: BTreeMap::new(),
    }
}

/// Build a [`ToolSpec`] without struct-literal syntax.
pub fn make_tool_spec(name: String, description: String, input_schema: Value) -> ToolSpec {
    ToolSpec {
        name,
        description,
        input_schema,
    }
}

/// Build a [`ToolResult`] without struct-literal syntax.
pub fn make_tool_result(content: Vec<ToolContent>, is_error: bool) -> ToolResult {
    ToolResult { content, is_error }
}

/// Build a [`SessionContext`] without struct-literal syntax. Provided
/// for tests in tau-runtime and elsewhere; tau-ports callers should
/// use [`SessionContext::new`] in production code.
///
/// `granted_capabilities` defaults to empty. To set a grant, use the
/// builder: `make_session_context(...).with_granted_capabilities(caps)`.
pub fn make_session_context(
    agent_instance_id: AgentInstanceId,
    session_id: Uuid,
    deadline: Option<SystemTime>,
) -> SessionContext {
    SessionContext::new(agent_instance_id, session_id, deadline)
}

// ---------------------------------------------------------------------------
// scratch_dir
// ---------------------------------------------------------------------------

/// Create a labeled scratch [`tempfile::TempDir`] for test use.
///
/// Prefer this over `tempfile::TempDir::new().unwrap()`: the panic message
/// names the call site via `label`, and the directory itself is prefixed
/// `tau-test-<label>-` so stray scratch dirs are attributable when a test
/// leaks one (e.g. on hard process kill).
///
/// # Panics
///
/// Panics with a descriptive message (`label` included) if the underlying
/// `tempfile::Builder::tempdir()` call fails.
///
/// # Example
///
/// ```
/// use tau_ports::fixtures::scratch_dir;
///
/// let dir = scratch_dir("install-roundtrip");
/// // The directory lives on disk until `dir` is dropped.
/// assert!(dir.path().exists());
/// assert!(
///     dir.path()
///         .file_name()
///         .and_then(|s| s.to_str())
///         .map(|s| s.starts_with("tau-test-install-roundtrip-"))
///         .unwrap_or(false),
///     "scratch_dir should embed the label in the dir name; got {:?}",
///     dir.path()
/// );
/// ```
pub fn scratch_dir(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("tau-test-{label}-"))
        .tempdir()
        .unwrap_or_else(|e| panic!("failed to create scratch dir for '{label}': {e}"))
}

// ---------------------------------------------------------------------------
// MockLlmBackend
// ---------------------------------------------------------------------------

/// Mock [`LlmBackend`] with configurable canned responses.
///
/// Records each [`LlmBackend::complete`] / [`LlmBackend::stream`] invocation
/// for later inspection via [`MockLlmBackend::invocations`]. Interior
/// mutability is provided by [`Mutex`] so the mock is `Send + Sync`.
///
/// # Configuration
///
/// - [`MockLlmBackend::with_response`] sets the canned [`CompletionResponse`]
///   returned by `complete()`. If unset, `complete()` returns an empty
///   default response (no text, no tool uses, [`StopReason::EndTurn`]).
/// - [`MockLlmBackend::with_chunks`] sets the canned chunks emitted by
///   `stream()`. If unset, `stream()` derives chunks from the canned
///   response via [`batch_to_stream`].
///
/// # Example
///
/// ```
/// use tau_ports::fixtures::MockLlmBackend;
///
/// let backend = MockLlmBackend::new("mock-llm");
/// // ... drive backend.complete(...) / backend.stream(...) ...
/// let recorded = backend.invocations();
/// assert!(recorded.is_empty()); // none yet
/// ```
pub struct MockLlmBackend {
    name: String,
    state: Mutex<MockLlmState>,
}

struct MockLlmState {
    response: Option<CompletionResponse>,
    chunks: Option<Vec<CompletionChunk>>,
    invocations: Vec<CompletionRequest>,
}

impl MockLlmBackend {
    /// Create a fresh mock with no canned responses configured.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: Mutex::new(MockLlmState {
                response: None,
                chunks: None,
                invocations: Vec::new(),
            }),
        }
    }

    /// Set the canned response returned by [`LlmBackend::complete`].
    ///
    /// Also seeds [`LlmBackend::stream`] when no chunks have been
    /// configured: `stream()` then derives chunks from this response
    /// via [`batch_to_stream`].
    pub fn with_response(self, resp: CompletionResponse) -> Self {
        {
            let mut state = self.state.lock().expect("MockLlmBackend mutex poisoned");
            state.response = Some(resp);
        }
        self
    }

    /// Set the canned chunks emitted by [`LlmBackend::stream`].
    pub fn with_chunks(self, chunks: Vec<CompletionChunk>) -> Self {
        {
            let mut state = self.state.lock().expect("MockLlmBackend mutex poisoned");
            state.chunks = Some(chunks);
        }
        self
    }

    /// Read recorded invocations in the order they were issued.
    pub fn invocations(&self) -> Vec<CompletionRequest> {
        self.state
            .lock()
            .expect("MockLlmBackend mutex poisoned")
            .invocations
            .clone()
    }

    /// Number of times [`LlmBackend::complete`] or [`LlmBackend::stream`]
    /// has been invoked on this mock.
    pub fn invocation_count(&self) -> usize {
        self.state
            .lock()
            .expect("MockLlmBackend mutex poisoned")
            .invocations
            .len()
    }

    /// Assert the mock was invoked exactly `expected` times.
    ///
    /// Call this near the end of a test (after the code-under-test has
    /// finished using the mock) to make the test fail loudly when the
    /// mock was never called, or was called too many times. Counts both
    /// [`LlmBackend::complete`] and [`LlmBackend::stream`].
    ///
    /// `#[track_caller]` makes the panic point at the call site rather
    /// than into this helper.
    #[track_caller]
    pub fn verify_invocation_count(&self, expected: usize) {
        let actual = self.invocation_count();
        assert_eq!(
            actual, expected,
            "MockLlmBackend invocation count mismatch: expected {expected}, got {actual}"
        );
    }
}

impl LlmBackend for MockLlmBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let mut state = self.state.lock().expect("MockLlmBackend mutex poisoned");
        state.invocations.push(req);
        Ok(state.response.clone().unwrap_or(CompletionResponse {
            text: String::new(),
            tool_uses: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: None,
        }))
    }

    async fn stream(&self, req: CompletionRequest) -> Result<CompletionStream, LlmError> {
        let chunks = {
            let mut state = self.state.lock().expect("MockLlmBackend mutex poisoned");
            state.invocations.push(req);
            match state.chunks.clone() {
                Some(chunks) => chunks,
                None => {
                    // Fall back to batch_to_stream of the canned response.
                    let resp = state.response.clone().unwrap_or(CompletionResponse {
                        text: String::new(),
                        tool_uses: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: None,
                    });
                    return Ok(batch_to_stream(resp));
                }
            }
        };
        Ok(Box::pin(VecChunkStream {
            items: chunks.into_iter(),
        }))
    }
}

/// Adapter from a `Vec<CompletionChunk>` to a [`CompletionStream`]. Used
/// by [`MockLlmBackend::stream`] when explicit chunks are configured.
struct VecChunkStream {
    items: alloc::vec::IntoIter<CompletionChunk>,
}

impl futures_core::Stream for VecChunkStream {
    type Item = Result<CompletionChunk, LlmError>;

    fn poll_next(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::task::Poll::Ready(self.get_mut().items.next().map(Ok))
    }
}

// ---------------------------------------------------------------------------
// MockTool
// ---------------------------------------------------------------------------

/// Mock [`Tool`] that records invocations and returns a canned
/// [`ToolResult`] (or [`ToolError`]).
///
/// `Session = ()`: the mock is stateless. Configure via
/// [`MockTool::with_result`] (default success path) or
/// [`MockTool::with_error`] (error path; takes precedence over a
/// configured result).
///
/// # Example
///
/// ```text
/// // Illustrative; `ToolSpec` is `#[non_exhaustive]` so external doctests
/// // cannot construct it via struct-literal syntax. Use
/// // `tau_ports::fixtures::make_tool_spec` from the `test-fixtures` feature.
/// use tau_ports::fixtures::MockTool;
/// use tau_ports::llm::ToolSpec;
/// use tau_domain::Value;
///
/// let spec = ToolSpec {
///     name: "echo".into(),
///     description: "echo".into(),
///     input_schema: Value::Object(Default::default()),
/// };
/// let tool = MockTool::new("echo", spec);
/// ```
pub struct MockTool {
    name: String,
    schema: ToolSpec,
    state: Mutex<MockToolState>,
}

struct MockToolState {
    result: Option<ToolResult>,
    error: Option<ToolError>,
    invocations: Vec<Value>,
}

impl MockTool {
    /// Create a fresh mock with no canned outcome configured. Calling
    /// `invoke` on an unconfigured mock returns a default success
    /// response with empty content.
    pub fn new(name: &str, schema: ToolSpec) -> Self {
        Self {
            name: name.to_string(),
            schema,
            state: Mutex::new(MockToolState {
                result: None,
                error: None,
                invocations: Vec::new(),
            }),
        }
    }

    /// Set the canned [`ToolResult`] returned by [`Tool::invoke`] when
    /// no error is configured.
    pub fn with_result(self, result: ToolResult) -> Self {
        {
            let mut state = self.state.lock().expect("MockTool mutex poisoned");
            state.result = Some(result);
        }
        self
    }

    /// Configure the mock to return `Err(error)` from [`Tool::invoke`].
    /// Takes precedence over a result configured via
    /// [`MockTool::with_result`].
    pub fn with_error(self, error: ToolError) -> Self {
        {
            let mut state = self.state.lock().expect("MockTool mutex poisoned");
            state.error = Some(error);
        }
        self
    }

    /// Read recorded invocation arguments in the order they were
    /// issued.
    pub fn invocations(&self) -> Vec<Value> {
        self.state
            .lock()
            .expect("MockTool mutex poisoned")
            .invocations
            .clone()
    }
}

impl Tool for MockTool {
    type Session = ();

    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> ToolSpec {
        self.schema.clone()
    }

    async fn init(&self, _ctx: SessionContext) -> Result<Self::Session, ToolError> {
        Ok(())
    }

    async fn invoke(
        &self,
        _session: &mut Self::Session,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let mut state = self.state.lock().expect("MockTool mutex poisoned");
        state.invocations.push(args);
        if let Some(error) = state.error.clone() {
            return Err(error);
        }
        Ok(state.result.clone().unwrap_or(ToolResult {
            content: Vec::new(),
            is_error: false,
        }))
    }

    async fn teardown(&self, _session: Self::Session) -> Result<(), ToolError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MockStorage
// ---------------------------------------------------------------------------

/// Mock [`Storage`] backed by an in-memory `BTreeMap`.
///
/// All operations route through a [`Mutex`]-guarded map so the mock is
/// `Send + Sync`. Use this in tests that need a working KV store
/// without spinning up a real backend.
///
/// # Example
///
/// ```
/// use tau_ports::fixtures::MockStorage;
///
/// let storage = MockStorage::new("mem");
/// // ... drive storage.put(...) / storage.get(...) ...
/// # let _ = storage;
/// ```
pub struct MockStorage {
    name: String,
    inner: Mutex<BTreeMap<(Namespace, Key), Vec<u8>>>,
}

impl MockStorage {
    /// Create an empty in-memory storage mock.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Storage for MockStorage {
    fn name(&self) -> &str {
        &self.name
    }

    async fn get(&self, namespace: &Namespace, key: &Key) -> Result<Option<Vec<u8>>, StorageError> {
        let map = self.inner.lock().expect("MockStorage mutex poisoned");
        Ok(map.get(&(namespace.clone(), key.clone())).cloned())
    }

    async fn put(
        &self,
        namespace: &Namespace,
        key: &Key,
        value: &[u8],
    ) -> Result<(), StorageError> {
        let mut map = self.inner.lock().expect("MockStorage mutex poisoned");
        map.insert((namespace.clone(), key.clone()), value.to_vec());
        Ok(())
    }

    async fn delete(&self, namespace: &Namespace, key: &Key) -> Result<bool, StorageError> {
        let mut map = self.inner.lock().expect("MockStorage mutex poisoned");
        Ok(map.remove(&(namespace.clone(), key.clone())).is_some())
    }

    async fn list(&self, namespace: &Namespace, prefix: &str) -> Result<Vec<Key>, StorageError> {
        let map = self.inner.lock().expect("MockStorage mutex poisoned");
        Ok(map
            .iter()
            .filter(|((ns, _), _)| ns == namespace)
            .filter(|((_, k), _)| k.as_str().starts_with(prefix))
            .map(|((_, k), _)| k.clone())
            .collect())
    }
}

// ---------------------------------------------------------------------------
// MockCapabilityGate
// ---------------------------------------------------------------------------

/// Mock [`CapabilityGate`] adapter for tests. Reports `Available` with `Tier::None`,
/// supports every known [`CapabilityShape`] except [`CapabilityShape::Custom`],
/// and enforces nothing (all plans pass validate_plan for known shapes).
pub struct MockCapabilityGate {
    name: String,
}

impl MockCapabilityGate {
    /// Create a fresh mock capability gate.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl CapabilityGate for MockCapabilityGate {
    fn name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> CapabilityProbe {
        CapabilityProbe::Available {
            tier: CapabilityTier::None,
            details: "mock — no enforcement".into(),
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        let mut set = CapabilityShapeSet::new();
        set.insert(CapabilityShape::FilesystemRead);
        set.insert(CapabilityShape::FilesystemWrite);
        set.insert(CapabilityShape::ProcessExec);
        set.insert(CapabilityShape::NetworkHttp);
        set.insert(CapabilityShape::AgentSpawn);
        set
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        let supported = self.supported_shapes();
        for cap in &plan.capabilities {
            let shape = cap.required_shape();
            if !supported.contains(&shape) {
                return Err(CapabilityError::ShapeUnsupported { shape });
            }
        }
        Ok(())
    }
}

#[cfg(feature = "process")]
impl crate::ProcessCapabilityGate for MockCapabilityGate {
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        _cmd: &mut std::process::Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        self.validate_plan(plan)?;
        Ok(CapabilityHandle::noop())
    }
}

// ---------------------------------------------------------------------------
// CapabilityPlan / WorkingContext / ResourceLimits construction helpers
// ---------------------------------------------------------------------------
//
// `CapabilityPlan`, `WorkingContext`, and `ResourceLimits` are all
// `#[non_exhaustive]`. External crates cannot use struct-literal syntax to
// construct them. These helpers provide typed constructors so downstream test
// crates can build canonical values without the
// `serde_json::from_value(json!({...}))` round-trip workaround.

/// Build a [`CapabilityPlan`] from a capability list with no context or limits.
pub fn plan_from_capabilities(caps: Vec<tau_domain::Capability>) -> CapabilityPlan {
    CapabilityPlan::new(caps, None, None)
}

/// Build a [`CapabilityPlan`] from a capability list with a
/// [`WorkingContext`](crate::capability_gate::WorkingContext).
pub fn plan_with_context(
    caps: Vec<tau_domain::Capability>,
    ctx: crate::capability_gate::WorkingContext,
) -> CapabilityPlan {
    CapabilityPlan::new(caps, Some(ctx), None)
}

/// Build a [`WorkingContext`](crate::capability_gate::WorkingContext) from a
/// working directory and environment map.
pub fn working_context(
    working_dir: impl Into<std::path::PathBuf>,
    env: BTreeMap<String, String>,
) -> crate::capability_gate::WorkingContext {
    crate::capability_gate::WorkingContext {
        working_dir: Some(working_dir.into()),
        env,
    }
}

/// Build a [`ResourceLimits`](crate::capability_gate::ResourceLimits) from
/// optional memory and CPU-second limits.
///
/// `cpu_seconds` is `Option<u32>` (matching the field type in
/// [`crate::capability_gate::ResourceLimits`]).
pub fn resource_limits(
    memory_bytes: Option<u64>,
    cpu_seconds: Option<u32>,
) -> crate::capability_gate::ResourceLimits {
    crate::capability_gate::ResourceLimits {
        memory_bytes,
        cpu_seconds,
        wall_clock_seconds: None,
        max_subprocesses: None,
    }
}

#[cfg(test)]
mod mock_llm_tests {
    use super::*;

    #[tokio::test]
    async fn verify_invocation_count_passes_on_zero_when_unused() {
        let mock = MockLlmBackend::new("zero");
        assert_eq!(mock.invocation_count(), 0);
        mock.verify_invocation_count(0); // should not panic
    }

    #[tokio::test]
    async fn verify_invocation_count_tracks_complete_calls() {
        let mock = MockLlmBackend::new("counter");
        let req = make_completion_request("model-x".into());
        let _ = mock.complete(req.clone()).await.expect("complete ok");
        let _ = mock.complete(req).await.expect("complete ok");
        assert_eq!(mock.invocation_count(), 2);
        mock.verify_invocation_count(2); // should not panic
    }

    #[tokio::test]
    async fn verify_invocation_count_panics_on_mismatch() {
        let mock = MockLlmBackend::new("mismatch");
        // 0 invocations recorded; verifying against 1 must panic.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            mock.verify_invocation_count(1);
        }));
        assert!(
            result.is_err(),
            "verify_invocation_count must panic when actual != expected"
        );
    }
}

#[cfg(test)]
mod sandbox_v01_tests {
    use super::*;
    use alloc::vec;
    use tau_domain::{Capability, CapabilityShape};

    fn read_cap() -> Capability {
        tau_domain::fixtures::cap_fs_read(&["/tmp/**"])
    }

    #[tokio::test]
    async fn mock_probe_is_available() {
        let mock = MockCapabilityGate::new("mem");
        let probe = mock.probe().await;
        assert!(matches!(probe, CapabilityProbe::Available { .. }));
    }

    #[tokio::test]
    async fn mock_supports_all_known_shapes() {
        let mock = MockCapabilityGate::new("mem");
        let supported = mock.supported_shapes();
        assert!(supported.contains(&CapabilityShape::FilesystemRead));
        assert!(supported.contains(&CapabilityShape::FilesystemWrite));
        assert!(supported.contains(&CapabilityShape::ProcessExec));
        assert!(supported.contains(&CapabilityShape::NetworkHttp));
        assert!(supported.contains(&CapabilityShape::AgentSpawn));
    }

    #[tokio::test]
    async fn mock_validate_plan_accepts_known_shape() {
        let mock = MockCapabilityGate::new("mem");
        let plan = CapabilityPlan {
            capabilities: vec![read_cap()],
            context: None,
            limits: None,
        };
        assert!(mock.validate_plan(&plan).is_ok());
    }

    #[tokio::test]
    async fn mock_validate_plan_rejects_custom_shape() {
        let mock = MockCapabilityGate::new("mem");
        let plan = CapabilityPlan {
            capabilities: vec![Capability::Custom {
                name: "weird".into(),
                params: Default::default(),
            }],
            context: None,
            limits: None,
        };
        match mock.validate_plan(&plan) {
            Err(CapabilityError::ShapeUnsupported { shape }) => {
                assert!(matches!(shape, CapabilityShape::Custom { .. }));
            }
            other => panic!("expected ShapeUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn sandbox_handle_runs_cleanup_on_drop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        drop(CapabilityHandle::new(move || {
            f.store(true, Ordering::SeqCst)
        }));
        assert!(
            flag.load(Ordering::SeqCst),
            "cleanup closure must run on drop"
        );
    }

    #[test]
    fn sandbox_handle_noop_does_not_panic_on_drop() {
        drop(CapabilityHandle::noop());
    }

    #[tokio::test]
    async fn sandbox_tier_ordering() {
        assert!(CapabilityTier::Light < CapabilityTier::Strict);
        assert!(CapabilityTier::None < CapabilityTier::Light);
    }

    #[test]
    fn sandbox_error_unavailable_renders() {
        let e = CapabilityError::Unavailable {
            reason: "no kernel".into(),
        };
        assert!(format!("{e}").contains("unavailable"));
    }

    #[test]
    fn sandbox_error_shape_unsupported_renders() {
        let e = CapabilityError::ShapeUnsupported {
            shape: CapabilityShape::FilesystemRead,
        };
        assert!(format!("{e}").contains("unsupported shape"));
    }

    #[test]
    fn scratch_dir_creates_with_labeled_prefix() {
        let dir = scratch_dir("smoke");
        let name = dir
            .path()
            .file_name()
            .expect("tempdir path has a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("tau-test-smoke-"),
            "prefix not applied; got: {name}"
        );
        assert!(dir.path().exists(), "scratch dir should exist on disk");
    }
}

#[cfg(all(test, feature = "process"))]
mod process_tests {
    use super::*;
    use crate::ProcessCapabilityGate;
    use alloc::vec;

    #[tokio::test]
    async fn wrap_spawn_is_noop_when_plan_validates() {
        let mock = MockCapabilityGate::new("mem");
        let plan = plan_from_capabilities(vec![]);
        let mut cmd = std::process::Command::new("/bin/true");
        let handle = mock.wrap_spawn(&plan, &mut cmd).await;
        assert!(handle.is_ok());
    }
}

// ---------------------------------------------------------------------------
// MockCheckpointStore — in-memory CheckpointStore (ADR-0053)
// ---------------------------------------------------------------------------

use crate::orchestration::{CheckpointError, CheckpointStore, RunId, TurnCheckpoint};

/// In-memory [`CheckpointStore`] for tests.
///
/// Records every persisted [`TurnCheckpoint`] keyed by `(run_id, turn)` and
/// returns the highest-`turn` checkpoint from `load_latest`. Lets a
/// kill-and-resume test run entirely in-core: drive a run with this store,
/// inspect [`MockCheckpointStore::persisted_turns`], then construct a fresh
/// run that resumes from `load_latest` — no filesystem, no tokio host.
///
/// `log` is an append-only record of every `persist` call in order, including
/// mid-turn intermediate writes. Use [`MockCheckpointStore::all_persists`] to
/// assert that a per-tool checkpoint carried the expected `pending_tool_uses`.
#[derive(Default)]
pub struct MockCheckpointStore {
    by_run: Mutex<BTreeMap<RunId, BTreeMap<u32, TurnCheckpoint>>>,
    log: Mutex<Vec<TurnCheckpoint>>,
}

impl MockCheckpointStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The turn numbers committed for `run_id`, ascending. Test introspection.
    pub fn persisted_turns(&self, run_id: &RunId) -> Vec<u32> {
        self.by_run
            .lock()
            .expect("checkpoint mutex")
            .get(run_id)
            .map(|turns| turns.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Total checkpoints recorded across all runs. Test introspection.
    pub fn persist_count(&self) -> usize {
        self.by_run
            .lock()
            .expect("checkpoint mutex")
            .values()
            .map(|turns| turns.len())
            .sum()
    }

    /// Every `persist` call in chronological order, including mid-turn
    /// intermediate writes that are later overwritten in `by_run`. Use this
    /// to assert that a per-tool-call checkpoint carried the expected
    /// `pending_tool_uses` before the turn-boundary write cleared them.
    pub fn all_persists(&self) -> Vec<TurnCheckpoint> {
        self.log.lock().expect("log mutex").clone()
    }
}

impl CheckpointStore for MockCheckpointStore {
    fn persist(&self, ckpt: &TurnCheckpoint) -> Result<(), CheckpointError> {
        self.by_run
            .lock()
            .expect("checkpoint mutex")
            .entry(ckpt.run_id.clone())
            .or_default()
            .insert(ckpt.turn, ckpt.clone());
        self.log
            .lock()
            .expect("log mutex")
            .push(ckpt.clone());
        Ok(())
    }

    fn load_latest(&self, run_id: &RunId) -> Result<Option<TurnCheckpoint>, CheckpointError> {
        Ok(self
            .by_run
            .lock()
            .expect("checkpoint mutex")
            .get(run_id)
            .and_then(|turns| turns.values().next_back().cloned()))
    }
}
