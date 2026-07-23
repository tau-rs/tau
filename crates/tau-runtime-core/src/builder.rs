//! Runtime kernel and its builder.
//!
//! [`Runtime`] is the immutable kernel produced by
//! [`RuntimeBuilder::build`]. Plugin instances (LLM backends, tools,
//! storages) are registered on the builder, validated at `build()`
//! time, and stored in name-keyed registries on the resulting
//! `Runtime`. The registries are read-only post-`build()`: to add or
//! remove plugins, construct a new `Runtime`.
//!
//! # Dyn-compatibility shim
//!
//! `tau_ports::{LlmBackend, Tool, Storage}` use native `async fn in
//! trait` (per ADR-0003), which makes them **not** dyn-compatible
//! under Rust 1.93. The spec's literal `Arc<dyn LlmBackend>` doesn't
//! compile.
//!
//! tau-runtime resolves this by defining dyn-compatible wrapper
//! traits ([`DynLlmBackend`], [`DynTool`], [`DynStorage`],
//! [`DynCapabilityGate`]) with [`Box`]-returning futures, and a
//! blanket impl for any `T: LlmBackend + 'static` (etc.). Public
//! `with_*` builder methods take generics; the registry stores
//! `Arc<dyn Dyn*>`. This is the "boxes once at the dyn-cast boundary"
//! pattern called out in the tau-ports design doc §3.1.
//!
//! `DynProcessCapabilityGate` (the process-spawn extension) is defined
//! in `tau-runtime::process_gate` — it references `std::process::Command`
//! which is not available in `no_std` + alloc contexts. Phase β.1.4
//! folds it into `tau-runtime-tokio`.
//!
//! See `docs/superpowers/specs/2026-04-28-tau-runtime-design.md` §3.4
//! for the rest of the design rationale.

extern crate alloc;

use alloc::boxed::Box;
#[cfg(feature = "tool-validation")]
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;

use hashbrown::HashMap;

use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityPlan, CapabilityProbe, CompletionRequest,
    CompletionResponse, CompletionStream, Key, LlmBackend, LlmError, Namespace, SessionContext,
    Storage, StorageError, Tool, ToolError, ToolResult, ToolSpec,
};

use crate::error::{BuildError, PluginKind};

// ---------------------------------------------------------------------------
// Internal Registry type alias (hashbrown + foldhash)
// ---------------------------------------------------------------------------

/// Plugin-registry map type. Uses [`hashbrown::HashMap`] with
/// [`foldhash::quality::FixedState`] for deterministic, fast hashing
/// without `std`-only `RandomState`.
type Registry<V> = HashMap<String, V, foldhash::quality::FixedState>;

fn registry_with_capacity<V>(cap: usize) -> Registry<V> {
    Registry::with_capacity_and_hasher(cap, foldhash::quality::FixedState::default())
}

// ---------------------------------------------------------------------------
// Dyn-compatible wrapper traits
// ---------------------------------------------------------------------------

// Boxed futures are deliberately *not* `Send`-bound: the underlying
// `async fn in trait` methods on `tau_ports::{LlmBackend, Tool,
// Storage}` don't promise `Send`-ness in their RPITIT and there is no
// `trait_variant`-generated `Send` variant at v0.1. tau-runtime's
// dispatcher will adopt a `Send`-bounded variant once one exists; for
// now, the registry is dyn-compatible but the futures are
// single-thread.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Object-safe wrapper for [`LlmBackend`]. Used internally by
/// [`Runtime`] to store plugin instances in a `Registry`. Plugin
/// authors implement [`LlmBackend`] directly; the blanket impl below
/// handles the dyn-cast.
pub trait DynLlmBackend: Send + Sync {
    /// Plugin-visible name (matches [`LlmBackend::name`]).
    fn name(&self) -> &str;

    /// Boxed-future wrapper for [`LlmBackend::complete`].
    fn complete<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> BoxFuture<'a, Result<CompletionResponse, LlmError>>;

    /// Boxed-future wrapper for [`LlmBackend::stream`].
    fn stream<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> BoxFuture<'a, Result<CompletionStream, LlmError>>;
}

