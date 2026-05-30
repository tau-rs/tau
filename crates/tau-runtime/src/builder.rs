//! Builder for tau-runtime. Wraps `tau_runtime_core::builder` types with
//! tokio-shell extensions.
//!
//! `CoreRuntime` and `RuntimeBuilder` from `tau-runtime-core` carry the
//! executor-agnostic plugin registries. This module:
//!
//! - Re-exports the Dyn* wrapper traits from core (including the process
//!   extension [`DynProcessCapabilityGate`] from this crate's
//!   `process_gate` module).
//! - Re-exports [`RuntimeBuilder`] from core so callers can use
//!   `Runtime::builder()` as before.
//! - Defines a newtype `Runtime` that wraps `tau_runtime_core::Runtime`
//!   and `Deref`s to it; this allows the existing `impl Runtime { ... }`
//!   blocks in `run.rs`, `dispatch.rs`, and this file to define inherent
//!   methods on a type owned by tau-runtime.
//!
//! The streaming entry points `run_streaming` / `run_streaming_with_history`
//! require tokio-shell types and live in `impl Runtime` below.

pub use tau_runtime_core::builder::{
    DynCapabilityGate, DynLlmBackend, DynStorage, DynTool, RuntimeBuilder,
};
pub use crate::process_gate::DynProcessCapabilityGate;

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use tau_ports::ToolSpec;
use tau_domain::Capability;
use tracing::{debug, info, warn};

use crate::capability::check_capabilities;
use crate::run::narrowed_capability_for_session;

/// The tokio-shell runtime kernel. Wraps [`tau_runtime_core::Runtime`].
///
/// Build with [`Runtime::builder`].
///
/// Plugin registries are immutable post-[`RuntimeBuilder::build`]. To
/// add or remove plugins, construct a new `Runtime`.
///
/// # Example
///
/// ```rust,ignore
/// use tau_runtime::Runtime;
/// use tau_ports::fixtures::MockLlmBackend;
///
/// let runtime = Runtime::builder()
///     .with_llm_backend(MockLlmBackend::new("gpt-4"))
///     .build()
///     .expect("build runtime");
/// ```
pub struct Runtime(pub(crate) tau_runtime_core::Runtime);

impl Deref for Runtime {
    type Target = tau_runtime_core::Runtime;

    fn deref(&self) -> &tau_runtime_core::Runtime {
        &self.0
    }
}

impl DerefMut for Runtime {
    fn deref_mut(&mut self) -> &mut tau_runtime_core::Runtime {
        &mut self.0
    }
}

impl core::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl Runtime {
    /// Construct a fresh [`RuntimeBuilder`].
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::Runtime;
    /// use tau_ports::fixtures::MockLlmBackend;
    ///
    /// let runtime = Runtime::builder()
    ///     .with_llm_backend(MockLlmBackend::new("test-pkg"))
    ///     .build()
    ///     .expect("build runtime");
    /// assert!(format!("{runtime:?}").contains("test-pkg"));
    /// ```
    pub fn builder() -> TauRuntimeBuilder {
        TauRuntimeBuilder(tau_runtime_core::builder::RuntimeBuilder::default())
    }
}

/// Builder for [`Runtime`]. Wraps [`tau_runtime_core::builder::RuntimeBuilder`].
pub struct TauRuntimeBuilder(tau_runtime_core::builder::RuntimeBuilder);

