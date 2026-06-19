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
    // Tripped when an outbound send fails (writer task gone). The dispatch
    // loop selects on it to begin graceful shutdown (audit O4).
    let writer_gone = tokio_util::sync::CancellationToken::new();
    let dispatcher = Dispatcher {
        project: project.clone(),
        runtime: Arc::new(runtime),
        handshake: HandshakeState::default(),
        cancel_reg: cancel_reg.clone(),
        max_concurrent: opts.max_concurrent,
        out_tx: out_tx.clone(),
        writer_gone: writer_gone.clone(),
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
    let writer_gone_signal = writer_gone.clone();
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
                _ = writer_gone_signal.cancelled() => {
                    warn!("writer task gone; shutting down dispatcher");
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

/// Resolve an agent entry's LLM backend package name via the project
/// `[models]` table (per-agent model resolution). `validate_models`
/// (D7 stage 1) refuses a project whose agent names an unknown alias, so this
/// is infallible for a validated config; the error arm is defense-in-depth.
fn agent_llm_backend(
    entry: &tau_pkg::project::project::AgentEntry,
    models: &std::collections::BTreeMap<String, tau_pkg::project::project::ModelEntry>,
) -> Result<String> {
    models
        .get(&entry.model)
        .map(|m| m.backend.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "agent {:?} references model alias {:?} absent from [models]",
                entry.id,
                entry.model
            )
        })
}