impl<T: LlmBackend + 'static> DynLlmBackend for T {
    fn name(&self) -> &str {
        LlmBackend::name(self)
    }

    fn complete<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> BoxFuture<'a, Result<CompletionResponse, LlmError>> {
        Box::pin(LlmBackend::complete(self, req))
    }

    fn stream<'a>(
        &'a self,
        req: CompletionRequest,
    ) -> BoxFuture<'a, Result<CompletionStream, LlmError>> {
        Box::pin(LlmBackend::stream(self, req))
    }
}

/// Object-safe wrapper for [`Tool<Session = ()>`]. v0.1 restricts to
/// stateless tools (`Session = ()`) per the spec; stateful tools are
/// reached via [`tau_ports::StatelessAdapter`] until a `DynTool`
/// extension lands (ADR-0006).
pub trait DynTool: Send + Sync {
    /// Plugin-visible name (matches [`Tool::name`]).
    fn name(&self) -> &str;

    /// JSON Schema describing the tool's input.
    fn schema(&self) -> ToolSpec;

    /// Capabilities the tool requires of the calling agent's package.
    fn capabilities(&self) -> &[tau_domain::Capability];

    /// Boxed-future wrapper for [`Tool::init`] (returns the empty
    /// session value `()` for stateless tools).
    fn init<'a>(&'a self, ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>>;

    /// Boxed-future wrapper for [`Tool::invoke`].
    fn invoke<'a>(
        &'a self,
        ctx: &'a SessionContext,
        session: &'a mut (),
        args: tau_domain::Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>>;

    /// Boxed-future wrapper for [`Tool::teardown`].
    fn teardown<'a>(&'a self, session: ()) -> BoxFuture<'a, Result<(), ToolError>>;
}

impl<T: Tool<Session = ()> + 'static> DynTool for T {
    fn name(&self) -> &str {
        Tool::name(self)
    }

    fn schema(&self) -> ToolSpec {
        Tool::schema(self)
    }

    fn capabilities(&self) -> &[tau_domain::Capability] {
        Tool::capabilities(self)
    }

    fn init<'a>(&'a self, ctx: SessionContext) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(Tool::init(self, ctx))
    }

    fn invoke<'a>(
        &'a self,
        _ctx: &'a SessionContext,
        session: &'a mut (),
        args: tau_domain::Value,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        // The in-process Tool trait's invoke takes (&mut Session, args).
        // The session is what was returned from init(ctx); plugins that
        // need ctx at invoke time stash it in their Session. This blanket
        // impl ignores the new ctx parameter — out-of-process plugins
        // reach the SessionContext via the IPC frame's encoded ctx.
        Box::pin(Tool::invoke(self, session, args))
    }

    fn teardown<'a>(&'a self, session: ()) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(Tool::teardown(self, session))
    }
}

/// Object-safe wrapper for [`Storage`].
pub trait DynStorage: Send + Sync {
    /// Plugin-visible name (matches [`Storage::name`]).
    fn name(&self) -> &str;

    /// Boxed-future wrapper for [`Storage::get`].
    fn get<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<Option<Vec<u8>>, StorageError>>;

    /// Boxed-future wrapper for [`Storage::put`].
    fn put<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
        value: &'a [u8],
    ) -> BoxFuture<'a, Result<(), StorageError>>;

    /// Boxed-future wrapper for [`Storage::delete`].
    fn delete<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<bool, StorageError>>;

    /// Boxed-future wrapper for [`Storage::list`].
    fn list<'a>(
        &'a self,
        namespace: &'a Namespace,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Key>, StorageError>>;
}

impl<T: Storage + 'static> DynStorage for T {
    fn name(&self) -> &str {
        Storage::name(self)
    }

    fn get<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<Option<Vec<u8>>, StorageError>> {
        Box::pin(Storage::get(self, namespace, key))
    }

    fn put<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
        value: &'a [u8],
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(Storage::put(self, namespace, key, value))
    }

    fn delete<'a>(
        &'a self,
        namespace: &'a Namespace,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<bool, StorageError>> {
        Box::pin(Storage::delete(self, namespace, key))
    }

    fn list<'a>(
        &'a self,
        namespace: &'a Namespace,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Key>, StorageError>> {
        Box::pin(Storage::list(self, namespace, prefix))
    }
}

