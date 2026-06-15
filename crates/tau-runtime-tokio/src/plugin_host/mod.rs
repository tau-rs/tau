//! Host-side plugin loader for `tau-runtime-tokio`.
//!
//! # ⚠ Deprecated since β.1
//!
//! This module is preserved as-is for the tokio shell only. Phase β.3
//! introduces the MCP facilitator which **replaces the bespoke
//! subprocess plugin protocol** with a standard MCP contract surface.
//! Embassy / wasm / MCU shells will never carry `plugin_host`.
//!
//! New plugin types should target the β.3 MCP facilitator rather than
//! adding to `plugin_host`. The legacy entries here remain functional
//! to keep existing fs-read / shell / Anthropic / OpenAI / Ollama
//! plugins running through γ.0; expect removal once β.3 lands and the
//! existing plugins are migrated.
//!
//! This module spawns plugin processes, drives the `meta.handshake`
//! exchange, and produces `Arc<dyn DynLlmBackend>` (and friends) that
//! the kernel consumes unchanged. Plugin authors sit on the SDK side
//! ([`tau_plugin_sdk`](../../tau_plugin_sdk/index.html)); this module
//! is the host counterpart, the *opposite* end of the same wire
//! protocol defined in `tau-plugin-protocol`.
//!
//! # Module status
//!
//! Task 15 wires spawn + dispatch end-to-end: the three [`load_*`]
//! entry points spawn the plugin binary, drive the handshake, and
//! return an `Arc<dyn Dyn*>` shim backed by an `Ipc*` adapter
//! (`IpcLlmBackend` / `IpcTool` / `IpcStorage`).
//! Streaming (`llm.stream`) is wired in Task 16; protocol recording
//! is wired in Task 17.
//!
//! Note: `IpcSandbox` and `load_sandbox` were removed in the v0.1 port
//! refinement. The new `Sandbox::wrap_spawn` takes `&mut Command` (an
//! in-process concept) which cannot be transmitted over IPC. In-tree
//! adapters (`tau-sandbox-native`, `tau-sandbox-container`) replace the
//! IPC route (Tasks 3, 6 of the sandboxing plan).
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md` §7
//! and (forthcoming) ADR-0008 for the design rationale.
//!
//! [`load_*`]: load_llm_backend

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tau_domain::PortKind;
use tau_pkg::LockedPlugin;
use tau_plugin_protocol::handshake::TraceContext;
use tau_plugin_protocol::FramerOptions;
use tau_ports::CapabilityPlan;

use crate::builder::{DynLlmBackend, DynStorage, DynTool};
use crate::error::{CoreRuntimeError, RuntimeError};

mod handshake;
mod ipc_llm;
mod ipc_storage;
mod ipc_tool;
mod process;
mod recording;
mod stream_router;

/// Internal-but-test-visible re-exports. Hidden from rustdoc and not
/// covered by stability guarantees: integration tests under
/// `tau-runtime/tests/plugin_host_*.rs` reach in here to drive the
/// handshake driver and the per-port IPC adapters over a
/// `tau_plugin_protocol::test_support::FakeStdioPeer` without
/// spawning real subprocesses.
///
/// The non-test items
/// ([`__internals::drive_handshake`]) are always available; the test
/// constructors (`__internals::PluginProcess`, `__internals::IpcLlmBackend`,
/// `__internals::IpcTool`, `__internals::IpcStorage`, etc.) require the
/// `test-support` cargo feature so the production build can drop them.
#[doc(hidden)]
pub mod __internals {
    pub use super::handshake::drive_handshake;

    #[cfg(any(test, feature = "test-support"))]
    pub use super::ipc_llm::IpcLlmBackend;
    #[cfg(any(test, feature = "test-support"))]
    pub use super::ipc_storage::IpcStorage;
    #[cfg(any(test, feature = "test-support"))]
    pub use super::ipc_tool::IpcTool;
    #[cfg(any(test, feature = "test-support"))]
    pub use super::process::{DynAsyncWriter, PluginProcess};
    #[cfg(any(test, feature = "test-support"))]
    pub use super::recording::{Recorder, RecorderHandle};
}

pub use recording::{Direction, Recorder, RecorderHandle};

/// Spawn a plugin, drive the handshake, and immediately shut it down,
/// returning the validated [`tau_plugin_protocol::HandshakeResponse`].
///
/// Used by `tau plugin describe` (spec §9 debug tier) so the CLI can
/// print a plugin's advertised metadata + per-method schemas without
/// keeping the subprocess alive. Equivalent to one full lifecycle:
///
/// 1. Spawn the plugin binary with the same env scrubbing as the
///    `load_*` paths.
/// 2. Drive `meta.handshake` against the requested
///    [`tau_domain::PortKind`].
/// 3. Send `meta.shutdown` and wait for the child to exit
///    (escalating to SIGTERM/SIGKILL per the standard
///    [`PluginHostOptions::shutdown_timeout`] budget).
///
/// `expected_port` is taken from the plugin's manifest by the caller;
/// `required_methods` is left empty so the host doesn't reject plugins
/// that omit the conventional `*.complete`/`*.call` methods (the
/// describe command's purpose is exactly to *report* what's there).
///
/// Errors mirror [`load_llm_backend`]: spawn / handshake failures
/// surface typed [`RuntimeError`] variants.
pub async fn describe_plugin(
    plugin: &LockedPlugin,
    trace_context: TraceContext,
    options: PluginHostOptions,
) -> Result<tau_plugin_protocol::HandshakeResponse, RuntimeError> {
    let plugin_name = plugin.manifest.bin.clone();
    let run_id = trace_context.run_id.clone();
    let agent_id = trace_context.agent_id.clone();
    let trace_for_handshake = trace_context.clone();
    let plugin_name_for_handshake = plugin_name.clone();
    let handshake_timeout = options.handshake_timeout;
    let mut framer_options = FramerOptions::default();
    framer_options.max_message_size = options.max_message_size;
    let recorder = build_recorder(&plugin_name, &options).await;
    let expected_port = plugin.manifest.provides;

    let (process, response) = process::PluginProcess::spawn_and_handshake(
        &plugin.binary_path,
        plugin_name.clone(),
        &run_id,
        &agent_id,
        framer_options,
        options.shutdown_timeout,
        recorder.clone(),
        // No sandbox for the describe path — this is a one-shot introspection
        // call that doesn't exercise plugin capabilities.
        None,
        // No credential injection for describe — introspection has no agent.
        None,
        &[],
        |reader, writer| {
            Box::pin(async move {
                handshake::drive_handshake(
                    reader,
                    writer,
                    &plugin_name_for_handshake,
                    expected_port,
                    &[],
                    serde_json::Value::Null,
                    trace_for_handshake,
                    handshake_timeout,
                )
                .await
            })
        },
    )
    .await?;

    // Cleanly shut the plugin down: send `meta.shutdown` and wait for
    // the child to exit. This deviates from the long-lived `load_*`
    // path in that we don't keep the `PluginProcess` around — describe
    // is one-shot.
    process.shutdown().await;

    // The recorder is now a stateless tracing emitter (Sub-project D
    // Task 5). The actual file is owned by `PluginRecordingLayer`,
    // installed by the CLI; the caller is responsible for flushing
    // the layer on process exit when `--record-protocol` is set.
    let _ = recorder;

    Ok(response)
}

/// Build a [`recording::RecorderHandle`] from
/// [`PluginHostOptions::recording`]. Post Sub-project D Task 5 this is
/// a pure constructor: the recorder no longer opens any file, it just
/// holds the plugin name so per-plugin attribution can be carried on
/// each `tracing::event!` it emits. The actual JSONL writer is
/// [`tau_observe::layers::plugin_recording::PluginRecordingLayer`],
/// which the CLI installs alongside the global subscriber.
async fn build_recorder(
    plugin_name: &str,
    options: &PluginHostOptions,
) -> Option<recording::RecorderHandle> {
    options
        .recording
        .as_ref()
        .map(|RecordingSink::JsonlFile { .. }| Arc::new(recording::Recorder::new(plugin_name)))
}

/// Optional protocol-recording sink. Currently only
/// [`RecordingSink::JsonlFile`] is defined; Task 17 wires the actual
/// frame-by-frame tap.
///
/// `#[non_exhaustive]`: future sinks (e.g. an in-memory ring buffer
/// for tests, a UDS-streamed sink for live inspection) are additive.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
/// use tau_runtime_tokio::plugin_host::RecordingSink;
///
/// let sink = RecordingSink::JsonlFile {
///     path: PathBuf::from("/tmp/protocol.jsonl"),
/// };
/// assert!(matches!(sink, RecordingSink::JsonlFile { .. }));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RecordingSink {
    /// Append-mode JSONL file. Each frame becomes one line carrying
    /// metadata (`ts`, `plugin`, `dir`, `msgid`, `method`) and the
    /// base64-encoded MessagePack frame, per spec §7.8.
    JsonlFile {
        /// Path to the recording file. The file is opened in append
        /// mode and created if it doesn't exist.
        path: PathBuf,
    },
}

