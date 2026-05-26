#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! tau-cli internals. The `tau` binary is a thin wrapper around
//! [`run_main`]; this lib exists so integration tests can drive
//! command logic without subprocess overhead.

pub mod cli;
pub mod cmd;
pub mod config;
pub mod exit;
pub mod output;
pub(crate) mod session;
pub mod tracing;

pub use config::{
    build_agent_definition, AgentEntry, AgentResolutionError, ProjectConfig, ProjectConfigError,
    PromptEntry, RequiresEntry,
};
pub use exit::ExitCode;
pub use output::{ColorChoice, Output};

use clap::Parser;

/// Top-level entry point used by `main` and integration tests.
///
/// Parses CLI arguments, dispatches to the appropriate `cmd::*::run`
/// handler, and maps the result to a process exit code.
///
/// At v0.1 of Task 4, all subcommand handlers are stubs that return
/// an "unimplemented" error. Tasks 10-14 land the real handlers.
pub async fn run_main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();
    // When the user is running `tau workflow run`, mint the run id and
    // attach a `WorkflowRunLogLayer` to the global subscriber so step
    // events written by the runner end up in the on-disk JSONL log.
    // The minted run id is then handed to the workflow runner via
    // `RunOpts::run_id` so both producer and layer point at the same
    // file. If scope resolution fails here the install proceeds without
    // the layer; the workflow handler will surface the same error
    // shortly after with a richer context message.
    let prepared_workflow_run = prepare_workflow_run_layer(&cli.command);
    let mut extra_layers: Vec<
        Box<
            dyn ::tracing_subscriber::Layer<::tracing_subscriber::Registry> + Send + Sync + 'static,
        >,
    > = Vec::new();
    let workflow_run_id = if let Some(prep) = prepared_workflow_run {
        extra_layers.push(Box::new(prep.layer));
        Some(prep.run_id)
    } else {
        None
    };
    // Sub-project D Task 5: when `--record-protocol <path>` is set,
    // install a `PluginRecordingLayer` aimed at that path. The same
    // layer handle is stashed in a process-global slot so the
    // top-level command handlers (cmd::run / cmd::chat / cmd::plugin
    // describe) can call `flush()` on exit to ensure pending writes
    // reach disk before the process terminates.
    //
    // The layer is wrapped with a per-layer `FilterFn` that always
    // enables the `tau::plugin::frame` target regardless of the
    // global `EnvFilter`, and pins its `max_level_hint` to TRACE so
    // the tracing macros don't short-circuit recording events when
    // the user's filter is more restrictive than INFO (e.g.
    // `--quiet` → `tau=warn`). Protocol recording is opt-in via the
    // explicit `--record-protocol` flag — gating it behind any
    // particular log-level directive would surprise users who
    // reasonably expect "the flag is set, ergo frames are recorded".
    if let Some(path) = cli.record_protocol.clone() {
        use tracing_subscriber::filter::{filter_fn, LevelFilter};
        use tracing_subscriber::layer::Layer as _;
        let layer = tau_observe::layers::plugin_recording::PluginRecordingLayer::new(path);
        crate::cmd::plugin_loader::set_plugin_recording_layer(layer.clone());
        let per_layer_filter =
            filter_fn(|meta| meta.target() == tau_observe::layers::plugin_recording::TARGET)
                .with_max_level_hint(LevelFilter::TRACE);
        let filtered = layer.with_filter(per_layer_filter);
        extra_layers.push(Box::new(filtered));
    }
    tracing::install_with_extra_layers(&cli, extra_layers);
    // Capture `cli.debug` before `dispatch` consumes the parsed `Cli`.
    // When set, the error path renders the full `anyhow` chain via
    // `{err:?}` instead of the single-line top-level message. This is
    // the integration-level surface for `--debug` (the tracing module
    // already promotes the filter to DEBUG independently).
    let debug = cli.debug;
    match dispatch(cli, workflow_run_id).await {
        Ok(()) => ExitCode::Success.into(),
        Err(err) => {
            // The AgentFailed marker is emitted by `cmd::run` when the
            // agent reaches `RunOutcome::Failed`. It must NOT print the
            // generic "error:" prefix — the run handler has already
            // emitted a structured failure to the user. All other
            // errors are kernel/CLI failures; they get the prefix and
            // map to `ExitCode::Error`.
            if err.downcast_ref::<crate::cmd::run::AgentFailed>().is_some() {
                ExitCode::AgentFailed.into()
            } else {
                if debug {
                    eprintln!("error: {err:?}");
                } else {
                    eprintln!("error: {err}");
                }
                ExitCode::from(&err).into()
            }
        }
    }
}