/// Object-safe wrapper of [`CapabilityGate`] (the universal four
/// methods). Stored in registries that don't care about process
/// extensions (wasm host, MCU, MCP facilitator).
///
/// The process-spawn extension (`DynProcessCapabilityGate`) lives in
/// `tau_runtime_tokio::process_gate` and depends on `std::process::Command`.
pub trait DynCapabilityGate: Send + Sync {
    /// Plugin-visible name.
    fn name(&self) -> &str;
    /// Boxed-future wrapper for [`CapabilityGate::probe`].
    fn probe<'a>(&'a self) -> BoxFuture<'a, CapabilityProbe>;
    /// Delegate to [`CapabilityGate::supported_shapes`].
    fn supported_shapes(&self) -> tau_domain::CapabilityShapeSet;
    /// Delegate to [`CapabilityGate::validate_plan`].
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError>;
}

impl<T: CapabilityGate + 'static> DynCapabilityGate for T {
    fn name(&self) -> &str {
        CapabilityGate::name(self)
    }

    fn probe<'a>(&'a self) -> BoxFuture<'a, CapabilityProbe> {
        Box::pin(CapabilityGate::probe(self))
    }

    fn supported_shapes(&self) -> tau_domain::CapabilityShapeSet {
        CapabilityGate::supported_shapes(self)
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        CapabilityGate::validate_plan(self, plan)
    }
}

// ---------------------------------------------------------------------------
// Runtime + RuntimeBuilder
// ---------------------------------------------------------------------------

/// The kernel. Build with [`Runtime::builder`].
///
/// Plugin registries are immutable post-[`RuntimeBuilder::build`]. To
/// add or remove plugins, construct a new `Runtime`.
///
/// # Example
///
/// ```rust,ignore
/// // `Runtime` is `#[non_exhaustive]`; doctests can't construct via
/// // struct-literal syntax, so this example is illustrative only.
/// use tau_runtime_core::Runtime;
/// use tau_ports::fixtures::MockLlmBackend;
///
/// let runtime = Runtime::builder()
///     .with_llm_backend(MockLlmBackend::new("gpt-4"))
///     .build()
///     .expect("build runtime");
/// ```
#[non_exhaustive]
pub struct Runtime {
    pub(crate) llm_backends: Registry<Arc<dyn DynLlmBackend>>,
    pub(crate) tools: Registry<Arc<dyn DynTool>>,
    /// Pre-compiled input_schema validators, keyed by tool name. One
    /// entry per registered tool (in 1:1 correspondence with `tools`).
    /// Built once at `RuntimeBuilder::build()` per ADR-0010.
    #[cfg(feature = "tool-validation")]
    pub(crate) tool_validators: Registry<crate::tool_args::ToolArgsValidator>,
    #[allow(dead_code)]
    pub(crate) storages: Registry<Arc<dyn DynStorage>>,
    // sandboxes reserved for forward compat (not used at v0.1).
}

impl core::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut s = f.debug_struct("Runtime");
        s.field(
            "llm_backends",
            &self.llm_backends.keys().collect::<Vec<_>>(),
        );
        s.field("tools", &self.tools.keys().collect::<Vec<_>>());
        #[cfg(feature = "tool-validation")]
        s.field(
            "tool_validators",
            &self.tool_validators.keys().collect::<Vec<_>>(),
        );
        s.field("storages", &self.storages.keys().collect::<Vec<_>>());
        s.finish()
    }
}

impl Runtime {
    /// Construct a fresh [`RuntimeBuilder`].
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::Runtime;
    /// use tau_ports::fixtures::MockLlmBackend;
    ///
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .build()
    ///     .expect("build runtime");
    /// assert!(format!("{runtime:?}").contains("test-pkg"));
    /// ```
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    /// Read-only access to the LLM-backend registry. Used by dispatch
    /// resolution helpers (Task 9) and the run loop (Task 10).
    pub fn llm_backends(&self) -> &Registry<Arc<dyn DynLlmBackend>> {
        &self.llm_backends
    }

    /// Read-only access to the tool registry. Used by dispatch
    /// resolution helpers (Task 9) and the run loop (Task 10).
    pub fn tools(&self) -> &Registry<Arc<dyn DynTool>> {
        &self.tools
    }