/// Build the `Runtime` from a loaded `Project`.
///
/// Iterates every agent in the project's `tau.toml`, reads the lockfile to
/// locate the LLM-backend and tool plugin binaries, and spawns them via
/// `tau_runtime_tokio::plugin_host`. Deduplicates by plugin name so that two
/// agents sharing the same backend/tool spawn only one subprocess.
///
/// Sandbox enforcement (audit S4): serve mirrors the CLI `run`/`chat`
/// path. It resolves one [`SandboxAdapter`] for the whole serve process
/// and builds a per-plugin [`tau_ports::CapabilityPlan`] from each
/// plugin's declared capabilities, then passes both into `load_*`.
/// `plugin_host` only enforces when both are present (`plan.zip(adapter)`),
/// so resolving the adapter is as load-bearing as building the plan —
/// passing one without the other silently disables the Layer-4 backstop.
///
/// Serve mode v1 simplifications (deferred to future iterations):
/// - Recorder / trace context: a synthetic trace context is generated
///   per plugin (no recorder ledger, no run-level trace propagation).
/// - `[agents.<id>.config]` is forwarded to the LLM backend as JSON;
///   tools always receive `{}` (per-tool config selectors not yet landed).
/// - `working_context` / resource `limits` are not yet threaded into the
///   plan (both `None`), matching the CLI tool path.
async fn build_runtime(project: &Project) -> Result<tau_runtime_tokio::Runtime> {
    use tau_pkg::LockFile;
    use tau_plugin_protocol::handshake::TraceContext;
    use tau_runtime_tokio::plugin_host;

    let lockfile_path = project.scope.lockfile_path();
    let lockfile = LockFile::load(&lockfile_path)
        .with_context(|| format!("load lockfile {}", lockfile_path.display()))?;

    // Resolve a single sandbox adapter for the serve process. Serve has no
    // `--no-sandbox` / `--sandbox` flags, so we always ask the resolver for
    // the best adapter satisfying the scope's `[sandbox]` requirements and
    // every referenced plugin's declared tier floor (audit S4).
    let scope_reqs = read_scope_sandbox_requirements(&project.scope)?;
    let plugin_reqs = gather_plugin_sandbox_reqs(&project.scope, &lockfile, &project.config);
    let adapter = resolve_serve_adapter(&scope_reqs, &plugin_reqs).await?;
    let mut host_options = plugin_host::PluginHostOptions::default();
    host_options.sandbox_adapter = Some(adapter);

    let mut builder = tau_runtime_tokio::Runtime::builder();

    // Dedup by plugin name — multiple agents may reference the same package.
    let mut seen_llm_backends: std::collections::HashSet<String> = Default::default();
    let mut seen_tools: std::collections::HashSet<String> = Default::default();

    for entry in project.config.agents.values() {
        // ---- LLM backend ----
        // The agent's backend package is derived from the project `[models]`
        // table via its `model` alias (per-agent model resolution); the
        // removed `AgentEntry.llm_backend` field no longer carries it.
        let backend_name = agent_llm_backend(entry, &project.config.models)?;
        if seen_llm_backends.insert(backend_name.clone()) {
            let pkg_name: tau_domain::PackageName = backend_name
                .parse()
                .with_context(|| format!("invalid LLM backend package name {backend_name:?}"))?;
            let pkg = lockfile.find(&pkg_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "LLM backend {:?} not installed in scope (run `tau install <url>`)",
                    backend_name
                )
            })?;
            let plugin = pkg.plugin.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "package {:?} has no [plugin] table in its tau.toml (data-only package \
                     cannot be used as an llm_backend)",
                    backend_name
                )
            })?;
            let trace_context = TraceContext::new(
                "serve".to_string(),
                entry.id.clone(),
                "serve-root".to_string(),
            );
            let config = agent_config_to_json(entry);
            // Build this plugin's capability plan from its manifest, narrowed
            // by the agent's project-level capability overrides (audit S4).
            let caps = read_plugin_caps(&project.scope, &lockfile, &backend_name);
            let plan = plan_for_plugin(&caps, &entry.capability_overrides).with_context(|| {
                format!("building sandbox plan for LLM backend {backend_name:?}")
            })?;
            let backend = plugin_host::load_llm_backend(
                plugin,
                config,
                trace_context,
                host_options.clone(),
                Some(&plan),
            )
            .await
            .with_context(|| format!("load LLM backend {backend_name:?}"))?;
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
                // Per-tool capability overrides are not addressable in the
                // project tau.toml at v0.1 — pass an empty override slice, as
                // the CLI tool path does (audit S4).
                let caps = read_plugin_caps(&project.scope, &lockfile, &tool_name);
                let plan = plan_for_plugin(&caps, &[])
                    .with_context(|| format!("building sandbox plan for tool {tool_name:?}"))?;
                let loaded = plugin_host::load_tool(
                    plugin,
                    serde_json::Value::Object(Default::default()),
                    trace_context,
                    host_options.clone(),
                    Some(&plan),
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

/// Resolve the sandbox adapter for serve mode (audit S4).
///
/// Mirrors the non-force branch of the CLI `run`/`chat` path
/// (`plugin_loader::load_plugins`): serve exposes no `--no-sandbox` /
/// `--sandbox` flags, so it always asks the resolver for the best adapter
/// satisfying the scope's `[sandbox]` requirements and every referenced
/// plugin's declared tier floor. The returned adapter is the half
/// `plugin_host` zips against each plugin's [`tau_ports::CapabilityPlan`];
/// without it the Layer-4 backstop is silently disabled.
async fn resolve_serve_adapter(
    requirements: &tau_pkg::scope::SandboxRequirements,
    plugin_reqs: &[tau_domain::PluginSandboxRequirements],
) -> Result<std::sync::Arc<tau_runtime_tokio::process_gate::SandboxAdapter>> {
    tau_runtime_tokio::process_gate::resolve_adapter(requirements, plugin_reqs)
        .await
        .map(std::sync::Arc::new)
        .map_err(|e| anyhow::anyhow!("resolve sandbox adapter for serve mode: {e}"))
}

/// Build a plugin's Layer-3 capability plan (audit S4).
///
/// Thin serve-side wrapper over [`tau_runtime_tokio::process_gate::build_plan`]:
/// serve defers `working_context` and resource `limits` (both `None`),
/// exactly like the CLI tool path. Returns a real `CapabilityPlan` so serve
/// can pass `Some(&plan)` into `load_*` and have `plugin_host` enforce it.
fn plan_for_plugin(
    caps: &[tau_domain::Capability],
    overrides: &[tau_pkg::capability_override::CapabilityOverride],
) -> Result<tau_ports::CapabilityPlan> {
    tau_runtime_tokio::process_gate::build_plan(caps, overrides, None, None)
        .map_err(|e| anyhow::anyhow!("build sandbox plan: {e}"))
}

/// Read the scope's `[sandbox]` requirements from `config.toml`, falling
/// back to defaults when the file doesn't exist (a freshly-created scope
/// without an explicit config). Mirrors `plugin_loader::load_plugins`.
fn read_scope_sandbox_requirements(
    scope: &tau_pkg::Scope,
) -> Result<tau_pkg::scope::SandboxRequirements> {
    let config_path = scope.config_path();
    if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading scope config at {config_path:?}"))?;
        let scope_config = tau_pkg::scope::ScopeConfig::read_from_str(&text)
            .with_context(|| format!("parsing scope config at {config_path:?}"))?;
        Ok(scope_config.sandbox)
    } else {
        Ok(tau_pkg::scope::SandboxRequirements::default())
    }
}

/// Collect the sandbox-tier floors declared by every plugin the project's
/// agents reference (LLM backends + tools), deduplicated by package name.
/// Feeds the resolver so a plugin demanding e.g. `Strict` can't be admitted
/// under a weaker adapter (audit S4).
fn gather_plugin_sandbox_reqs(
    scope: &tau_pkg::Scope,
    lockfile: &tau_pkg::LockFile,
    config: &tau_pkg::project::ProjectConfig,
) -> Vec<tau_domain::PluginSandboxRequirements> {
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut reqs = Vec::new();
    for entry in config.agents.values() {
        // Backend package derived from the project `[models]` table; skip
        // agents whose model alias does not resolve (validate_models refuses
        // these at build time — defensive skip here).
        if let Some(backend) = config.models.get(&entry.model).map(|m| m.backend.clone()) {
            if seen.insert(backend.clone()) {
                if let Some(req) = read_plugin_sandbox_req(scope, lockfile, &backend) {
                    reqs.push(req);
                }
            }
        }
        for tool in &entry.requires.tools {
            let name = tool.name.to_string();
            if seen.insert(name.clone()) {
                if let Some(req) = read_plugin_sandbox_req(scope, lockfile, &name) {
                    reqs.push(req);
                }
            }
        }
    }
    reqs
}

