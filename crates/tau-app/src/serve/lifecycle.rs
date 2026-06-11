//! Process lifecycle: startup, signals, graceful shutdown.

use super::cancel::CancelRegistry;
use super::dispatch::Dispatcher;
use super::framing;
use super::handshake::HandshakeState;
use super::idle::{self, Activity};
use super::options::ServeOptions;
use super::project::Project;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tracing::{info, warn};

/// Main serve entry point. Builds runtime, spawns tasks, blocks until shutdown.
pub async fn run(opts: ServeOptions) -> Result<()> {
    super::tracing_init::install();

    info!(project = %opts.project_path.display(), "serve starting");
    // Echo the resolved configuration so an operator can confirm from the
    // logs that their flags (`--idle-timeout`, `--max-concurrent`, …) took
    // effect (audit O1).
    info!(?opts, "serve config");

    let project = Arc::new(
        Project::load(&opts.project_path)
            .await
            .context("load project")?,
    );

    let runtime = build_runtime(&project).await.context("build runtime")?;

    let (in_tx, in_rx) = mpsc::channel(64);
    let (out_tx, out_rx) = mpsc::channel(256);

    // Linux: set PDEATHSIG so we die when parent dies.
    #[cfg(target_os = "linux")]
    set_pdeathsig();

    let cancel_reg = CancelRegistry::default();
    let dispatcher = Dispatcher {
        project: project.clone(),
        runtime: Arc::new(runtime),
        handshake: HandshakeState::default(),
        cancel_reg: cancel_reg.clone(),
        max_concurrent: opts.max_concurrent,
        out_tx: out_tx.clone(),
    };

    let local_set = LocalSet::new();

    // Shared idle clock — touched by the reader on every inbound line and by
    // the writer on every outbound message, so an in-flight run that keeps
    // streaming holds the timer open.
    let activity = Activity::new();

    // Reader and writer tasks are Send-friendly — spawn on multi-thread side.
    let reader_handle = tokio::spawn(framing::reader_task(in_tx, activity.clone()));
    let writer_handle = tokio::spawn(framing::writer_task(out_rx, activity.clone()));

    if opts.ready_on_stderr {
        eprintln!("tau-serve ready");
    }

    // Run dispatcher loop on the LocalSet so per-request tasks have a
    // current_thread executor available for non-Send streams.
    // spawn_local (used inside Dispatcher::spawn_run) works within any
    // active LocalSet on the current thread — no &LocalSet borrow needed.
    let idle_timeout = opts.idle_timeout;
    let dispatch_result = local_set
        .run_until(async move {
            let shutdown_signal = wait_for_shutdown_signal();
            // When no `--idle-timeout` is set, this future never resolves and
            // the select arm is effectively disabled.
            let idle = async {
                match idle_timeout {
                    Some(d) => idle::wait_for_idle(activity, d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                r = dispatcher.run(in_rx) => r,
                _ = shutdown_signal => Ok(()),
                _ = idle => {
                    info!(timeout = ?idle_timeout, "idle timeout reached, shutting down");
                    Ok(())
                }
            }
        })
        .await;

    // Graceful drain.
    cancel_reg.cancel_all();
    let grace_result = tokio::time::timeout(opts.shutdown_grace, async {
        let _ = reader_handle.await;
    })
    .await;
    if grace_result.is_err() {
        warn!(grace = ?opts.shutdown_grace, "shutdown grace expired");
    }
    drop(out_tx);
    let _ = writer_handle.await;

    info!("serve shutdown complete");
    dispatch_result?;
    Ok(())
}

/// Build the `Runtime` from a loaded `Project`.
///
/// Iterates every agent in the project's `tau.toml`, reads the lockfile to
/// locate the LLM-backend and tool plugin binaries, and spawns them via
/// `tau_runtime_tokio::plugin_host`. Deduplicates by plugin name so that two
/// agents sharing the same backend/tool spawn only one subprocess.
///
/// Serve mode v1 simplifications (deferred to future iterations):
/// - Sandbox is not enforced (`sandbox_plan = None`); plugins run
///   unsandboxed. `PluginHostOptions::sandbox_adapter` is `None`.
/// - Recorder / trace context: a synthetic trace context is generated
///   per plugin (no recorder ledger, no run-level trace propagation).
/// - `[agents.<id>.config]` is forwarded to the LLM backend as JSON;
///   tools always receive `{}` (per-tool config selectors not yet landed).
async fn build_runtime(project: &Project) -> Result<tau_runtime_tokio::Runtime> {
    use tau_pkg::LockFile;
    use tau_plugin_protocol::handshake::TraceContext;
    use tau_runtime_tokio::plugin_host;

    let lockfile_path = project.scope.lockfile_path();
    let lockfile = LockFile::load(&lockfile_path)
        .with_context(|| format!("load lockfile {}", lockfile_path.display()))?;

    let mut builder = tau_runtime_tokio::Runtime::builder();

    // Dedup by plugin name — multiple agents may reference the same package.
    let mut seen_llm_backends: std::collections::HashSet<String> = Default::default();
    let mut seen_tools: std::collections::HashSet<String> = Default::default();

    for entry in project.config.agents.values() {
        // ---- LLM backend ----
        if seen_llm_backends.insert(entry.llm_backend.clone()) {
            let pkg_name: tau_domain::PackageName =
                entry.llm_backend.parse().with_context(|| {
                    format!("invalid LLM backend package name {:?}", entry.llm_backend)
                })?;
            let pkg = lockfile.find(&pkg_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "LLM backend {:?} not installed in scope (run `tau install <url>`)",
                    entry.llm_backend
                )
            })?;
            let plugin = pkg.plugin.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "package {:?} has no [plugin] table in its tau.toml (data-only package \
                     cannot be used as an llm_backend)",
                    entry.llm_backend
                )
            })?;
            let trace_context = TraceContext::new(
                "serve".to_string(),
                entry.id.clone(),
                "serve-root".to_string(),
            );
            let config = agent_config_to_json(entry);
            let backend = plugin_host::load_llm_backend(
                plugin,
                config,
                trace_context,
                plugin_host::PluginHostOptions::default(),
                None,
            )
            .await
            .with_context(|| format!("load LLM backend {:?}", entry.llm_backend))?;
            builder = builder.with_dyn_llm_backend(backend);
        }

        // ---- Tools ----
        for tool in &entry.requires.tools {
            let tool_name = tool.name.to_string();
            if seen_tools.insert(tool_name.clone()) {
                let pkg = lockfile.find(&tool.name).ok_or_else(|| {
                    anyhow::anyhow!(
                        "tool {:?} not installed in scope (run `tau install <url>`)",
                        tool_name
                    )
                })?;
                let plugin = pkg.plugin.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "package {:?} has no [plugin] table in its tau.toml (data-only package \
                         cannot be used as a tool)",
                        tool_name
                    )
                })?;
                let trace_context = TraceContext::new(
                    "serve".to_string(),
                    entry.id.clone(),
                    "serve-root".to_string(),
                );
                let loaded = plugin_host::load_tool(
                    plugin,
                    serde_json::Value::Object(Default::default()),
                    trace_context,
                    plugin_host::PluginHostOptions::default(),
                    None,
                )
                .await
                .with_context(|| format!("load tool {:?}", tool_name))?;
                builder = builder.with_dyn_tool(loaded);
            }
        }
    }

    // Use `build_allow_empty` so a project with zero agents still starts
    // successfully. Serve mode accepts `meta.handshake` and `meta.ping` on
    // any project; `runtime.run` calls will return -32010 UNKNOWN_AGENT
    // which is the correct behaviour when no agents are defined.
    builder.build_allow_empty().context("build Runtime")
}