    /// Read-only access to the per-tool input_schema validators. Used
    /// by the run loop's call-site integration in `run.rs` (replaces
    /// the v0.1 `deserialize_tool_args` passthrough). Realizes ADR-0010.
    #[cfg(feature = "tool-validation")]
    pub fn tool_validators(&self) -> &Registry<crate::tool_args::ToolArgsValidator> {
        &self.tool_validators
    }

    /// Read-only access to the storage registry. Reserved for future
    /// dispatch use — at v0.1 nothing in the kernel routes through
    /// storage from the run loop.
    pub fn storages(&self) -> &Registry<Arc<dyn DynStorage>> {
        &self.storages
    }
}

/// Builder for [`Runtime`]. Plugin instances accumulate via the
/// `with_*` methods; [`RuntimeBuilder::build`] validates invariants
/// and finalizes the registries.
///
/// # Example
///
/// ```rust,ignore
/// // `RuntimeBuilder` is `#[non_exhaustive]`; doctests can't construct
/// // it via struct-literal syntax. Use [`Runtime::builder`] in
/// // production code.
/// use tau_runtime_core::Runtime;
/// use tau_ports::fixtures::MockLlmBackend;
///
/// let runtime = Runtime::builder()
///     .with_llm_backend(MockLlmBackend::new("gpt-4"))
///     .build()
///     .expect("build runtime");
/// ```
#[non_exhaustive]
#[derive(Default)]
pub struct RuntimeBuilder {
    pub(crate) llm_backends: Vec<Arc<dyn DynLlmBackend>>,
    pub(crate) tools: Vec<Arc<dyn DynTool>>,
    pub(crate) storages: Vec<Arc<dyn DynStorage>>,
}