impl TauRuntimeBuilder {
    /// Register an [`tau_ports::LlmBackend`] plugin instance.
    pub fn with_llm_backend<L: tau_ports::LlmBackend + 'static>(self, backend: L) -> Self {
        Self(self.0.with_llm_backend(backend))
    }

    /// Register a [`tau_ports::Tool`] plugin instance with `Session = ()`.
    pub fn with_tool<T: tau_ports::Tool<Session = ()> + 'static>(self, tool: T) -> Self {
        Self(self.0.with_tool(tool))
    }

    /// Register a [`tau_ports::Storage`] plugin instance.
    pub fn with_storage<S: tau_ports::Storage + 'static>(self, storage: S) -> Self {
        Self(self.0.with_storage(storage))
    }

    /// Register a pre-boxed [`Arc<dyn DynLlmBackend>`] instance.
    pub fn with_dyn_llm_backend(self, backend: Arc<dyn DynLlmBackend>) -> Self {
        Self(self.0.with_dyn_llm_backend(backend))
    }

    /// Register a pre-boxed [`Arc<dyn DynTool>`].
    pub fn with_dyn_tool(self, tool: Arc<dyn DynTool>) -> Self {
        Self(self.0.with_dyn_tool(tool))
    }

    /// Register a pre-boxed [`Arc<dyn DynStorage>`].
    pub fn with_dyn_storage(self, storage: Arc<dyn DynStorage>) -> Self {
        Self(self.0.with_dyn_storage(storage))
    }

    /// Validate registrations and produce a [`Runtime`].
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::{Runtime, BuildError};
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
    pub fn build(self) -> Result<Runtime, crate::BuildError> {
        self.0.build().map(Runtime)
    }

    /// Build a [`Runtime`] that may have zero LLM backends (serve-mode).
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime::Runtime;
    ///
    /// // Succeeds even with zero backends (serve-mode use case).
    /// let runtime = Runtime::builder()
    ///     .build_allow_empty()
    ///     .expect("empty runtime is valid in serve mode");
    /// assert!(format!("{runtime:?}").contains("llm_backends"));
    /// ```
    pub fn build_allow_empty(self) -> Result<Runtime, crate::BuildError> {
        self.0.build_allow_empty().map(Runtime)
    }
}

// Keep a `RuntimeBuilder` type alias so existing `use tau_runtime::RuntimeBuilder` code compiles.
// The canonical builder type is TauRuntimeBuilder.
/// Type alias — use [`Runtime::builder()`] to construct.
pub type RuntimeBuilderAlias = TauRuntimeBuilder;

impl Runtime {
    // -----------------------------------------------------------------------
    // Public streaming entry points (Task 6)
    // -----------------------------------------------------------------------

    /// Stream an agent run from a single initial message.
    ///
    /// Convenience wrapper over [`Runtime::run_streaming_with_history`]
    /// that passes an empty history. Validates inputs (capability override,
    /// LLM backend resolution, tool capability filtering) and constructs
    /// owned snapshots of registry data for the `'static` stream.
    ///
    /// Returns a `Stream<Item = RunEvent>` that yields text deltas,
    /// tool-call events, turn-completion signals, and a terminal
    /// `RunCompleted` event.
    ///
    /// # Note: non-`Send` stream
    ///
    /// Note: the returned stream is **not** `Send`. The underlying
    /// `DynLlmBackend::stream` returns a non-`Send` boxed future per
    /// the design at `builder.rs:45-50`. Consumers must drive the
    /// stream from a single tokio task (or use `tokio::task::LocalSet`
    /// for cross-task usage).
    ///
    /// This will be revisited in a future ADR if/when `DynLlmBackend`
    /// gains a Send-bounded variant.
    ///
    /// # Example
    ///
    /// ```
    /// # tokio_test::block_on(async {
    /// # use tau_runtime::{Runtime, RunOptions};
    /// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
    /// # use tau_ports::StopReason;
    /// # use tau_domain::fixtures::{any_agent_definition, any_package_manifest, any_message};
    /// # use futures_util::{StreamExt, pin_mut};
    /// # let resp = make_completion_response(
    /// #     "hello".into(), Vec::new(), StopReason::EndTurn, Some(make_token_usage(1, 1))
    /// # );
    /// # // "test-pkg" matches the llm_backend field set by any_agent_definition().
    /// # let llm = MockLlmBackend::new("test-pkg").with_response(resp);
    /// # let runtime = Runtime::builder().with_llm_backend(llm).build().unwrap();
    /// # let agent_def = any_agent_definition();
    /// # let manifest = any_package_manifest();
    /// # let msg = any_message();
    /// # let opts: RunOptions = Default::default();
    /// let stream = runtime.run_streaming(agent_def, manifest, msg, opts).await.unwrap();
    /// pin_mut!(stream);
    /// while let Some(_event) = stream.next().await {
    ///     // handle event
    /// }
    /// # });
    /// ```
    pub async fn run_streaming(
        &self,
        agent_def: tau_domain::AgentDefinition,
        package_manifest: tau_domain::PackageManifest,
        initial_message: tau_domain::Message,
        options: crate::options::RunOptions,
    ) -> Result<
        impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static,
        crate::error::RuntimeError,
    > {
        self.run_streaming_with_history(
            agent_def,
            package_manifest,
            Vec::new(),
            initial_message,
            options,
        )
        .await
    }