/// Convert an agent's `[agents.<id>.config]` TOML sub-table to a
/// `serde_json::Value` for the plugin handshake.
///
/// Returns `{}` (not `Null`) when no config is set: plugin config structs
/// typically don't deserialize from JSON null, so an empty object lets every
/// plugin with default-able config construct its defaults.
fn agent_config_to_json(entry: &tau_pkg::project::AgentEntry) -> serde_json::Value {
    if entry.config.is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    let mut map = serde_json::Map::with_capacity(entry.config.len());
    for (k, v) in &entry.config {
        map.insert(k.clone(), toml_to_json(v.clone()));
    }
    serde_json::Value::Object(map)
}

fn toml_to_json(v: toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s),
        toml::Value::Integer(i) => serde_json::Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(toml_to_json).collect())
        }
        toml::Value::Table(t) => {
            serde_json::Value::Object(t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect())
        }
    }
}

/// Wait for any of: SIGTERM, SIGINT (Unix), Ctrl-C (all platforms).
///
/// Unix uses dedicated `SignalKind::terminate` + `SignalKind::interrupt`
/// streams (`tokio::signal::unix`). Windows has no `unix` submodule;
/// we use `tokio::signal::ctrl_c` as the cross-platform interrupt
/// signal. SIGTERM has no analog on Windows — parent-death detection
/// there relies on stdin EOF (see lifecycle::run).
#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut int = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(_) => return,
    };
    tokio::select! {
        _ = term.recv() => info!("received SIGTERM"),
        _ = int.recv() => info!("received SIGINT"),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("received Ctrl-C");
}

/// On Linux, ask the kernel to deliver SIGTERM to us when our parent dies.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn set_pdeathsig() {
    // SAFETY: prctl is async-signal-safe; the SIGTERM target is the
    // current process which always exists. This is the only unsafe
    // block in tau-app — see the crate-level lib.rs note for context.
    unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGTERM as libc::c_ulong,
            0,
            0,
            0,
        );
    }
}