impl RuntimeBuilder {
    /// Register an [`LlmBackend`] plugin instance. Multiple backends
    /// may be registered as long as their [`LlmBackend::name`] values
    /// are unique; collisions are reported by [`RuntimeBuilder::build`].
    ///
    /// **Deviation from spec:** the spec writes `Box<dyn LlmBackend>`,
    /// but `LlmBackend`'s native `async fn in trait` is not
    /// dyn-compatible. Accepting a generic `L: LlmBackend + 'static`
    /// keeps the public API ergonomic; the builder boxes through
    /// [`DynLlmBackend`] internally.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::Runtime;
    /// use tau_ports::fixtures::MockLlmBackend;
    ///
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("backend-a"))
    ///     .with_llm_backend(MockLlmBackend::new("backend-b"))
    ///     .build()
    ///     .expect("both backends registered");
    /// assert!(format!("{runtime:?}").contains("backend-a"));
    /// assert!(format!("{runtime:?}").contains("backend-b"));
    /// ```
    pub fn with_llm_backend<L: LlmBackend + 'static>(mut self, backend: L) -> Self {
        self.llm_backends.push(Arc::new(backend));
        self
    }

    /// Register a [`Tool`] plugin instance with `Session = ()`.
    /// Multiple tools may be registered as long as their
    /// [`Tool::name`] values are unique; collisions are reported by
    /// [`RuntimeBuilder::build`].
    ///
    /// **Deviation from spec:** see [`RuntimeBuilder::with_llm_backend`]
    /// for the dyn-compatibility rationale; the same applies here.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::Runtime;
    /// use tau_ports::fixtures::{MockLlmBackend, MockTool, make_tool_spec};
    /// use tau_domain::Value;
    ///
    /// let spec = make_tool_spec(
    ///     "echo".into(), "echo tool".into(), Value::Object(Default::default()),
    /// );
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .with_tool(MockTool::new("echo", spec))
    ///     .build()
    ///     .expect("tool registered");
    /// assert!(format!("{runtime:?}").contains("echo"));
    /// ```
    pub fn with_tool<T: Tool<Session = ()> + 'static>(mut self, tool: T) -> Self {
        self.tools.push(Arc::new(tool));
        self
    }

    /// Register a [`Storage`] plugin instance. Multiple storages may
    /// be registered as long as their [`Storage::name`] values are
    /// unique; collisions are reported by [`RuntimeBuilder::build`].
    ///
    /// **Deviation from spec:** see [`RuntimeBuilder::with_llm_backend`]
    /// for the dyn-compatibility rationale; the same applies here.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::Runtime;
    /// use tau_ports::fixtures::{MockLlmBackend, MockStorage};
    ///
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .with_storage(MockStorage::new("mem"))
    ///     .build()
    ///     .expect("storage registered");
    /// assert!(format!("{runtime:?}").contains("mem"));
    /// ```
    pub fn with_storage<S: Storage + 'static>(mut self, storage: S) -> Self {
        self.storages.push(Arc::new(storage));
        self
    }

    /// Register a pre-boxed [`Arc<dyn DynLlmBackend>`] instance.
    ///
    /// This is the entry point used by the plugin host: the
    /// `tau_runtime_tokio::plugin_host::load_llm_backend` return type is
    /// exactly `Arc<dyn DynLlmBackend>` because the IPC adapter
    /// (`IpcLlmBackend`) only implements [`DynLlmBackend`]'s
    /// dyn-compatible signature, not the native [`LlmBackend`] trait.
    /// See `crate::builder` module-level docs for the rationale.
    ///
    /// In-process plugins continue to use [`with_llm_backend`] (which
    /// takes a generic `L: LlmBackend`); IPC-loaded plugins funnel
    /// through this method.
    ///
    /// [`with_llm_backend`]: RuntimeBuilder::with_llm_backend
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use tau_runtime_core::{Runtime, builder::DynLlmBackend};
    /// use tau_ports::fixtures::MockLlmBackend;
    ///
    /// // Simulate the IPC path: obtain an Arc<dyn DynLlmBackend>.
    /// let backend: Arc<dyn DynLlmBackend> = Arc::new(MockLlmBackend::new("ipc-backend"));
    /// let runtime = Runtime::builder()
    ///     .with_dyn_llm_backend(backend)
    ///     .build()
    ///     .expect("dyn backend registered");
    /// assert!(format!("{runtime:?}").contains("ipc-backend"));
    /// ```
    pub fn with_dyn_llm_backend(mut self, backend: Arc<dyn DynLlmBackend>) -> Self {
        self.llm_backends.push(backend);
        self
    }

    /// Register a pre-boxed [`Arc<dyn DynTool>`]. Mirrors
    /// [`with_dyn_llm_backend`] for the tool port.
    ///
    /// [`with_dyn_llm_backend`]: RuntimeBuilder::with_dyn_llm_backend
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use tau_runtime_core::{Runtime, builder::DynTool};
    /// use tau_ports::fixtures::{MockLlmBackend, MockTool, make_tool_spec};
    /// use tau_domain::Value;
    ///
    /// let spec = make_tool_spec(
    ///     "greet".into(), "greet tool".into(), Value::Object(Default::default()),
    /// );
    /// let tool: Arc<dyn DynTool> = Arc::new(MockTool::new("greet", spec));
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .with_dyn_tool(tool)
    ///     .build()
    ///     .expect("dyn tool registered");
    /// assert!(format!("{runtime:?}").contains("greet"));
    /// ```
    pub fn with_dyn_tool(mut self, tool: Arc<dyn DynTool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Register a pre-boxed [`Arc<dyn DynStorage>`]. Mirrors
    /// [`with_dyn_llm_backend`] for the storage port.
    ///
    /// [`with_dyn_llm_backend`]: RuntimeBuilder::with_dyn_llm_backend
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use tau_runtime_core::{Runtime, builder::DynStorage};
    /// use tau_ports::fixtures::{MockLlmBackend, MockStorage};
    ///
    /// let storage: Arc<dyn DynStorage> = Arc::new(MockStorage::new("mem"));
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .with_dyn_storage(storage)
    ///     .build()
    ///     .expect("dyn storage registered");
    /// assert!(format!("{runtime:?}").contains("mem"));
    /// ```
    pub fn with_dyn_storage(mut self, storage: Arc<dyn DynStorage>) -> Self {
        self.storages.push(storage);
        self
    }

    /// Validate registrations and produce a [`Runtime`].
    ///
    /// Validation:
    /// - At least one LLM backend must be registered
    ///   ([`BuildError::NoLlmBackend`] otherwise).
    /// - No name collisions within a kind
    ///   ([`BuildError::NameCollision`] otherwise).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::{Runtime, BuildError};
    /// use tau_ports::fixtures::MockLlmBackend;
    ///
    /// // Success path.
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .build()
    ///     .expect("build with one backend succeeds");
    /// assert!(format!("{runtime:?}").contains("test-pkg"));
    ///
    /// // Failure path — no backends.
    /// let err = Runtime::builder().build().unwrap_err();
    /// assert!(matches!(err, BuildError::NoLlmBackend));
    /// ```
    pub fn build(self) -> Result<Runtime, BuildError> {
        if self.llm_backends.is_empty() {
            return Err(BuildError::NoLlmBackend);
        }
        let llm_backends = collect_llm_backends_by_name(self.llm_backends)?;
        #[cfg(feature = "tool-validation")]
        let (tools, tool_validators) = collect_tools_by_name(self.tools)?;
        #[cfg(not(feature = "tool-validation"))]
        let tools = collect_tools_by_name(self.tools)?;
        let storages = collect_storages_by_name(self.storages)?;
        Ok(Runtime {
            llm_backends,
            tools,
            #[cfg(feature = "tool-validation")]
            tool_validators,
            storages,
        })
    }

    /// Build a [`Runtime`] that may have zero LLM backends.
    ///
    /// Unlike [`Self::build`], this variant does **not** reject an empty backend
    /// set. Use it in serve mode when the project tau.toml declares no agents
    /// (the serve process still accepts `meta.handshake` and `meta.ping`;
    /// any `runtime.run` call will return `-32010 UNKNOWN_AGENT` because no
    /// agents are registered, which is correct behaviour).
    ///
    /// Name collisions within a kind are still rejected.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_core::Runtime;
    ///
    /// // Succeeds even with zero backends (serve-mode use case).
    /// let runtime = Runtime::builder()
    ///     .build_allow_empty()
    ///     .expect("empty runtime is valid in serve mode");
    /// assert!(format!("{runtime:?}").contains("llm_backends"));
    /// ```
    pub fn build_allow_empty(self) -> Result<Runtime, BuildError> {
        let llm_backends = collect_llm_backends_by_name(self.llm_backends)?;
        #[cfg(feature = "tool-validation")]
        let (tools, tool_validators) = collect_tools_by_name(self.tools)?;
        #[cfg(not(feature = "tool-validation"))]
        let tools = collect_tools_by_name(self.tools)?;
        let storages = collect_storages_by_name(self.storages)?;
        Ok(Runtime {
            llm_backends,
            tools,
            #[cfg(feature = "tool-validation")]
            tool_validators,
            storages,
        })
    }
}

