//! Owns a running plugin subprocess: child handle, framer, dispatch
//! task, stderr re-emit task, and in-flight call tracking. Shared
//! across the per-port IPC adapters
//! (`IpcLlmBackend` / `IpcTool` / etc., landing in Task 15) via
//! [`std::sync::Arc`].
//!
//! The split between this module and
//! [`crate::plugin_host::handshake`] mirrors the SDK side: `process`
//! owns lifecycle (spawn / read-loop / stderr / shutdown);
//! `handshake` is the host-side handshake driver that runs once
//! before the read loop is allowed to consume inbound frames.
//!
//! See `docs/superpowers/specs/2026-04-28-plugin-loading-design.md`
//! §7.3 (PluginProcess + read loop + stderr task + shutdown sequence)
//! and §9.2 (wire-decode tracing).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;

use tau_plugin_protocol::{
    error::{RpcErrorEnvelope, INTERNAL_ERROR},
    Frame, FramedReader, FramedWriter, FramerOptions, ProtocolError,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio::task::JoinHandle;

use super::recording::{Direction, RecorderHandle};
use crate::process_gate::{validate_plan_against_adapter, SandboxAdapter, SandboxValidationError};

/// Type-erased async writer used by [`PluginProcess`] for outbound
/// frames. Boxing here lets the same struct wrap a real
/// [`tokio::process::ChildStdin`] (production spawn path) or an
/// in-memory [`tokio::io::DuplexStream`] (`#[cfg(test)]` /
/// `feature = "test-support"` path used by the per-port IPC adapter
/// integration tests). The runtime cost is one virtual call per
/// `write_all` — negligible against the rmp-serde encode and the
/// kernel's tokio scheduler hop.
pub type DynAsyncWriter = Box<dyn AsyncWrite + Send + Unpin>;

use tau_ports::{CapabilityHandle, CapabilityPlan};

use crate::error::RuntimeError;

/// Result delivered to a waiting `oneshot::Sender` for a host-issued
/// request. `Ok(bytes)` carries the rmp-serde-encoded result payload;
/// `Err(envelope)` carries the protocol-level error envelope from a
/// `Frame::Response`. Cancellation (sender dropped because the read
/// loop terminated) maps to a synthetic `Internal` envelope at the
/// caller boundary in Task 15.
pub(crate) type RpcResult = Result<Vec<u8>, RpcErrorEnvelope>;

/// Shared map of in-flight request msgids → response delivery channel.
pub(crate) type InFlightResponses = Arc<Mutex<HashMap<u32, oneshot::Sender<RpcResult>>>>;

/// Shared map of in-flight stream msgids → chunk delivery channel.
pub(crate) type InFlightStreams =
    Arc<Mutex<HashMap<u32, mpsc::Sender<tau_ports::CompletionChunk>>>>;

/// Owns a spawned plugin subprocess and the per-process dispatch
/// machinery. Not constructed directly outside this module in the
/// production path: the per-port `load_*` entry points wrap this in
/// `Arc<dyn Dyn*>` shims after [`PluginProcess::spawn_and_handshake`]
/// returns.
///
/// Public for the same `__internals` test-export reasons as
/// [`super::ipc_llm::IpcLlmBackend`]; the `new_for_test` constructor
/// gated by `feature = "test-support"` lets integration tests build
/// instances without spawning a real subprocess.
pub struct PluginProcess {
    /// Plugin name (typically `LockedPlugin::manifest.name`). Used
    /// for tracing fields and error messages.
    pub(crate) name: String,
    /// Monotonic msgid generator. Starts at 2; msgid `1` is reserved
    /// for the handshake exchange driven before this struct is built.
    pub(crate) next_msgid: AtomicU32,
    /// Writer for outbound frames. `Mutex`-wrapped so multiple
    /// concurrent IPC method calls can serialize their writes. The
    /// concrete writer is type-erased ([`DynAsyncWriter`]) so the same
    /// struct is shared between the production spawn path
    /// (wrapping a [`tokio::process::ChildStdin`]) and the test path
    /// (wrapping a [`tokio::io::DuplexStream`]).
    pub(crate) writer: Mutex<FramedWriter<DynAsyncWriter>>,
    /// Map of msgid → response delivery channel for outstanding
    /// requests. The read loop completes entries here; the per-port
    /// adapter inserts them before sending the request.
    pub(crate) in_flight_responses: InFlightResponses,
    /// Map of msgid → stream chunk delivery channel for outstanding
    /// streaming requests. The read loop forwards `stream.chunk`
    /// notifications here; [`super::ipc_llm::IpcLlmBackend::stream`]
    /// inserts an entry per `llm.stream` call and pairs it with an
    /// `in_flight_responses` entry under the same msgid. The
    /// [`super::stream_router`] consumes both halves to assemble a
    /// [`tau_ports::CompletionStream`].
    pub(crate) in_flight_streams: InFlightStreams,
    /// The spawned child process. Wrapped in `Mutex<Option<_>>` so
    /// shutdown can `take()` it before awaiting `Child::wait`.
    child: Mutex<Option<Child>>,
    /// Notified when shutdown starts; the read/stderr tasks observe
    /// this only via the underlying transport closing, not directly.
    /// Reserved for Task 17's recording-flush coordination.
    #[allow(dead_code)]
    shutdown_signal: Notify,
    /// Optional protocol recorder. When set, every outbound frame
    /// (via [`PluginProcess::send_frame`]) and every inbound frame
    /// (in the read loop) is mirrored to the recorder before
    /// dispatch. See `crate::plugin_host::recording`.
    pub(crate) recorder: Option<RecorderHandle>,
    /// How long [`PluginProcess::shutdown`] waits between escalation
    /// steps (post-`meta.shutdown` graceful → SIGTERM, and after
    /// SIGTERM → SIGKILL). Default 2s from `PluginHostOptions`.
    shutdown_timeout: Duration,
    /// Read-loop join handle; kept so the runtime can observe it for
    /// diagnostics. Dropped on `PluginProcess` drop, which aborts the
    /// task if it is still running.
    _read_task: JoinHandle<()>,
    /// Stderr re-emit task join handle. Same drop semantics as
    /// `_read_task`.
    _stderr_task: JoinHandle<()>,
    /// Holds adapter-side sandbox resources (e.g., container ID).
    /// Dropped after the child exits, running adapter cleanup via
    /// `CapabilityHandle::drop`. `Option` because test/no-sandbox paths
    /// construct without one (via `new_for_test`).
    ///
    /// Wrapped in `Mutex` because `CapabilityHandle` contains a
    /// `Box<dyn FnOnce()>` which is `!Sync`; the `Mutex` adds the
    /// `Sync` bound required by `Arc<PluginProcess>` (the `Dyn*`
    /// port traits are `Send + Sync`).
    #[allow(dead_code)] // keeps the handle alive; cleanup runs on Mutex drop
    _sandbox_handle: std::sync::Mutex<Option<CapabilityHandle>>,
}

/// Secret env-var names re-injected into every plugin child after
/// [`tokio::process::Command::env_clear`]. This is an **explicit
/// allowlist**, not the host's full environment: `env_clear()` exists
/// precisely so plugins run in a minimal, reproducible env, and only
/// these well-known secret names are passed back through.
///
/// One entry per shipped LLM plugin's default env-var name
/// (`default_api_key_env` / `default_bearer_token_env`):
/// - `ANTHROPIC_API_KEY` — `tau-plugins/anthropic`
/// - `OPENAI_API_KEY` — `tau-plugins/openai`
/// - `OLLAMA_BEARER_TOKEN` — `tau-plugins/ollama`
///
/// A plugin configured with a *custom* `api_key_env` name still needs
/// the cleartext-config path or a future per-plugin env declaration; the
/// host cannot enumerate arbitrary names without re-introducing the
/// pass-everything behavior `env_clear()` removed.
const SECRET_ENV_ALLOWLIST: &[&str] =
    &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OLLAMA_BEARER_TOKEN"];

/// Apply the plugin child's minimal, reproducible environment to
/// `command`: clear the inherited env, then re-add only the two
/// `TAU_PLUGIN_*` identity vars, `PATH` (for shared-library
/// resolution), and any [`SECRET_ENV_ALLOWLIST`] names present in the
/// parent env (resolved via `env_lookup`).
///
/// Without the allowlist re-injection, `env_clear()` wiped env-provided
/// secrets like `ANTHROPIC_API_KEY` before the plugin's `from_config`
/// `std::env::var` lookup could read them — forcing real keys into
/// plaintext `tau.toml`. Only allowlisted names cross over, so the
/// security intent of `env_clear()` (no ambient host env) is preserved.
///
/// `env_lookup` is injected (rather than calling `std::env::var`
/// directly) so the policy is unit-testable without spawning a
/// subprocess or mutating the test process's real environment. The
/// production caller passes `|n| std::env::var(n).ok()`.
fn configure_plugin_command_env(
    command: &mut Command,
    run_id: &str,
    agent_id: &str,
    env_lookup: impl Fn(&str) -> Option<String>,
) {
    command
        .env_clear()
        .env("TAU_PLUGIN_RUN_ID", run_id)
        .env("TAU_PLUGIN_AGENT_ID", agent_id)
        // Inherit PATH so shared-library lookups (libc, libssl, …) work
        // the same as for the host. Anything more ambient than PATH and
        // the allowlisted secrets below should be added via the
        // per-plugin config payload, not the env, so plugin behavior
        // stays reproducible across hosts.
        .env("PATH", env_lookup("PATH").unwrap_or_default());

    // Re-inject env-provided secrets so plaintext `tau.toml` is no
    // longer the only working path. Only allowlisted names cross over,
    // and only when actually set in the parent env.
    for name in SECRET_ENV_ALLOWLIST {
        if let Some(value) = env_lookup(name) {
            command.env(name, value);
        }
    }
}

impl PluginProcess {
    /// Spawn a plugin subprocess and install the framer, dispatch
    /// read loop, and stderr re-emit task. The handshake is **not**
    /// driven here — callers run [`crate::plugin_host::handshake`]
    /// over a borrowed reader+writer first, then feed the post-
    /// handshake reader into [`PluginProcess::install_read_loop`] via
    /// this constructor.
    ///
    /// Steps:
    ///
    /// 1. `tokio::process::Command::spawn` with `stdin`/`stdout`/`stderr`
    ///    piped, `env_clear()`, and the two `TAU_PLUGIN_*` env vars
    ///    plus `PATH` for shared-library resolution.
    /// 2. `kill_on_drop(true)` so a panicking host doesn't leak the
    ///    child.
    /// 3. The handshake driver runs against the borrowed
    ///    `FramedReader`/`FramedWriter` returned through the
    ///    `pre_handshake` closure.
    /// 4. After the handshake completes, the read loop and stderr
    ///    task spawn and the `PluginProcess` is returned.
    ///
    /// Errors:
    ///
    /// * [`RuntimeError::PluginSpawnFailed`] if `Command::spawn`
    ///   fails (binary missing, not executable, sandbox denied, …).
    /// * Whatever the `pre_handshake` closure returns.
    // Nine params (two beyond clippy's default ceiling) is the natural
    // shape of "spawn + sandbox + drive handshake" — splitting into a
    // builder wouldn't simplify the call sites in
    // `crate::plugin_host::load_*`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn spawn_and_handshake<F, T>(
        binary_path: &Path,
        plugin_name: String,
        run_id: &str,
        agent_id: &str,
        framer_options: FramerOptions,
        shutdown_timeout: Duration,
        recorder: Option<RecorderHandle>,
        // Optional sandbox plan + adapter pair. When `Some((plan, adapter))`:
        // 1. `validate_plan_against_adapter` is called (Layer 3 cross-check).
        // 2. `adapter.wrap_spawn(plan, &mut command)` is called to apply
        //    enforcement. The resulting `CapabilityHandle` is stored on the
        //    `PluginProcess` so its `Drop` runs adapter cleanup on plugin
        //    exit. When `None`, the spawn proceeds without sandboxing (test
        //    paths; use `MockSandbox` for behavioral tests instead).
        sandbox: Option<(&CapabilityPlan, &SandboxAdapter)>,
        pre_handshake: F,
    ) -> Result<(Arc<PluginProcess>, T), RuntimeError>
    where
        // The HRTB on the future return type lets the closure body
        // capture the `&mut` borrows without us having to spell out a
        // separate `for<'a>` lifetime in the call site. The returned
        // future is type-erased to `Pin<Box<dyn Future + 'a>>` so
        // calling code can use `async move {...}` without tripping
        // the higher-ranked-closure lifetime inference bug
        // (rust-lang/rust#90696).
        F: for<'a> FnOnce(
            &'a mut FramedReader<ChildStdout>,
            &'a mut FramedWriter<DynAsyncWriter>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, RuntimeError>> + Send + 'a>,
        >,
    {
        tracing::debug!(
            target: "tau_runtime_tokio::plugin_host",
            plugin = plugin_name.as_str(),
            binary_path = ?binary_path,
            "plugin.spawning"
        );

        let mut command = Command::new(binary_path);
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Clear the inherited env and re-add only the identity vars,
        // PATH, and the secret-env allowlist (so env-provided keys like
        // ANTHROPIC_API_KEY reach the plugin without plaintext config).
        configure_plugin_command_env(&mut command, run_id, agent_id, |n| std::env::var(n).ok());

        // Layer 3 + 4: sandbox validation and enforcement.
        //
        // Order is deliberate: validate_plan_against_adapter (Layer 3)
        // runs BEFORE wrap_spawn (Layer 4) so a bad plan never reaches
        // the adapter.
        let sandbox_handle: Option<CapabilityHandle> = if let Some((plan, adapter)) = sandbox {
            // Layer 3: cross-check plan capabilities against adapter shapes.
            validate_plan_against_adapter(&plugin_name, plan, adapter).map_err(
                |errors: Vec<SandboxValidationError>| RuntimeError::SandboxValidationFailed {
                    plugin: plugin_name.clone(),
                    errors,
                },
            )?;

            // Layer 4: apply sandbox enforcement to the Command.
            // `as_std_mut()` is the bridge between `tokio::process::Command`
            // and `std::process::Command` required by `Sandbox::wrap_spawn`.
            let handle = adapter
                .wrap_spawn(plan, command.as_std_mut())
                .await
                .map_err(|source| RuntimeError::SandboxWrapFailed {
                    plugin: plugin_name.clone(),
                    source,
                })?;
            Some(handle)
        } else {
            None
        };

        let mut child = command
            .spawn()
            .map_err(|source| RuntimeError::PluginSpawnFailed {
                plugin: plugin_name.clone(),
                source,
            })?;

        let stdin = child
            .stdin
            .take()
            .expect("stdin piped via stdin(Stdio::piped())");
        let stdout = child
            .stdout
            .take()
            .expect("stdout piped via stdout(Stdio::piped())");
        let stderr = child
            .stderr
            .take()
            .expect("stderr piped via stderr(Stdio::piped())");

        let pid = child.id();
        tracing::info!(
            target: "tau_runtime_tokio::plugin_host",
            plugin = plugin_name.as_str(),
            pid,
            "plugin.spawned"
        );

        // Box the writer up-front so `PluginProcess::writer` can hold a
        // type-erased writer (this lets the test-only constructor share
        // the same field type with a `DuplexStream`-backed writer).
        // The handshake driver is generic over `W: AsyncWrite + Unpin`,
        // so passing a `FramedWriter<DynAsyncWriter>` is sound.
        let mut writer: FramedWriter<DynAsyncWriter> =
            FramedWriter::new(Box::new(stdin) as DynAsyncWriter);
        let mut reader = FramedReader::new(stdout, framer_options);

        // Drive the handshake (or whatever the caller needs to run
        // *before* the dispatch read loop starts consuming frames).
        let handshake_outcome = match pre_handshake(&mut reader, &mut writer).await {
            Ok(value) => value,
            Err(err) => {
                // Best-effort cleanup: kill the child so we don't leak
                // it. `kill_on_drop` handles drop-time cleanup, but
                // explicit kill yields more deterministic teardown
                // for the failure path.
                let _ = child.kill().await;
                return Err(err);
            }
        };

        let in_flight_responses: InFlightResponses = Arc::new(Mutex::new(HashMap::new()));
        let in_flight_streams: InFlightStreams = Arc::new(Mutex::new(HashMap::new()));

        let read_task = tokio::spawn(read_loop(
            reader,
            plugin_name.clone(),
            in_flight_responses.clone(),
            in_flight_streams.clone(),
            recorder.clone(),
        ));

        let stderr_task = tokio::spawn(stderr_loop(stderr, plugin_name.clone()));

        let process = Arc::new(PluginProcess {
            name: plugin_name,
            // msgid 1 was used for the handshake; subsequent calls
            // start at 2.
            next_msgid: AtomicU32::new(2),
            writer: Mutex::new(writer),
            in_flight_responses,
            in_flight_streams,
            child: Mutex::new(Some(child)),
            shutdown_signal: Notify::new(),
            recorder,
            shutdown_timeout,
            _read_task: read_task,
            _stderr_task: stderr_task,
            _sandbox_handle: std::sync::Mutex::new(sandbox_handle),
        });

        Ok((process, handshake_outcome))
    }

    /// Send a single outbound frame, tapping the protocol recorder
    /// (if any) before acquiring the writer mutex. Centralizes the
    /// host-side write path so per-port adapters
    /// (`IpcLlmBackend` / `IpcTool` / `IpcStorage`) get
    /// recording for free without each having to reach into the
    /// recorder themselves.
    ///
    /// Returns the framer's [`ProtocolError`] verbatim on write failure
    /// so call sites can wrap it in their port-specific error variants.
    pub(crate) async fn send_frame(&self, frame_bytes: &[u8]) -> Result<(), ProtocolError> {
        if let Some(recorder) = &self.recorder {
            recorder.record(Direction::HostToPlugin, frame_bytes).await;
        }
        let mut writer = self.writer.lock().await;
        writer.write_frame(frame_bytes).await
    }

    /// Drive the spec §7.3 shutdown sequence:
    ///
    /// 1. Send `meta.shutdown` notification (`[2, "meta.shutdown",
    ///    []]`).
    /// 2. Wait up to `shutdown_timeout` for the child to exit.
    /// 3. If still alive, send SIGTERM (`Child::start_kill` =
    ///    `SIGKILL` on Unix per `tokio` — see note below) and wait
    ///    500 ms.
    /// 4. If still alive, force-kill via `Child::kill`.
    /// 5. Emit `plugin.exited` with `clean: bool`.
    ///
    /// **Tokio detail**: `tokio::process::Child` does not expose a
    /// portable SIGTERM API. We approximate the spec sequence with
    /// the available primitives: graceful → `start_kill` (SIGKILL on
    /// Unix) after `shutdown_timeout` → `kill().await` (idempotent)
    /// after a 500 ms grace. Replacing `start_kill` with explicit
    /// SIGTERM via `nix` is a Phase-1 follow-up and additive to this
    /// API.
    #[allow(dead_code)] // wired by Task 24 via explicit close on Runtime drop.
    pub(crate) async fn shutdown(self: Arc<Self>) {
        // Step 1: send meta.shutdown notification (best-effort).
        let shutdown_frame = Frame::Notification {
            method: tau_plugin_protocol::handshake::meta::SHUTDOWN_METHOD.to_string(),
            // params is an empty rmp-serde-encoded array. The smallest
            // legitimate payload (per `Frame::encode`'s
            // `EmptyFrameSlot` rejection) is the one-byte `[0x90]`
            // (empty MessagePack array). Build it via rmp-serde so
            // the wire encoding is canonical.
            params: rmp_serde::to_vec::<Vec<()>>(&Vec::new())
                .expect("encoding empty Vec<()> never fails"),
        };
        if let Ok(body) = shutdown_frame.encode() {
            let mut writer = self.writer.lock().await;
            let _ = writer.write_frame(&body).await;
            tracing::debug!(
                target: "tau_runtime_tokio::plugin_host",
                plugin = self.name.as_str(),
                "plugin.shutdown_sent"
            );
        }

        // Step 2-5: take the child out of its mutex so we can wait/kill.
        let mut guard = self.child.lock().await;
        let Some(mut child) = guard.take() else {
            return;
        };
        drop(guard);

        // Wait up to `shutdown_timeout` for clean exit.
        let exit_status = match tokio::time::timeout(self.shutdown_timeout, child.wait()).await {
            Ok(Ok(status)) => Some((status, true)),
            Ok(Err(_)) | Err(_) => None,
        };

        let (exit_status, clean) = if let Some(es) = exit_status {
            es
        } else {
            // Step 3: SIGKILL fallback (tokio's only portable knob).
            // Spec calls for SIGTERM first; see method docs.
            let _ = child.start_kill();
            match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
                Ok(Ok(status)) => (status, false),
                Ok(Err(_)) | Err(_) => {
                    // Step 4: hard kill.
                    let _ = child.kill().await;
                    match child.wait().await {
                        Ok(status) => (status, false),
                        Err(_) => return, // process gone, give up
                    }
                }
            }
        };

        let exit_code = exit_status.code();
        tracing::info!(
            target: "tau_runtime_tokio::plugin_host",
            plugin = self.name.as_str(),
            exit_code = ?exit_code,
            clean,
            "plugin.exited"
        );
    }

    /// Construct a [`PluginProcess`] from pre-built reader+writer halves
    /// without spawning a subprocess.
    ///
    /// Used by the per-port IPC adapter integration tests in
    /// `tau-runtime/tests/plugin_host_ipc_*.rs`: the test harness pairs
    /// a [`tau_plugin_protocol::test_support::FakeStdioPeer`] with the
    /// host-side adapter via two duplex streams, drives the handshake
    /// out of band (or skips it for unit-style tests), and constructs
    /// the [`PluginProcess`] via this constructor so the adapter has a
    /// working `Arc<PluginProcess>` to dispatch through.
    ///
    /// # Caller responsibilities
    ///
    /// - The handshake must already have run on `reader`+`writer`
    ///   (msgid 1 consumed).
    /// - `next_msgid` is initialized to 2 to match
    ///   [`PluginProcess::spawn_and_handshake`]'s post-handshake state.
    /// - There is no underlying child process: the [`PluginProcess::shutdown`]
    ///   path is a no-op for test instances.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_for_test<R>(
        plugin_name: String,
        reader: FramedReader<R>,
        writer: FramedWriter<DynAsyncWriter>,
        shutdown_timeout: Duration,
    ) -> Arc<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        Self::new_for_test_with_recorder(plugin_name, reader, writer, shutdown_timeout, None)
    }

    /// Variant of [`PluginProcess::new_for_test`] that accepts an
    /// optional recorder. Used by `tau-runtime`'s recording integration
    /// tests to verify that the read- and write-side tap points fire
    /// without spawning a real subprocess. Production code must not
    /// reach for this constructor; the public `load_*` entry points
    /// build the recorder from [`crate::plugin_host::PluginHostOptions`]
    /// and feed it into [`PluginProcess::spawn_and_handshake`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_for_test_with_recorder<R>(
        plugin_name: String,
        reader: FramedReader<R>,
        writer: FramedWriter<DynAsyncWriter>,
        shutdown_timeout: Duration,
        recorder: Option<RecorderHandle>,
    ) -> Arc<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        let in_flight_responses: InFlightResponses = Arc::new(Mutex::new(HashMap::new()));
        let in_flight_streams: InFlightStreams = Arc::new(Mutex::new(HashMap::new()));

        let read_task = tokio::spawn(read_loop(
            reader,
            plugin_name.clone(),
            in_flight_responses.clone(),
            in_flight_streams.clone(),
            recorder.clone(),
        ));

        // Test instances have no real stderr to re-emit; spawn a stub
        // task that immediately returns so the field has a valid
        // `JoinHandle<()>` to drop with.
        let stderr_task = tokio::spawn(async {});

        Arc::new(PluginProcess {
            name: plugin_name,
            next_msgid: AtomicU32::new(2),
            writer: Mutex::new(writer),
            in_flight_responses,
            in_flight_streams,
            child: Mutex::new(None),
            shutdown_signal: Notify::new(),
            recorder,
            shutdown_timeout,
            _read_task: read_task,
            _stderr_task: stderr_task,
            // Test constructors have no sandbox; cleanup is a no-op.
            _sandbox_handle: std::sync::Mutex::new(None),
        })
    }
}