    /// Stream an agent run, prepending conversation history before the
    /// initial message.
    ///
    /// Validates inputs — capability override compute, LLM backend
    /// resolution, and tool capability filtering — then builds owned
    /// snapshots of the registry data and constructs the stream by
    /// calling `crate::stream::run_streaming_inner`.
    ///
    /// The setup mirrors `run.rs` `run_with_history` exactly (§4.1):
    ///   - `compute_effective` for project capability override
    ///   - `granted_for_kernel` + `granted_for_session` views
    ///   - `deny_entries` collection
    ///   - LLM backend resolution
    ///   - capability-filtered `tool_specs` (filtered tools are dropped)
    ///   - `tools` + `tool_validators` HashMap snapshots (Arc-clones)
    ///
    /// # Note: non-`Send` stream
    ///
    /// Note: the returned stream is **not** `Send`. The underlying
    /// `DynLlmBackend::stream` returns a non-`Send` boxed future per
    /// the design at `builder.rs:45-50`. Consumers must drive the
    /// stream from a single tokio task (or use `tokio::task::LocalSet`
    /// for cross-task usage).
    ///
    /// This will be revisited in a future ADR if/when `DynLlmBackend`
    /// gains a Send-bounded variant.
    ///
    /// # Example
    ///
    /// ```
    /// # tokio_test::block_on(async {
    /// # use tau_runtime::{Runtime, RunOptions};
    /// # use tau_ports::fixtures::{MockLlmBackend, make_completion_response, make_token_usage};
    /// # use tau_ports::StopReason;
    /// # use tau_domain::{Message, fixtures::{any_agent_definition, any_package_manifest, any_message}};
    /// # use futures_util::{StreamExt, pin_mut};
    /// # let resp = make_completion_response(
    /// #     "hello".into(), Vec::new(), StopReason::EndTurn, Some(make_token_usage(1, 1))
    /// # );
    /// # // "test-pkg" matches the llm_backend field set by any_agent_definition().
    /// # let llm = MockLlmBackend::new("test-pkg").with_response(resp);
    /// # let runtime = Runtime::builder().with_llm_backend(llm).build().unwrap();
    /// # let agent_def = any_agent_definition();
    /// # let manifest = any_package_manifest();
    /// # let history: Vec<Message> = Vec::new();
    /// # let msg = any_message();
    /// # let opts: RunOptions = Default::default();
    /// let stream = runtime
    ///     .run_streaming_with_history(agent_def, manifest, history, msg, opts)
    ///     .await
    ///     .unwrap();
    /// pin_mut!(stream);
    /// while let Some(_event) = stream.next().await {
    ///     // handle event
    /// }
    /// # });
    /// ```
    pub async fn run_streaming_with_history(
        &self,
        agent_def: tau_domain::AgentDefinition,
        package_manifest: tau_domain::PackageManifest,
        history: Vec<tau_domain::Message>,
        initial_message: tau_domain::Message,
        options: crate::options::RunOptions,
    ) -> Result<
        impl futures_core::Stream<Item = crate::stream::RunEvent> + 'static,
        crate::error::RuntimeError,
    > {
        use tau_ports::DenyEntry;

        info!(name = "runtime.run_streaming_started");

        // Step 1: Determine the effective grant set.
        let (granted_for_kernel, granted_for_session, deny_entries) = if let Some(override_grant) =
            options.granted_capabilities_override.as_ref()
        {
            debug!(
                name = "runtime.streaming_capability_set_loaded_from_override",
                count = override_grant.len(),
                reason = "agent.<kind>.spawn child run",
            );
            let kernel: Vec<Capability> = override_grant.clone();
            let session: Vec<Capability> = override_grant.clone();
            (kernel, session, Vec::<DenyEntry>::new())
        } else {
            let effective = crate::capability_override::compute_effective(
                package_manifest.capabilities(),
                &options.project_override,
            )
            .map_err(|e| {
                warn!(
                    name = "runtime.streaming_capability_override_rejected",
                    agent_id = %agent_def.id,
                    package_id = %agent_def.package.name,
                    kind = %e.kind,
                    reason = %e.reason,
                );
                crate::error::RuntimeError::Core(
                    crate::error::CoreRuntimeError::CapabilityOverrideExpands {
                        kind: e.kind,
                        reason: e.reason,
                    },
                )
            })?;

            let granted_for_kernel: Vec<Capability> =
                effective.iter().map(|e| e.source.clone()).collect();
            debug!(
                name = "runtime.streaming_capability_set_loaded",
                count = granted_for_kernel.len(),
                overrides_applied = options.project_override.len(),
            );

            let granted_for_session: Vec<Capability> = effective
                .iter()
                .map(narrowed_capability_for_session)
                .collect();

            let deny_entries: Vec<DenyEntry> = effective
                .iter()
                .filter(|e| !e.deny.is_empty())
                .map(|e| {
                    let kind = match &e.source {
                        Capability::Filesystem(tau_domain::FsCapability::Read { .. }) => "fs.read",
                        Capability::Filesystem(tau_domain::FsCapability::Write { .. }) => {
                            "fs.write"
                        }
                        Capability::Filesystem(tau_domain::FsCapability::Exec { .. }) => "fs.exec",
                        Capability::Network(tau_domain::NetCapability::Http { .. }) => "net.http",
                        Capability::Process(tau_domain::ProcessCapability::Spawn { .. }) => {
                            "process.spawn"
                        }
                        _ => "unknown",
                    };
                    DenyEntry::new(kind.to_string(), e.deny.clone())
                })
                .collect();

            (granted_for_kernel, granted_for_session, deny_entries)
        };
        let granted: &[Capability] = &granted_for_kernel;

        // Step 5: Resolve LLM backend.
        let backend = self
            .resolve_llm_backend(agent_def.id.as_str(), agent_def.llm_backend.as_str())?
            .clone();

        // Step 6: Build capability-filtered tool_specs.
        let mut tool_specs: Vec<ToolSpec> = Vec::with_capacity(self.tools().len());
        for (name, tool) in self.tools().iter() {
            let required = tool.capabilities();
            if check_capabilities(granted, required).is_none() {
                tool_specs.push(tool.schema());
            } else {
                debug!(
                    name = "runtime.streaming_tool_filtered",
                    tool_name = name.as_str(),
                    "tool filtered out: missing capability",
                );
            }
        }

        // Step 7: Snapshot tools registry (Arc-clones).
        let tools: std::collections::HashMap<String, Arc<dyn DynTool>> = self
            .tools()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Step 8: Snapshot tool_validators registry.
        let tool_validators: std::collections::HashMap<String, crate::tool_args::ToolArgsValidator> =
            self.tool_validators().clone().into_iter().collect();

        // Step 9: Construct and return the stream.
        let stream = crate::stream::run_streaming_inner(
            backend,
            agent_def,
            package_manifest,
            history,
            initial_message,
            options,
            tools,
            tool_validators,
            granted_for_kernel,
            tool_specs,
            deny_entries,
            granted_for_session,
        );
        Ok(stream)
    }
}