// Three separate collectors instead of one generic helper: closing
// over `?Sized` `dyn Trait` values fights the type system more than
// the duplication is worth at v0.1.

fn collect_llm_backends_by_name(
    backends: Vec<Arc<dyn DynLlmBackend>>,
) -> Result<Registry<Arc<dyn DynLlmBackend>>, BuildError> {
    let mut map = registry_with_capacity(backends.len());
    for backend in backends {
        let name = backend.name().to_string();
        if map.contains_key(&name) {
            return Err(BuildError::NameCollision {
                kind: PluginKind::LlmBackend,
                name,
            });
        }
        map.insert(name, backend);
    }
    Ok(map)
}

// The return type carries two parallel maps; a type alias would be
// private-implementation detail noise with no benefit at the call site.
#[cfg(feature = "tool-validation")]
#[allow(clippy::type_complexity)]
fn collect_tools_by_name(
    tools: Vec<Arc<dyn DynTool>>,
) -> Result<
    (
        Registry<Arc<dyn DynTool>>,
        Registry<crate::tool_args::ToolArgsValidator>,
    ),
    BuildError,
> {
    let mut tool_map: Registry<Arc<dyn DynTool>> = registry_with_capacity(tools.len());
    let mut validator_map: Registry<crate::tool_args::ToolArgsValidator> =
        registry_with_capacity(tools.len());
    for tool in tools {
        let name = tool.name().to_string();
        if tool_map.contains_key(&name) {
            return Err(BuildError::NameCollision {
                kind: PluginKind::Tool,
                name,
            });
        }
        // Compile the input_schema once at build time; failure surfaces
        // as BuildError::ToolSchemaInvalid before any LLM round-trip.
        let spec = tool.schema();
        let validator =
            crate::tool_args::ToolArgsValidator::compile(&spec.input_schema).map_err(|e| {
                BuildError::ToolSchemaInvalid {
                    tool_name: name.clone(),
                    detail: format!("{}; excerpt: {}", e.kind, e.schema_excerpt),
                }
            })?;
        tool_map.insert(name.clone(), tool);
        validator_map.insert(name, validator);
    }
    Ok((tool_map, validator_map))
}