/// Tunables controlling plugin-host behavior.
///
/// `#[non_exhaustive]`: additive fields are non-breaking. Use
/// [`PluginHostOptions::default`] and mutate the fields you care about
/// rather than struct-literal construction across crate boundaries.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use tau_runtime_tokio::plugin_host::PluginHostOptions;
///
/// let mut opts = PluginHostOptions::default();
/// opts.handshake_timeout = Duration::from_secs(10);
/// assert_eq!(opts.handshake_timeout, Duration::from_secs(10));
/// assert!(opts.recording.is_none());
/// assert!(!opts.force_passthrough);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PluginHostOptions {
    /// How long the host waits for the plugin's `meta.handshake`
    /// response before declaring the plugin unresponsive (surfaces as
    /// [`CoreRuntimeError::PluginHandshakeFailed`] with reason
    /// [`crate::error::HandshakeFailureReason::Timeout`]). Default
    /// `30s` — sized for the realistic worst-case process spawn +
    /// `dyld` resolution + tokio-runtime init under CI saturation
    /// (observed up to ~3s p99 on shared macOS runners). Genuinely
    /// broken plugins (crash on start, garbage on the wire) fail in
    /// <100ms regardless, so this only forgives slow-but-alive
    /// startups; it does not mask real failures.
    pub handshake_timeout: Duration,

    /// How long the host waits for the plugin to exit cleanly after a
    /// `meta.shutdown` notification before escalating to SIGTERM /
    /// SIGKILL. Default `2s`.
    pub shutdown_timeout: Duration,

    /// Maximum decoded frame body size (bytes), passed to the framer
    /// to bound memory use against malformed peers. Default 64 MiB.
    pub max_message_size: usize,

    /// If `Some`, every frame in either direction is mirrored to the
    /// supplied sink. Used by `tau --record-protocol <path>` (Task 20)
    /// and integration-test golden files. Defaults to `None`.
    ///
    /// Post Sub-project D Task 5, the actual file writer is the
    /// process-global
    /// [`tau_observe::layers::plugin_recording::PluginRecordingLayer`]
    /// installed by the CLI; this option only governs whether the
    /// plugin host wires up a per-plugin tracing emitter ([`Recorder`])
    /// so the layer can pick the frames up. Both pieces are toggled
    /// by `--record-protocol <path>`.
    pub recording: Option<RecordingSink>,

    /// Sandbox adapter to wrap each plugin spawn. If `None`, plugins
    /// run unsandboxed (the legacy default for non-CLI embedders).
    /// Populated by `tau-cli::cmd::plugin_loader::load_plugins` from
    /// the scope's `[sandbox]` config + `resolve_adapter`.
    ///
    /// At spawn time, this is zipped with the per-call `Option<&CapabilityPlan>`
    /// supplied to each `load_*` function. Both must be `Some` for sandbox
    /// enforcement to take effect — allowing callers to opt-out per plugin
    /// by passing `None` for the plan.
    pub sandbox_adapter: Option<Arc<crate::process_gate::SandboxAdapter>>,

    /// CLI override: force passthrough adapter (no isolation). Set by
    /// `--no-sandbox` or `--sandbox passthrough`. When `true`, the resolver
    /// receives `required_tier = None` and ignores plugin-tier floors.
    pub force_passthrough: bool,

    /// CLI override: force a specific adapter kind. Set by `--sandbox <kind>`
    /// (other than passthrough). When `Some`, the resolver instantiates and
    /// probes ONLY that kind. `None` = normal multi-adapter resolution.
    pub force_adapter_kind: Option<crate::process_gate::registry::RegistryKind>,

    /// Resolved credential chain (β.5). Behind `Arc` because
    /// [`CredentialChain`] is not `Clone` and `PluginHostOptions` is.
    /// `None` (default) = legacy embedders without a chain; declared
    /// credentials then fall back to today's env passthrough.
    /// Populated by `tau-cli::cmd::plugin_loader::load_plugins` from the
    /// scope's `[credentials]` config.
    ///
    /// [`CredentialChain`]: tau_ports::credential::CredentialChain
    pub credential_chain: Option<Arc<tau_ports::credential::CredentialChain>>,

    /// The active agent's `[[credentials]]` declarations (β.5). Each is
    /// resolved through [`Self::credential_chain`] and injected into the
    /// plugin child's env at spawn time. Default empty = no injection.
    /// Populated from `AgentEntry::credentials` by `load_plugins`.
    pub agent_credentials: Arc<[tau_pkg::project::project::AgentCredential]>,
}