/// Outcome of inspecting the parsed CLI for a `tau workflow run`
/// invocation: a pre-minted run id and the run-log layer to install.
struct PreparedWorkflowRun {
    run_id: String,
    layer: tau_observe::layers::workflow_run_log::WorkflowRunLogLayer,
}

/// If `command` is `Workflow(Run(...))`, resolve the scope, mint a fresh
/// run id, and build the `WorkflowRunLogLayer` pointing at the same
/// `<scope>/.tau/workflow-runs/<name>-<run_id>.jsonl` path the runner
/// will use. Returns `None` for any other command, or when scope
/// resolution fails (the workflow handler will re-surface that error).
fn prepare_workflow_run_layer(command: &cli::Command) -> Option<PreparedWorkflowRun> {
    let cli::Command::Workflow(cli::WorkflowSubcommand::Run(run_args)) = command else {
        return None;
    };
    let cwd = std::env::current_dir().ok()?;
    let scope = tau_pkg::Scope::resolve(&cwd).ok()?;
    let run_id = ulid::Ulid::new().to_string();
    let log_path = tau_workflow::run_log_path(scope.path(), &run_args.name, &run_id);
    let layer = tau_observe::layers::workflow_run_log::WorkflowRunLogLayer::new(log_path);
    Some(PreparedWorkflowRun { run_id, layer })
}

async fn dispatch(cli: cli::Cli, workflow_run_id: Option<String>) -> anyhow::Result<()> {
    use cli::SandboxKindArg;
    use tau_runtime::sandbox::registry::RegistryKind;

    let mut output = Output::from_cli(&cli);
    let record_protocol = cli.record_protocol.clone();

    // Resolve sandbox override flags (--no-sandbox / --sandbox <kind>) once,
    // so every plugin-loading subcommand gets the same derived values.
    let force_passthrough =
        cli.no_sandbox || matches!(cli.sandbox, Some(SandboxKindArg::Passthrough));
    let force_adapter_kind: Option<RegistryKind> = match cli.sandbox {
        Some(SandboxKindArg::Native) => Some(RegistryKind::Native),
        Some(SandboxKindArg::Container) => Some(RegistryKind::Container),
        Some(SandboxKindArg::Passthrough) | None => None,
    };

    match cli.command {
        cli::Command::Init(args) => cmd::init::run(&args, &mut output).await,
        cli::Command::Install(args) => cmd::install::run(&args, &mut output).await,
        cli::Command::List(args) => cmd::list::run(&args, &mut output).await,
        cli::Command::Run(args) => {
            cmd::run::run(
                &args,
                record_protocol,
                force_passthrough,
                force_adapter_kind,
                &mut output,
            )
            .await
        }
        cli::Command::Chat(args) => {
            cmd::chat::run(
                &args,
                record_protocol,
                force_passthrough,
                force_adapter_kind,
                &mut output,
            )
            .await
        }
        cli::Command::Resolve(args) => cmd::resolve::run(&args, &mut output).await,
        cli::Command::Uninstall(args) => cmd::uninstall::run(&args, &mut output).await,
        cli::Command::Update(args) => cmd::update::run(&args, &mut output).await,
        cli::Command::Verify(args) => cmd::verify::run(&args, &mut output).await,
        cli::Command::Plugin { action } => {
            cmd::plugin::dispatch(action, record_protocol, &mut output).await
        }
        cli::Command::Session(args) => cmd::session::run(&args, &mut output).await,
        cli::Command::Sandbox(args) => cmd::sandbox::run(&args, &mut output).await,
        cli::Command::Skill(sub) => cmd::skill::dispatch(sub, &mut output).await,
        cli::Command::Target(sub) => cmd::target::run(&sub, &mut output).await,
        cli::Command::Workflow(sub) => {
            cmd::workflow::dispatch(sub, workflow_run_id, &mut output).await
        }
        cli::Command::Serve(args) => cmd::serve::run(&args).await,
        cli::Command::Check(args) => cmd::check::run(args).await,
    }
}