/// Read a plugin's `[sandbox]` requirements from its on-disk `tau.toml`.
///
/// Mirrors `plugin_loader::read_plugin_sandbox_req`. Returns `None` if the
/// package is absent from the lockfile (treated as "no floor", not an error).
fn read_plugin_sandbox_req(
    scope: &tau_pkg::Scope,
    lockfile: &tau_pkg::LockFile,
    package_name: &str,
) -> Option<tau_domain::PluginSandboxRequirements> {
    let pkg_name: tau_domain::PackageName = package_name.parse().ok()?;
    let locked = lockfile.find(&pkg_name)?;
    let pkg_dir = scope.package_dir(&locked.name, &locked.active_version);
    match tau_pkg::read_manifest(&pkg_dir.join("tau.toml")) {
        Ok(manifest) => Some(manifest.sandbox().clone()),
        Err(_) => Some(tau_domain::PluginSandboxRequirements::default()),
    }
}

/// Read a plugin's declared capabilities from its on-disk `tau.toml`, used
/// to construct its [`tau_ports::CapabilityPlan`].
///
/// Mirrors `plugin_loader::read_plugin_caps_by_name`. Returns an empty vec
/// when the package is absent from the lockfile or its manifest can't be
/// read — an empty plan denies everything, the safe default.
fn read_plugin_caps(
    scope: &tau_pkg::Scope,
    lockfile: &tau_pkg::LockFile,
    package_name: &str,
) -> Vec<tau_domain::Capability> {
    let Ok(pkg_name) = package_name.parse::<tau_domain::PackageName>() else {
        return Vec::new();
    };
    let Some(locked) = lockfile.find(&pkg_name) else {
        return Vec::new();
    };
    let pkg_dir = scope.package_dir(&locked.name, &locked.active_version);
    match tau_pkg::read_manifest(&pkg_dir.join("tau.toml")) {
        Ok(manifest) => manifest.capabilities().to_vec(),
        Err(_) => Vec::new(),
    }
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

// NB: keep this test module last — `clippy::items_after_test_module` (deny)
// rejects any item declared after a `#[cfg(test)] mod tests`.
#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::Capability;

    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("test capability JSON must be valid")
    }

    /// S4 regression — the adapter half.
    ///
    /// `plugin_host::load_*` enforces the sandbox only when **both** the
    /// capability plan and the adapter are present (`plan.zip(adapter)`).
    /// Serve previously passed `PluginHostOptions::default()`, whose
    /// `sandbox_adapter` is `None`, so the zip collapsed to `None` and every
    /// plugin ran unsandboxed. Serve must now resolve a real adapter.
    #[tokio::test]
    async fn serve_resolves_a_sandbox_adapter() {
        // Mock injection makes `resolve_adapter` deterministic across host
        // platforms (no real landlock/SBPL tooling needed in CI).
        std::env::set_var("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1");
        let reqs = tau_pkg::scope::SandboxRequirements::default();
        let result = resolve_serve_adapter(&reqs, &[]).await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let adapter = result.expect("serve must resolve a sandbox adapter");
        assert!(
            matches!(
                *adapter,
                tau_runtime_tokio::process_gate::SandboxAdapter::Mock(_)
            ),
            "expected a resolved adapter (the half plugin_host zips against the plan), \
             got a non-mock variant under TAU_TESTING_ALLOW_MOCK_SANDBOX",
        );
    }

    /// S4 regression — the plan half.
    ///
    /// Serve previously passed `sandbox_plan = None` to every `load_*` call.
    /// `plan_for_plugin` is the serve-side construction point that produces a
    /// real (non-`None`) `CapabilityPlan` carrying the plugin's declared
    /// capability shapes, which serve then passes as `Some(&plan)`.
    #[test]
    fn serve_builds_non_none_plan_carrying_declared_capabilities() {
        let caps = vec![
            cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#),
            cap(r#"{"kind":"net.http","hosts":["api.example.com"],"methods":["GET"]}"#),
        ];
        let plan = plan_for_plugin(&caps, &[]).expect("serve must build a plan");
        // A non-empty plan with both declared shapes preserved.
        assert_eq!(plan.capabilities.len(), 2);
        use tau_domain::CapabilityShape;
        assert_eq!(
            plan.capabilities[0].required_shape(),
            CapabilityShape::FilesystemRead
        );
        assert_eq!(
            plan.capabilities[1].required_shape(),
            CapabilityShape::NetworkHttp
        );
    }

    /// An empty manifest still yields a real, enforceable (empty) plan — never
    /// `None`. An empty plan means "deny everything", which is the safe default
    /// for a plugin that declared no capabilities.
    #[test]
    fn serve_plan_for_zero_capabilities_is_empty_not_none() {
        let plan = plan_for_plugin(&[], &[]).expect("serve must build a plan");
        assert!(plan.capabilities.is_empty());
    }
}
