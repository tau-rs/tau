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
    tracing::install(&cli);
    // Capture `cli.debug` before `dispatch` consumes the parsed `Cli`.
    // When set, the error path renders the full `anyhow` chain via
    // `{err:?}` instead of the single-line top-level message. This is
    // the integration-level surface for `--debug` (the tracing module
    // already promotes the filter to DEBUG independently).
    let debug = cli.debug;
    match dispatch(cli).await {
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

async fn dispatch(cli: cli::Cli) -> anyhow::Result<()> {
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
        cli::Command::Workflow(sub) => cmd::workflow::dispatch(sub, &mut output).await,
        cli::Command::Serve(args) => cmd::serve::run(&args).await,
        cli::Command::Check(args) => cmd::check::run(args).await,
    }
}