/// Per-process dispatch read loop. Runs on a dedicated tokio task
/// spawned by [`PluginProcess::spawn_and_handshake`]. Terminates on
/// EOF or framer error and drains both in-flight maps so callers
/// don't await forever.
///
/// Generic over the reader IO type so the production spawn path can
/// pass a [`tokio::process::ChildStdout`] and the test path can pass
/// a [`tokio::io::DuplexStream`].
async fn read_loop<R>(
    mut reader: FramedReader<R>,
    plugin_name: String,
    in_flight_responses: InFlightResponses,
    in_flight_streams: InFlightStreams,
    recorder: Option<RecorderHandle>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    loop {
        let body = match reader.next_frame().await {
            Ok(Some(b)) => b,
            Ok(None) => {
                tracing::info!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    "plugin stdout EOF"
                );
                break;
            }
            Err(err) => {
                tracing::error!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    err = %err,
                    "plugin stdout frame read error"
                );
                break;
            }
        };
        // Tap the inbound frame *before* decode-and-dispatch so the
        // recorded byte sequence matches exactly what came off the wire
        // — including frames whose payload `Frame::decode` later
        // rejects (those still get logged with `null` msgid/method via
        // `decode_frame_metadata` and the raw base64 frame).
        if let Some(recorder) = &recorder {
            recorder.record(Direction::PluginToHost, &body).await;
        }
        let frame = match Frame::decode(&body) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    err = %err,
                    "plugin frame decode failed"
                );
                continue;
            }
        };
        match frame {
            Frame::Response { id, error, result } => {
                tracing::trace!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    msgid = id,
                    error_code = error.as_ref().map(|e| e.code),
                    "plugin.response_received"
                );
                let mut map = in_flight_responses.lock().await;
                if let Some(sender) = map.remove(&id) {
                    let value = match (error, result) {
                        (Some(env), _) => Err(env),
                        // A `null` result is legitimate (per
                        // `Frame::decode`'s `Value::Nil → None`
                        // mapping); represent it as a zero-byte
                        // payload so downstream decoders can detect
                        // it. Per-port adapters in Task 15 are
                        // responsible for treating empty-bytes as
                        // "null" rather than as
                        // rmp-serde-deserialization fodder.
                        (None, Some(bytes)) => Ok(bytes),
                        (None, None) => Ok(Vec::new()),
                    };
                    let _ = sender.send(value);
                } else {
                    tracing::warn!(
                        target: "tau_runtime_tokio::plugin_host",
                        plugin = plugin_name.as_str(),
                        msgid = id,
                        "response with no matching in-flight request"
                    );
                }
            }
            Frame::Notification { method, params } if method == STREAM_CHUNK_METHOD => {
                // params shape: [originating_msgid, CompletionChunk]
                // (see test_support::send_stream_chunk + spec §7.3).
                let parsed: (u32, tau_ports::CompletionChunk) = match rmp_serde::from_slice(&params)
                {
                    Ok(p) => p,
                    Err(err) => {
                        tracing::warn!(
                            target: "tau_runtime_tokio::plugin_host",
                            plugin = plugin_name.as_str(),
                            err = %err,
                            "stream.chunk params decode failed"
                        );
                        continue;
                    }
                };
                let (originating_id, chunk) = parsed;
                tracing::trace!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    msgid = originating_id,
                    "plugin.stream_chunk"
                );
                let map = in_flight_streams.lock().await;
                if let Some(tx) = map.get(&originating_id) {
                    // Best-effort send; if the receiver dropped, the
                    // chunk is silently discarded (the streaming call
                    // was cancelled or the host stopped reading).
                    let _ = tx.send(chunk).await;
                }
            }
            // Other notifications and any plugin-initiated requests
            // are not part of the v0.1 host contract; ignore them.
            _ => {}
        }
    }

    // EOF/error path: notify all waiting callers that the plugin
    // exited mid-call so they don't await forever. Streams have their
    // senders dropped which terminates the receiver naturally.
    let mut responses = in_flight_responses.lock().await;
    for (_, sender) in responses.drain() {
        let _ = sender.send(Err(RpcErrorEnvelope::new(
            INTERNAL_ERROR,
            "plugin process exited mid-call".to_string(),
            None,
        )));
    }
    let mut streams = in_flight_streams.lock().await;
    streams.clear();
}