#[cfg(not(feature = "tool-validation"))]
fn collect_tools_by_name(
    tools: Vec<Arc<dyn DynTool>>,
) -> Result<Registry<Arc<dyn DynTool>>, BuildError> {
    let mut tool_map: Registry<Arc<dyn DynTool>> = registry_with_capacity(tools.len());
    for tool in tools {
        let name = tool.name().to_string();
        if tool_map.contains_key(&name) {
            return Err(BuildError::NameCollision {
                kind: PluginKind::Tool,
                name,
            });
        }
        tool_map.insert(name, tool);
    }
    Ok(tool_map)
}

fn collect_storages_by_name(
    storages: Vec<Arc<dyn DynStorage>>,
) -> Result<Registry<Arc<dyn DynStorage>>, BuildError> {
    let mut map = registry_with_capacity(storages.len());
    for storage in storages {
        let name = storage.name().to_string();
        if map.contains_key(&name) {
            return Err(BuildError::NameCollision {
                kind: PluginKind::Storage,
                name,
            });
        }
        map.insert(name, storage);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::vec;
    use tau_domain::Value;
    use tau_ports::fixtures::{make_tool_spec, MockLlmBackend, MockStorage, MockTool};

    fn empty_tool_spec(name: &str) -> tau_ports::ToolSpec {
        make_tool_spec(
            name.to_string(),
            "mock tool".to_string(),
            Value::Object(Default::default()),
        )
    }

    #[test]
    fn build_with_no_llm_backend_returns_no_llm_backend() {
        let result = Runtime::builder().build();
        assert!(
            matches!(result, Err(BuildError::NoLlmBackend)),
            "expected NoLlmBackend, got Ok or other error"
        );
    }

    #[test]
    fn build_with_two_llms_same_name_returns_collision() {
        let result = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("dup"))
            .with_llm_backend(MockLlmBackend::new("dup"))
            .build();

        let Err(BuildError::NameCollision { kind, name, .. }) = result else {
            panic!("expected NameCollision, got Ok or other error")
        };
        assert_eq!(kind, PluginKind::LlmBackend);
        assert_eq!(name, "dup");
    }

    #[test]
    fn build_with_unique_llms_succeeds() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_llm_backend(MockLlmBackend::new("claude"))
            .build()
            .expect("build runtime");

        let backends = runtime.llm_backends();
        assert_eq!(backends.len(), 2);
        assert!(backends.contains_key("gpt-4"));
        assert!(backends.contains_key("claude"));
    }

    #[test]
    fn build_with_zero_tools_succeeds() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        assert!(runtime.tools().is_empty());
    }

    #[test]
    fn build_with_two_tools_same_name_returns_collision() {
        let result = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_tool(MockTool::new("duped", empty_tool_spec("duped")))
            .with_tool(MockTool::new("duped", empty_tool_spec("duped")))
            .build();

        let Err(BuildError::NameCollision { kind, name, .. }) = result else {
            panic!("expected NameCollision, got Ok or other error")
        };
        assert_eq!(kind, PluginKind::Tool);
        assert_eq!(name, "duped");
    }

    #[test]
    fn build_with_zero_storages_succeeds() {
        let runtime = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .build()
            .expect("build runtime");

        assert!(runtime.storages().is_empty());
    }

    #[test]
    fn build_with_two_storages_same_name_returns_collision() {
        let result = Runtime::builder()
            .with_llm_backend(MockLlmBackend::new("gpt-4"))
            .with_storage(MockStorage::new("mem"))
            .with_storage(MockStorage::new("mem"))
            .build();

        let Err(BuildError::NameCollision { kind, name, .. }) = result else {
            panic!("expected NameCollision, got Ok or other error")
        };
        assert_eq!(kind, PluginKind::Storage);
        assert_eq!(name, "mem");
    }

    /// A test-only DynTool whose schema we control — used to test
    /// build-time schema validation without touching the existing
    /// production plugins.
    struct TestSchemaTool {
        name: &'static str,
        schema_value: tau_domain::Value,
    }

    impl DynTool for TestSchemaTool {
        fn name(&self) -> &str {
            self.name
        }

        fn schema(&self) -> tau_ports::ToolSpec {
            tau_ports::fixtures::make_tool_spec(
                self.name.into(),
                "test".into(),
                self.schema_value.clone(),
            )
        }

        fn capabilities(&self) -> &[tau_domain::Capability] {
            &[]
        }

        fn init<'a>(
            &'a self,
            _ctx: tau_ports::SessionContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), tau_ports::ToolError>> + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn invoke<'a>(
            &'a self,
            _ctx: &'a tau_ports::SessionContext,
            _session: &'a mut (),
            _args: tau_domain::Value,
        ) -> Pin<Box<dyn Future<Output = Result<tau_ports::ToolResult, tau_ports::ToolError>> + 'a>>
        {
            Box::pin(async { Ok(tau_ports::fixtures::make_tool_result(vec![], false)) })
        }

        fn teardown<'a>(
            &'a self,
            _session: (),
        ) -> Pin<Box<dyn Future<Output = Result<(), tau_ports::ToolError>> + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn schema_value(json: serde_json::Value) -> tau_domain::Value {
        let s = serde_json::to_string(&json).expect("schema serializes");
        serde_json::from_str(&s).expect("schema round-trips")
    }

    fn mock_llm() -> tau_ports::fixtures::MockLlmBackend {
        tau_ports::fixtures::MockLlmBackend::new("mock-llm")
    }

    #[cfg(feature = "tool-validation")]
    #[test]
    fn build_compiles_each_tools_input_schema() {
        let tool = TestSchemaTool {
            name: "echo",
            schema_value: schema_value(serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })),
        };
        let runtime = Runtime::builder()
            .with_dyn_tool(Arc::new(tool))
            .with_llm_backend(mock_llm())
            .build()
            .expect("build succeeds with valid schema");
        assert!(
            runtime.tool_validators().contains_key("echo"),
            "validator stored under tool name"
        );
    }

    #[cfg(feature = "tool-validation")]
    #[test]
    fn build_rejects_tool_with_malformed_schema() {
        let tool = TestSchemaTool {
            name: "broken",
            schema_value: schema_value(serde_json::json!({ "type": "objectt" })), // typo
        };
        let err = Runtime::builder()
            .with_dyn_tool(Arc::new(tool))
            .with_llm_backend(mock_llm())
            .build()
            .unwrap_err();
        let BuildError::ToolSchemaInvalid { tool_name, detail } = err else {
            panic!("expected BuildError::ToolSchemaInvalid, got: {err:?}");
        };
        assert_eq!(tool_name, "broken");
        assert!(
            detail.contains("type") || detail.contains("unsupported"),
            "detail: {detail}"
        );
    }

    #[cfg(feature = "tool-validation")]
    #[test]
    fn unsupported_schema_keyword_fails_build_with_named_keyword() {
        // `pattern` is not supported in v1 of ToolArgsValidator; the error
        // detail must name the offending keyword so `tau check` can surface it.
        let tool = TestSchemaTool {
            name: "patterned",
            schema_value: schema_value(serde_json::json!({ "type": "string", "pattern": "x" })),
        };
        let err = Runtime::builder()
            .with_dyn_tool(Arc::new(tool))
            .with_llm_backend(mock_llm())
            .build()
            .expect_err("pattern is an unsupported keyword");
        let BuildError::ToolSchemaInvalid { detail, .. } = err else {
            panic!("expected BuildError::ToolSchemaInvalid, got: {err:?}");
        };
        assert!(
            detail.contains("pattern"),
            "detail should name the unsupported keyword; got: {detail}"
        );
    }

    #[cfg(feature = "tool-validation")]
    #[test]
    fn build_handles_empty_schema_as_opt_out() {
        let tool = TestSchemaTool {
            name: "any-args",
            schema_value: schema_value(serde_json::json!({})),
        };
        let runtime = Runtime::builder()
            .with_dyn_tool(Arc::new(tool))
            .with_llm_backend(mock_llm())
            .build()
            .expect("build succeeds with empty schema");
        assert!(
            runtime.tool_validators().contains_key("any-args"),
            "validator stored even on opt-out"
        );
    }
}