impl Default for PluginHostOptions {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(2),
            // 64 MiB matches `tau-plugin-protocol::framer`'s default
            // `max_frame_size` so the same ceiling applies on both
            // sides of the wire.
            max_message_size: 64 * 1024 * 1024,
            recording: None,
            sandbox_adapter: None,
            force_passthrough: false,
            force_adapter_kind: None,
            credential_chain: None,
            agent_credentials: Arc::from(Vec::new()),
        }
    }
}

/// Load a plugin that provides the `LlmBackend` port and return a
/// kernel-ready `Arc<dyn DynLlmBackend>` shim.
///
/// # Lifecycle
///
/// 1. Spawn `plugin.binary_path` with stdio piped, env scrubbed
///    except for `TAU_PLUGIN_RUN_ID` / `TAU_PLUGIN_AGENT_ID` (plus
///    `PATH` for shared-library resolution).
/// 2. Send `meta.handshake` with `protocol_version`, the requested
///    [`tau_domain::PortKind::LlmBackend`], the supplied
///    `trace_context`, and the supplied `config`.
/// 3. Validate the handshake response: protocol version match,
///    `provides == LlmBackend`, required methods include
///    `llm.complete`.
/// 4. Spawn the per-process read loop and stderr re-emit task.
/// 5. Return an `IpcLlmBackend` wrapped in `Arc<dyn DynLlmBackend>`.
///
/// On any of those steps failing, [`crate::error::RuntimeError::PluginSpawnFailed`],
/// [`CoreRuntimeError::PluginHandshakeFailed`], or
/// [`CoreRuntimeError::PluginContractViolation`] is returned and the
/// child process is reaped.
pub async fn load_llm_backend(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    trace_context: TraceContext,
    options: PluginHostOptions,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynLlmBackend>, RuntimeError> {
    let plugin_name = plugin.manifest.bin.clone();
    let run_id = trace_context.run_id.clone();
    let agent_id = trace_context.agent_id.clone();
    let trace_for_handshake = trace_context.clone();
    let config_for_handshake = config;
    let plugin_name_for_handshake = plugin_name.clone();
    let handshake_timeout = options.handshake_timeout;
    // `FramerOptions` is `#[non_exhaustive]`; use `default()` and
    // mutate the field rather than struct-literal construction.
    let mut framer_options = FramerOptions::default();
    framer_options.max_message_size = options.max_message_size;
    let recorder = build_recorder(&plugin_name, &options).await;

    // Zip sandbox plan + adapter: both must be present for enforcement.
    // If either is None (legacy callers or no-sandbox config), spawn proceeds
    // without sandboxing, preserving backward compatibility.
    let sandbox = sandbox_plan.zip(options.sandbox_adapter.as_deref());

    let (process, _handshake_response) = process::PluginProcess::spawn_and_handshake(
        &plugin.binary_path,
        plugin_name.clone(),
        &run_id,
        &agent_id,
        framer_options,
        options.shutdown_timeout,
        recorder,
        sandbox,
        options.credential_chain.as_deref(),
        &options.agent_credentials,
        |reader, writer| {
            Box::pin(async move {
                handshake::drive_handshake(
                    reader,
                    writer,
                    &plugin_name_for_handshake,
                    PortKind::LlmBackend,
                    &["llm.complete"],
                    config_for_handshake,
                    trace_for_handshake,
                    handshake_timeout,
                )
                .await
            })
        },
    )
    .await?;

    Ok(Arc::new(ipc_llm::IpcLlmBackend::new(plugin_name, process)) as Arc<dyn DynLlmBackend>)
}

/// Load a plugin that provides the `Tool` port and return a
/// kernel-ready `Arc<dyn DynTool>` shim.
///
/// See [`load_llm_backend`] for the shared lifecycle / error
/// taxonomy. `Tool`-specific notes:
///
/// - The host drives the full `Tool` lifecycle (`init` / `invoke` /
///   `teardown`) per `tool.call`; the SDK runner on the plugin side
///   handles the corresponding session lifetime.
/// - The required wire methods are `tool.call` (and the meta methods
///   shared by every port).
///
/// # Implementation
///
/// 1. Spawn + handshake against [`tau_domain::PortKind::Tool`] +
///    `tool.call`.
/// 2. Issue a `tool.describe` RPC over the running process to capture
///    the [`tau_ports::ToolSpec`] eagerly (the kernel embeds it in
///    each `CompletionRequest.tools`).
/// 3. Wrap the process + cached schema in `ipc_tool::IpcTool` (private)
///    and erase to `Arc<dyn DynTool>`.
pub async fn load_tool(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    trace_context: TraceContext,
    options: PluginHostOptions,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynTool>, RuntimeError> {
    let plugin_name = plugin.manifest.bin.clone();
    let run_id = trace_context.run_id.clone();
    let agent_id = trace_context.agent_id.clone();
    let trace_for_handshake = trace_context.clone();
    let config_for_handshake = config;
    let plugin_name_for_handshake = plugin_name.clone();
    let handshake_timeout = options.handshake_timeout;
    let mut framer_options = FramerOptions::default();
    framer_options.max_message_size = options.max_message_size;
    let recorder = build_recorder(&plugin_name, &options).await;

    // Zip sandbox plan + adapter: both must be present for enforcement.
    let sandbox = sandbox_plan.zip(options.sandbox_adapter.as_deref());

    let (process, _handshake_response) = process::PluginProcess::spawn_and_handshake(
        &plugin.binary_path,
        plugin_name.clone(),
        &run_id,
        &agent_id,
        framer_options,
        options.shutdown_timeout,
        recorder,
        sandbox,
        options.credential_chain.as_deref(),
        &options.agent_credentials,
        |reader, writer| {
            Box::pin(async move {
                handshake::drive_handshake(
                    reader,
                    writer,
                    &plugin_name_for_handshake,
                    PortKind::Tool,
                    &["tool.call"],
                    config_for_handshake,
                    trace_for_handshake,
                    handshake_timeout,
                )
                .await
            })
        },
    )
    .await?;

    // Eagerly fetch the tool's schema so the kernel doesn't pay an
    // RPC round-trip per turn for `CompletionRequest.tools`.
    let schema = ipc_tool::IpcTool::fetch_schema(&process)
        .await
        .map_err(|e| {
            RuntimeError::Core(CoreRuntimeError::PluginContractViolation {
                plugin: plugin_name.clone(),
                detail: format!("tool.describe failed: {e}"),
            })
        })?;

    // Fetch declared capabilities. Tolerant — older plugins that
    // don't implement `tool.describe_capabilities` get an empty list
    // (which makes them unrestricted by the kernel's capability check
    // at `run.rs:272`, matching pre-Task-6 behavior).
    let capabilities = match ipc_tool::IpcTool::fetch_capabilities(&process).await {
        Ok(caps) => caps,
        Err(e) => {
            tracing::warn!(
                target: "tau_runtime_tokio::plugin_host",
                plugin = %plugin_name,
                error = %e,
                "tool.describe_capabilities failed; defaulting to empty list (plugin will be admitted unrestricted)",
            );
            Vec::new()
        }
    };

    Ok(Arc::new(ipc_tool::IpcTool::new(
        plugin_name,
        schema,
        capabilities,
        process,
    )) as Arc<dyn DynTool>)
}

/// Load a plugin that provides the `Storage` port and return a
/// kernel-ready `Arc<dyn DynStorage>` shim.
///
/// See [`load_llm_backend`] for the shared lifecycle / error taxonomy.
/// The required wire methods are `storage.get`, `storage.put`,
/// `storage.delete`, `storage.list` plus the meta methods.
///
/// # Implementation
///
/// Spawn + handshake against [`tau_domain::PortKind::Storage`]; the
/// host doesn't require any specific methods at handshake time
/// because the kernel doesn't route through `Storage` in v0.1 (per
/// spec §1.1). The returned `ipc_storage::IpcStorage` (private) dispatches
/// `storage.{get,put,list,delete}` per call.
pub async fn load_storage(
    plugin: &LockedPlugin,
    config: serde_json::Value,
    trace_context: TraceContext,
    options: PluginHostOptions,
    sandbox_plan: Option<&CapabilityPlan>,
) -> Result<Arc<dyn DynStorage>, RuntimeError> {
    let plugin_name = plugin.manifest.bin.clone();
    let run_id = trace_context.run_id.clone();
    let agent_id = trace_context.agent_id.clone();
    let trace_for_handshake = trace_context.clone();
    let config_for_handshake = config;
    let plugin_name_for_handshake = plugin_name.clone();
    let handshake_timeout = options.handshake_timeout;
    let mut framer_options = FramerOptions::default();
    framer_options.max_message_size = options.max_message_size;
    let recorder = build_recorder(&plugin_name, &options).await;

    // Zip sandbox plan + adapter: both must be present for enforcement.
    let sandbox = sandbox_plan.zip(options.sandbox_adapter.as_deref());

    let (process, _handshake_response) = process::PluginProcess::spawn_and_handshake(
        &plugin.binary_path,
        plugin_name.clone(),
        &run_id,
        &agent_id,
        framer_options,
        options.shutdown_timeout,
        recorder,
        sandbox,
        options.credential_chain.as_deref(),
        &options.agent_credentials,
        |reader, writer| {
            Box::pin(async move {
                handshake::drive_handshake(
                    reader,
                    writer,
                    &plugin_name_for_handshake,
                    PortKind::Storage,
                    &[],
                    config_for_handshake,
                    trace_for_handshake,
                    handshake_timeout,
                )
                .await
            })
        },
    )
    .await?;

    Ok(Arc::new(ipc_storage::IpcStorage::new(plugin_name, process)) as Arc<dyn DynStorage>)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::error::HandshakeFailureReason;

    #[test]
    fn plugin_host_options_default_has_30_second_handshake_timeout() {
        let opts = PluginHostOptions::default();
        assert_eq!(opts.handshake_timeout, Duration::from_secs(30));
    }

    #[test]
    fn plugin_host_options_default_has_2_second_shutdown_timeout() {
        let opts = PluginHostOptions::default();
        assert_eq!(opts.shutdown_timeout, Duration::from_secs(2));
    }

    #[test]
    fn plugin_host_options_default_has_64mib_max_message_size() {
        let opts = PluginHostOptions::default();
        assert_eq!(opts.max_message_size, 64 * 1024 * 1024);
    }

    #[test]
    fn plugin_host_options_default_has_no_recording() {
        let opts = PluginHostOptions::default();
        assert!(opts.recording.is_none());
    }

    #[test]
    fn recording_sink_jsonl_file_is_constructible() {
        // Smoke: the variant is constructible with a path. Actual sink
        // wiring lands in Task 17.
        let sink = RecordingSink::JsonlFile {
            path: PathBuf::from("/tmp/tau-protocol.jsonl"),
        };
        match sink {
            RecordingSink::JsonlFile { path } => {
                assert_eq!(path, PathBuf::from("/tmp/tau-protocol.jsonl"));
            }
        }
    }

    #[test]
    fn handshake_failure_reason_displays_as_expected() {
        let r = HandshakeFailureReason::Timeout;
        assert_eq!(format!("{r}"), "timeout");

        let r = HandshakeFailureReason::ProtocolVersionMismatch {
            host: "1".into(),
            plugin: "2".into(),
        };
        let s = format!("{r}");
        assert!(s.contains("1"), "got: {s}");
        assert!(s.contains("2"), "got: {s}");
    }
}