/// Re-emit plugin stderr lines as host-side tracing events. The SDK
/// formats events as JSON via [`tau-plugin-sdk`]'s tracing layer; we
/// best-effort parse each line and pull `level`/`message` to fan them
/// out into the right `tracing::*!` macro under the
/// `target = "plugin"` namespace, with the `plugin = <name>` field
/// preserved so subscribers can filter.
async fn stderr_loop(stderr: ChildStderr, plugin_name: String) {
    let reader = tokio::io::BufReader::new(stderr);
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => emit_plugin_line(&plugin_name, &line),
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(
                    target: "tau_runtime_tokio::plugin_host",
                    plugin = plugin_name.as_str(),
                    err = %err,
                    "plugin stderr read error"
                );
                break;
            }
        }
    }
}

/// Notification method for streaming chunks (see spec §4.6 and §7.3).
const STREAM_CHUNK_METHOD: &str = "stream.chunk";

fn emit_plugin_line(plugin_name: &str, line: &str) {
    // Try parsing as a JSON object first (the SDK's tracing layer
    // shape). If that fails, emit the raw line under
    // `target = "plugin"` at WARN.
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(json) => {
            let level = json
                .get("level")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("INFO");
            let message = json
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    json.get("fields")
                        .and_then(|f| f.get("message"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("");
            // `tracing` macros require static `target` strings, so
            // we use the constant `"plugin"` target and carry the
            // plugin name as a structured field.
            match level {
                "ERROR" => tracing::error!(target: "plugin", plugin = plugin_name, "{message}"),
                "WARN" => tracing::warn!(target: "plugin", plugin = plugin_name, "{message}"),
                "DEBUG" => tracing::debug!(target: "plugin", plugin = plugin_name, "{message}"),
                "TRACE" => tracing::trace!(target: "plugin", plugin = plugin_name, "{message}"),
                _ => tracing::info!(target: "plugin", plugin = plugin_name, "{message}"),
            }
        }
        Err(_) => {
            tracing::warn!(
                target: "plugin",
                plugin = plugin_name,
                raw = line,
                "non-json plugin stderr"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_plugin_command_env_injects_allowlisted_secrets_only() {
        use std::collections::HashMap;

        // A fake parent environment: allowlisted secrets + PATH, plus
        // non-allowlisted entries (including a non-allowlisted secret) that
        // must NOT cross into the child.
        let parent: HashMap<&str, &str> = [
            ("ANTHROPIC_API_KEY", "sk-ant-parent"),
            ("OPENAI_API_KEY", "sk-openai-parent"),
            ("OLLAMA_BEARER_TOKEN", "ollama-parent"),
            ("PATH", "/usr/bin:/bin"),
            ("AWS_SECRET_ACCESS_KEY", "leak-me-not"),
            ("HOME", "/home/host-user"),
        ]
        .into_iter()
        .collect();

        let mut command = Command::new("/bin/true");
        configure_plugin_command_env(&mut command, "run-7", "agent-9", |name| {
            parent.get(name).map(|v| (*v).to_string())
        });

        let envs: HashMap<String, Option<String>> = command
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        // Allowlisted secrets are re-injected from the parent env.
        assert_eq!(
            envs.get("ANTHROPIC_API_KEY"),
            Some(&Some("sk-ant-parent".to_string())),
            "ANTHROPIC_API_KEY must reach the plugin from the parent env"
        );
        assert_eq!(
            envs.get("OPENAI_API_KEY"),
            Some(&Some("sk-openai-parent".to_string()))
        );
        assert_eq!(
            envs.get("OLLAMA_BEARER_TOKEN"),
            Some(&Some("ollama-parent".to_string()))
        );

        // Required plumbing is still set.
        assert_eq!(
            envs.get("TAU_PLUGIN_RUN_ID"),
            Some(&Some("run-7".to_string()))
        );
        assert_eq!(
            envs.get("TAU_PLUGIN_AGENT_ID"),
            Some(&Some("agent-9".to_string()))
        );
        assert_eq!(envs.get("PATH"), Some(&Some("/usr/bin:/bin".to_string())));

        // Non-allowlisted vars (even secret-looking ones) stay cleared.
        assert!(
            !envs.contains_key("AWS_SECRET_ACCESS_KEY"),
            "non-allowlisted secret must not cross into the plugin"
        );
        assert!(!envs.contains_key("HOME"));
    }

    #[test]
    fn emit_plugin_line_handles_json_and_raw_without_panicking() {
        // Smoke: both branches must not panic and must produce no
        // observable side effects without an installed subscriber.
        emit_plugin_line(
            "echo-llm",
            r#"{"level":"INFO","message":"hello","fields":{}}"#,
        );
        emit_plugin_line("echo-llm", "not-json-at-all");
        emit_plugin_line("echo-llm", r#"{"level":"ERROR","fields":{"message":"x"}}"#);
        emit_plugin_line("echo-llm", r#"{"level":"WARN","message":"y"}"#);
    }

    // ---- sandbox integration tests ----

    /// `spawn_and_handshake` must return `SandboxValidationFailed` (not
    /// `PluginSpawnFailed`) when the sandbox plan has capabilities that the
    /// adapter rejects. This proves validate_plan runs BEFORE wrap_spawn and
    /// BEFORE `Command::spawn` — a binary that doesn't exist would produce
    /// `PluginSpawnFailed` instead if the order were wrong.
    ///
    /// MockSandbox rejects `CapabilityShape::Custom`, so we use a plan with
    /// a Custom capability to trigger the rejection.
    #[tokio::test]
    async fn spawn_fails_on_validation_error() {
        use tau_domain::fixtures as domain_fixtures;
        use tau_ports::CapabilityPlan;

        use crate::process_gate::SandboxAdapter;

        // A plan with a Custom capability that MockSandbox cannot handle.
        let custom_cap = domain_fixtures::cap_custom("mcp.tool.use");
        let plan = CapabilityPlan::new(vec![custom_cap], None, None);

        // MockSandbox validates correctly — rejects Custom shapes.
        let adapter = SandboxAdapter::Mock(tau_ports::fixtures::MockCapabilityGate::new("mock"));

        let result = PluginProcess::spawn_and_handshake(
            // A binary path that doesn't exist — if we reach spawn, we'd
            // get PluginSpawnFailed instead of SandboxValidationFailed.
            std::path::Path::new("/nonexistent/plugin-binary"),
            "test-plugin".to_owned(),
            "run-id",
            "agent-id",
            tau_plugin_protocol::FramerOptions::default(),
            Duration::from_secs(2),
            None,
            Some((&plan, &adapter)),
            |_reader, _writer| Box::pin(async { Ok(()) }),
        )
        .await;

        match result {
            Err(crate::error::RuntimeError::SandboxValidationFailed { plugin, errors }) => {
                assert_eq!(plugin, "test-plugin");
                assert!(!errors.is_empty(), "at least one validation error expected");
            }
            Err(other) => panic!("expected SandboxValidationFailed, got: {other:?}"),
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }

    /// Validates ordering: when a VALID plan is used, the spawn proceeds past
    /// the sandbox checks. Here the binary doesn't exist, so we get
    /// `PluginSpawnFailed` (proving that validation + wrap_spawn ran first
    /// without error, and the error came at the actual spawn step).
    #[tokio::test]
    async fn spawn_calls_validate_plan_then_wrap_spawn() {
        use tau_ports::CapabilityPlan;

        use crate::process_gate::SandboxAdapter;

        // Empty plan — MockSandbox accepts this unconditionally.
        let plan = CapabilityPlan::new(vec![], None, None);
        let adapter = SandboxAdapter::Mock(tau_ports::fixtures::MockCapabilityGate::new("mock"));

        let result = PluginProcess::spawn_and_handshake(
            // Non-existent binary so spawn fails AFTER sandbox succeeds.
            std::path::Path::new("/nonexistent/plugin-binary"),
            "test-plugin".to_owned(),
            "run-id",
            "agent-id",
            tau_plugin_protocol::FramerOptions::default(),
            Duration::from_secs(2),
            None,
            Some((&plan, &adapter)),
            |_reader, _writer| Box::pin(async { Ok(()) }),
        )
        .await;

        // The sandbox accepted the plan; spawn failed because the binary
        // doesn't exist. This proves validate_plan + wrap_spawn both ran
        // successfully before Command::spawn was called.
        match result {
            Err(crate::error::RuntimeError::PluginSpawnFailed { .. }) => {
                // Expected: sandbox passed, binary missing → spawn failed.
            }
            Err(other) => {
                panic!("expected PluginSpawnFailed (sandbox passed, binary missing), got: {other}")
            }
            Ok(_) => panic!("expected Err, got Ok"),
        }
    }
}
