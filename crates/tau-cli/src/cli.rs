//! Command-line argument parser. clap v4 derive.
//!
//! [`Cli`] is the top-level parser; [`Command`] enumerates the v0.1
//! subcommands plus the debug-tier `plugin` subcommand group. Each
//! subcommand has its own `*Args` struct.
//!
//! Global flags (verbose, quiet, debug, color, json, record-protocol)
//! are accessible from any subcommand via the parsed [`Cli`].

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Top-level parser for the `tau` binary.
#[derive(Parser, Debug)]
#[command(name = "tau", version, about = "tau runtime CLI")]
pub struct Cli {
    /// The subcommand to dispatch.
    #[command(subcommand)]
    pub command: Command,

    /// Verbosity level. Repeat for more (-v: DEBUG, -vv: TRACE).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-error output (sets tracing to WARN).
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Show full error chain + DEBUG-level tracing on failures.
    #[arg(long, global = true)]
    pub debug: bool,

    /// Color mode for terminal output.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub color: ColorMode,

    /// Emit structured JSON output instead of human-readable.
    /// Only supported on `install`, `list`, `run`, `init`.
    #[arg(long, global = true)]
    pub json: bool,

    /// Mirror every plugin protocol frame to a JSONL file at this path.
    ///
    /// Useful for debugging plugin behavior and replaying via
    /// `tau plugin protocol decode <path>`. The recording is best-effort
    /// per spec §7.8: write/open failures are logged but never abort the
    /// invocation.
    #[arg(long, global = true, value_name = "PATH")]
    pub record_protocol: Option<PathBuf>,

    /// Disable sandboxing entirely for this invocation (shorthand for
    /// `--sandbox passthrough`). For development and one-off debugging.
    /// Auditable in shell history.
    #[arg(long, global = true)]
    pub no_sandbox: bool,

    /// Force a specific sandbox adapter kind, overriding the resolver.
    /// `--no-sandbox` is shorthand for `--sandbox passthrough`. Conflicts
    /// with `--no-sandbox` if both set.
    #[arg(long, global = true, value_enum, conflicts_with = "no_sandbox")]
    pub sandbox: Option<SandboxKindArg>,
}

/// CLI value for `--sandbox <kind>`. Maps to the resolver's adapter
/// kinds. `passthrough` is exposed so `--sandbox passthrough` is a
/// fully-spelled equivalent of `--no-sandbox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SandboxKindArg {
    /// Linux landlock + seccomp + namespaces.
    Native,
    /// Docker / Podman shell-out.
    Container,
    /// No isolation (explicit opt-out).
    Passthrough,
}

/// All v0.1 subcommands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scaffold a project `tau.toml`.
    Init(InitArgs),
    /// Install a package from a Git URL.
    Install(InstallArgs),
    /// List installed packages or available agents.
    List(ListArgs),
    /// Invoke an agent one-shot.
    Run(RunArgs),
    /// Open a REPL chat session with an agent.
    Chat(ChatArgs),
    /// Install missing requires.tools dependencies for all agents in
    /// the project tau.toml. Project-wide form of the lazy resolve
    /// that `tau run` and `tau chat` perform per-agent.
    Resolve(ResolveArgs),
    /// Uninstall a package and remove its lockfile entry.
    Uninstall(UninstallArgs),
    /// Verify installed packages against the lockfile (spec §3).
    Verify(VerifyArgs),
    /// Update an installed package to a newer or specific version (spec §2).
    Update(UpdateArgs),
    /// Plugin debugging utilities (spec §9 debug tier).
    Plugin {
        /// Sub-action within the plugin group.
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Session management (list, show, delete, export).
    Session(SessionArgs),
    /// Sandbox configuration and diagnostics.
    Sandbox(SandboxArgs),
    /// Workflow subcommand group (list, run, log, resume).
    #[command(subcommand)]
    Workflow(WorkflowSubcommand),
    /// Inspect installed skill packages (list, show).
    #[command(subcommand)]
    Skill(SkillSubcommand),
    /// Inspect the deployment-target registry (list, show).
    #[command(subcommand)]
    Target(TargetSubcommand),
    /// Start serve mode: accept JSON-RPC requests over stdio.
    Serve(ServeArgs),
    /// Run pre-flight validation against the project (config, lockfile,
    /// packages, sandbox, plugins, skills). Aggregates existing validators
    /// into one CI/IDE-friendly verb.
    Check(CheckArgs),
}

/// `tau plugin <action>` — debug-tier helpers per spec §9.
#[derive(Subcommand, Debug)]
pub enum PluginAction {
    /// Print a plugin's handshake metadata + per-method schemas.
    Describe(PluginDescribeArgs),
    /// Run a plugin standalone (without an agent context).
    Run(PluginRunArgs),
    /// Inspect or transform plugin-protocol recordings.
    Protocol {
        /// Sub-action within the protocol group.
        #[command(subcommand)]
        action: PluginProtocolAction,
    },
}

/// `tau plugin describe` arguments.
#[derive(Args, Debug)]
pub struct PluginDescribeArgs {
    /// Plugin package name (must be installed in the project or global scope).
    pub name: String,
}

/// `tau plugin run` arguments.
#[derive(Args, Debug)]
pub struct PluginRunArgs {
    /// Path to the plugin binary.
    pub binary: PathBuf,
    /// REPL mode: read `<method> <json-args>` lines from stdin.
    #[arg(long, conflicts_with = "script")]
    pub interactive: bool,
    /// Scripted mode: read input from a JSONL file (one
    /// `{ "method": "...", "params": [...] }` object per line).
    #[arg(long, value_name = "PATH")]
    pub script: Option<PathBuf>,
}

/// `tau plugin protocol <action>` — recording-related utilities.
#[derive(Subcommand, Debug)]
pub enum PluginProtocolAction {
    /// Decode a JSONL recording into a human-readable transcript.
    Decode(PluginProtocolDecodeArgs),
}

/// `tau plugin protocol decode` arguments.
#[derive(Args, Debug)]
pub struct PluginProtocolDecodeArgs {
    /// Path to the recording file.
    pub path: PathBuf,
    /// Filter by `key=value` predicate (e.g. `plugin=echo-llm`,
    /// `method=llm.complete`, `dir=h2p`). May be supplied multiple times;
    /// frames must satisfy every predicate to be emitted.
    #[arg(long, value_name = "K=V")]
    pub filter: Vec<String>,
    /// Time-range start (Unix timestamp seconds, fractional ok).
    #[arg(long)]
    pub from: Option<f64>,
    /// Time-range end (Unix timestamp seconds, fractional ok).
    #[arg(long)]
    pub to: Option<f64>,
    /// Emit one decoded JSON object per line (machine-readable).
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `tau session`.
#[derive(Args, Debug)]
pub struct SessionArgs {
    /// Sub-action within the session group.
    #[command(subcommand)]
    pub action: SessionAction,
}

/// Sub-actions of `tau session`.
#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List sessions in the current scope.
    List(SessionListArgs),
    /// Print transcript of a session.
    Show(SessionShowArgs),
    /// Delete a session.
    Delete(SessionDeleteArgs),
    /// Export a session in a specific format.
    Export(SessionExportArgs),
}

/// Arguments for `tau session list`.
#[derive(Args, Debug)]
pub struct SessionListArgs {
    /// Filter by agent name.
    pub agent: Option<String>,
    /// Use global scope instead of project scope.
    #[arg(long)]
    pub global: bool,
    /// Maximum number of sessions to display (default 20).
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Disable the limit; show all sessions.
    #[arg(long)]
    pub all: bool,
}

/// Arguments for `tau session show`.
#[derive(Args, Debug)]
pub struct SessionShowArgs {
    /// Session id (or 8+ char prefix).
    pub id: String,
    /// Use global scope.
    #[arg(long)]
    pub global: bool,
}

/// Arguments for `tau session delete`.
#[derive(Args, Debug)]
pub struct SessionDeleteArgs {
    /// Session id (or 8+ char prefix).
    pub id: String,
    /// Use global scope.
    #[arg(long)]
    pub global: bool,
    /// Skip the confirmation prompt.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

/// Export format for `tau session export`.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ExportFormat {
    /// Raw JSONL passthrough (cat-equivalent).
    Jsonl,
    /// Markdown render (same as `tau session show` human mode).
    Md,
    /// Single envelope JSON object containing header + messages.
    Json,
}

/// Arguments for `tau session export`.
#[derive(Args, Debug)]
pub struct SessionExportArgs {
    /// Session id (or 8+ char prefix).
    pub id: String,
    /// Output format.
    #[arg(long, value_enum, default_value_t = ExportFormat::Jsonl)]
    pub format: ExportFormat,
    /// Use global scope.
    #[arg(long)]
    pub global: bool,
}

/// Arguments for `tau init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Overwrite an existing tau.toml.
    #[arg(long)]
    pub force: bool,
    /// Print what would be created without writing files.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for `tau install`.
#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Git URL of the package to install.
    pub url: String,
    /// Install into the global scope (~/.tau) instead of the project scope.
    #[arg(long)]
    pub global: bool,
    /// Validate the install without writing to disk or updating the lockfile.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for `tau list`.
#[derive(Args, Debug)]
pub struct ListArgs {
    /// What to list. Defaults to `packages`.
    #[arg(value_enum, default_value = "packages")]
    pub resource: ListResource,
    /// Restrict the listing to globally-installed entries.
    /// (Ignored when `resource` is `agents`; agents are always project-scoped.)
    #[arg(long, conflicts_with = "all")]
    pub global: bool,
    /// List both project and global entries side-by-side.
    /// (Ignored when `resource` is `agents`; agents are always project-scoped.)
    #[arg(long)]
    pub all: bool,
    /// When listing agents, also print the effective capability set
    /// (allow + deny per agent, computed against the package manifest).
    /// Ignored when `resource` is `packages`.
    #[arg(long)]
    pub capabilities: bool,
    /// Rejected explicitly: `tau list` is read-only.
    #[arg(long, hide = true)]
    pub dry_run: bool,
}

/// Arguments for `tau resolve`.
#[derive(Args, Debug)]
pub struct ResolveArgs {
    /// Skip install; print missing-deps hints and exit non-zero if
    /// anything would need fetching.
    #[arg(long)]
    pub no_install: bool,
    /// Print the resolution plan without fetching anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Run sandbox capability cross-check (Layer 3 validation) against
    /// the configured adapter; report any plugin whose plan is rejected.
    /// Exit 0 if all OK; exit 2 if any violation OR no adapter available.
    #[arg(long)]
    pub check_sandbox: bool,
}

/// Arguments for `tau uninstall`.
#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Package name to uninstall.
    pub package: String,
    /// Specific version (default: all versions).
    #[arg(long)]
    pub version: Option<String>,
    /// Use the global scope (~/.tau) instead of the project scope.
    #[arg(long)]
    pub global: bool,
}

/// Arguments for `tau verify`.
#[derive(Args, Debug)]
pub struct VerifyArgs {
    /// Package name to verify (default: all installed packages).
    pub package: Option<String>,
    /// Specific version.
    #[arg(long)]
    pub version: Option<String>,
    /// Use global scope.
    #[arg(long)]
    pub global: bool,
    /// Skills-5: in addition to the standard drift check, validate
    /// each installed skill against the Anthropic Agent Skills
    /// spec (non-empty description, non-empty body, well-formed
    /// frontmatter).
    #[arg(long)]
    pub anthropic_strict: bool,
}

/// Arguments for `tau update`.
#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Package name to update.
    pub package: String,
    /// Specific version (default: latest tag).
    #[arg(long)]
    pub version: Option<String>,
    /// Remove the old active version after the new install succeeds.
    #[arg(long)]
    pub prune: bool,
    /// Use global scope.
    #[arg(long)]
    pub global: bool,
}

/// Arguments for `tau run`.
#[derive(Args, Debug)]
pub struct RunArgs {
    /// Identifier of the agent to invoke.
    pub agent_id: String,
    /// Prompt for the agent. If omitted, read from stdin.
    pub prompt: Option<String>,
    /// Override RunOptions::max_turns.
    #[arg(long)]
    pub max_turns: Option<u32>,
    /// Validate setup without invoking the LLM.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip auto-install of missing requires.tools dependencies. If
    /// anything would need fetching, exit 2 with copy-pasteable
    /// `tau install <url>` hints instead.
    #[arg(long)]
    pub no_install: bool,
    /// Stream events as they arrive. Text deltas appear inline on stdout;
    /// tool annotations on stderr. With --json, emits one JSON object per
    /// event line (text_delta / tool_call_started / tool_call_completed /
    /// turn_completed / run_completed).
    #[arg(long, default_value_t = false)]
    pub stream: bool,
    /// Maximum cumulative input + output tokens across all agents
    /// spawned in the run. When the budget is exceeded, the run
    /// aborts with `RunStatus::Aborted`. Unset = no limit.
    #[arg(long, value_name = "TOKENS")]
    pub max_total_tokens: Option<u64>,
    /// Maximum wall-clock duration of the run, in seconds. When the
    /// budget is exceeded, the run aborts with `RunStatus::Aborted`.
    /// Unset = no limit.
    #[arg(long, value_name = "SECS")]
    pub max_total_duration_secs: Option<u64>,
    /// Maximum number of agents that may be spawned across the run
    /// (root + descendants). When exceeded, the run aborts with
    /// `RunStatus::Aborted`. Unset = no limit.
    #[arg(long, value_name = "N")]
    pub max_total_agents: Option<u32>,
}

/// Arguments for `tau chat`.
#[derive(Args, Debug)]
pub struct ChatArgs {
    /// Identifier of the agent to chat with.
    pub agent_id: String,
    /// Override RunOptions::max_turns.
    #[arg(long)]
    pub max_turns: Option<u32>,
    /// Validate setup without entering the REPL.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip auto-install of missing requires.tools dependencies. If
    /// anything would need fetching, exit 2 with copy-pasteable
    /// `tau install <url>` hints instead.
    #[arg(long)]
    pub no_install: bool,
    /// Disable streaming output. Renders the full response after each turn
    /// instead of typewriter-style as it arrives.
    #[arg(long, default_value_t = false)]
    pub no_stream: bool,
    /// Don't persist this session to disk; in-memory only.
    #[arg(long, default_value_t = false)]
    pub ephemeral: bool,
    /// Resume an existing session (id or 8+ char prefix).
    #[arg(long)]
    pub resume: Option<String>,
    /// Override drift detection on resume.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

/// Resource kinds accepted by `tau list`.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ListResource {
    /// List installed packages.
    Packages,
    /// List available agents.
    Agents,
}

/// Color output mode for terminal rendering.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ColorMode {
    /// Always emit ANSI color codes.
    Always,
    /// Auto-detect based on terminal capabilities.
    Auto,
    /// Never emit color codes.
    Never,
}

/// `tau sandbox` subcommand group.
#[derive(Debug, Args)]
pub struct SandboxArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: SandboxCommand,
}

/// `tau sandbox <subcommand>` variants.
#[derive(Debug, Subcommand)]
pub enum SandboxCommand {
    /// Print sandbox configuration + per-adapter probe results.
    /// Non-mutating; always exits 0.
    Status,
    /// Interactive (or non-interactive) wizard to write the `[sandbox]`
    /// block in `<scope>/config.toml`. Implemented in Task 10.
    Setup(SandboxSetupArgs),
}

/// Args for `tau sandbox setup`. Filled in by Task 10.
#[derive(Debug, Args)]
pub struct SandboxSetupArgs {
    /// Required tier to write to scope config (skips interactive prompt).
    #[arg(long, value_enum)]
    pub tier: Option<SandboxRequiredTierArg>,
    /// Disable interactive prompts; expects --tier to be provided.
    #[arg(long)]
    pub non_interactive: bool,
}

/// CLI value for `--tier` on `tau sandbox setup`. Mirrors
/// `tau_pkg::scope::SandboxRequiredTier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SandboxRequiredTierArg {
    /// No isolation required (allows passthrough).
    None,
    /// Filesystem isolation at minimum.
    Light,
    /// Full strict tier required.
    Strict,
}

/// `tau workflow <subcommand>` variants.
#[derive(Debug, clap::Subcommand)]
pub enum WorkflowSubcommand {
    /// List workflows declared under workflows/ in the current project.
    List,
    /// Run a workflow.
    Run(WorkflowRunArgs),
    /// Pretty-print a workflow run's JSONL log.
    Log(WorkflowLogArgs),
    /// Resume a workflow run from its saved JSONL log.
    Resume(WorkflowResumeArgs),
}

/// Arguments for `tau workflow run`.
#[derive(Debug, clap::Args)]
pub struct WorkflowRunArgs {
    /// Workflow name (matches `workflows/<name>.toml`).
    pub name: String,
    /// Input string passed to the first step as ${input}.
    #[arg(long, default_value = "")]
    pub input: String,
}

/// Arguments for `tau workflow log`.
#[derive(Debug, clap::Args)]
pub struct WorkflowLogArgs {
    /// Run id (ULID) from a prior `tau workflow run`.
    pub run_id: String,
    /// Emit raw JSONL lines instead of the pretty view.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `tau workflow resume`.
#[derive(Debug, clap::Args)]
pub struct WorkflowResumeArgs {
    /// Run id (ULID) of the prior incomplete run.
    pub run_id: String,
    /// Override drift detection (workflow file changed since the original run).
    #[arg(long)]
    pub force: bool,
}

/// `tau skill <subcommand>` variants.
#[derive(Debug, clap::Subcommand)]
pub enum SkillSubcommand {
    /// List all installed skill packages in the current scope.
    List(SkillListArgs),
    /// Show detail for one installed skill.
    Show(SkillShowArgs),
    /// Import an Anthropic-format skill source as a tau-skill directory
    /// (Skills-5). Synthesizes a tau.toml; does NOT install. Run
    /// `tau install <output-dir>` afterwards.
    Import(SkillImportArgs),
    /// Export an installed skill to a vanilla Anthropic-format directory
    /// (Skills-5). Drops tau.toml + capabilities.
    Export(SkillExportArgs),
}

/// Arguments for `tau skill list`.
#[derive(Debug, clap::Args)]
pub struct SkillListArgs {
    /// Emit canonical JSON instead of the default human-formatted table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `tau skill show`.
#[derive(Debug, clap::Args)]
pub struct SkillShowArgs {
    /// Skill package name (exact match).
    pub name: String,
    /// Emit canonical JSON instead of the default human-formatted summary.
    #[arg(long)]
    pub json: bool,
    /// Include the SKILL.md body in the output. Rendered for terminal
    /// display by default; combine with --raw for verbatim file bytes.
    #[arg(long)]
    pub body: bool,
    /// With --body: emit verbatim file bytes instead of rendered output.
    /// Implies --body (clap `requires` enforces). Useful for piping to
    /// grep, diff, less.
    #[arg(long, requires = "body")]
    pub raw: bool,
}

/// Arguments for `tau skill import`.
#[derive(Debug, clap::Args)]
pub struct SkillImportArgs {
    /// Source URL or local path to import from (an Anthropic-format skill
    /// directory or git URL). Local paths are copied; https/git@ URLs are
    /// cloned with `git clone`.
    pub source: String,

    /// Output directory to write the synthesized tau-skill into.
    #[arg(long, short)]
    pub output: PathBuf,

    /// Overwrite an existing output directory.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `tau skill export`.
#[derive(Debug, clap::Args)]
pub struct SkillExportArgs {
    /// Name of the installed skill to export.
    pub name: String,

    /// Output directory.
    #[arg(long, short)]
    pub output: PathBuf,

    /// Fail (instead of silently dropping) if anything tau-specific
    /// would be lost in the Anthropic-format export (e.g.
    /// capabilities or requires_skills).
    #[arg(long)]
    pub strict: bool,

    /// Overwrite an existing output directory.
    #[arg(long)]
    pub force: bool,
}

/// `tau target <subcommand>` — inspect the deployment-target registry.
#[derive(Debug, clap::Subcommand)]
pub enum TargetSubcommand {
    /// List all registered target triples.
    List(TargetListArgs),
    /// Show detail for one target triple.
    Show(TargetShowArgs),
}

/// Arguments for `tau target list`.
#[derive(Debug, clap::Args)]
pub struct TargetListArgs {
    /// Include Reserved triples (default: Available only).
    #[arg(long)]
    pub all: bool,
    /// Emit canonical JSON instead of the human-formatted table.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `tau target show`.
#[derive(Debug, clap::Args)]
pub struct TargetShowArgs {
    /// Triple to show (e.g. `linux-native-strict`).
    pub triple: String,
    /// Emit canonical JSON instead of the human-formatted summary.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `tau check`.
///
/// See spec at `docs/superpowers/specs/2026-05-18-tau-check-design.md` §5.
#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Optional category — runs only the named check (one of:
    /// config, lockfile, packages, sandbox, plugins, skills).
    /// When omitted, runs all 6 categories.
    #[arg(value_name = "CATEGORY", value_parser = ["config", "lockfile", "packages", "sandbox", "plugins", "skills"])]
    pub category: Option<String>,

    /// Reduce per-check I/O where a fast variant exists.
    /// No-op on categories without a fast variant (config, lockfile, packages).
    #[arg(long)]
    pub fast: bool,

    /// Install missing required packages before running checks.
    #[arg(long)]
    pub auto_resolve: bool,

    /// Project root override. Default: cwd (walks up for tau.toml).
    #[arg(long, value_name = "PATH")]
    pub project: Option<std::path::PathBuf>,

    /// Emit JSONL events to stdout. Mutually exclusive with --sarif.
    #[arg(long, conflicts_with = "sarif")]
    pub json: bool,

    /// Emit SARIF 2.1.0 document. With path → write file; without → stdout.
    /// Mutually exclusive with --json.
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = "-")]
    pub sarif: Option<String>,

    /// Validate against a specific target triple instead of the locally
    /// resolved adapter. See `tau target list` for valid values.
    #[arg(long, value_name = "TRIPLE")]
    pub target: Option<String>,
}

/// Arguments for `tau serve`.
#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Path to the tau project directory. Defaults to the current directory.
    #[arg(long, value_name = "PATH")]
    pub project: Option<std::path::PathBuf>,

    /// Maximum concurrent in-flight runs. Defaults to 8.
    #[arg(long, value_name = "N")]
    pub max_concurrent: Option<usize>,

    /// Initiate graceful shutdown after no message activity for this many seconds.
    #[arg(long, value_name = "SECS")]
    pub idle_timeout: Option<u64>,

    /// Write "tau-serve ready" to stderr after startup completes.
    #[arg(long)]
    pub ready_on_stderr: bool,

    /// Seconds to wait for in-flight tasks during graceful shutdown.
    #[arg(long, value_name = "SECS", default_value_t = 5)]
    pub shutdown_grace: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_install_url() {
        let cli = Cli::parse_from(["tau", "install", "https://github.com/foo/bar.git"]);
        let Command::Install(args) = cli.command else {
            panic!("expected Install: {:?}", cli.command)
        };
        assert_eq!(args.url, "https://github.com/foo/bar.git");
        assert!(!args.global);
        assert!(!args.dry_run);
    }

    #[test]
    fn parses_install_with_global_and_dry_run() {
        let cli = Cli::parse_from([
            "tau",
            "install",
            "https://github.com/foo/bar.git",
            "--global",
            "--dry-run",
        ]);
        let Command::Install(args) = cli.command else {
            panic!()
        };
        assert!(args.global);
        assert!(args.dry_run);
    }

    #[test]
    fn parses_run_with_prompt() {
        let cli = Cli::parse_from(["tau", "run", "agent-x", "hello world"]);
        let Command::Run(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.agent_id, "agent-x");
        assert_eq!(args.prompt.as_deref(), Some("hello world"));
    }

    #[test]
    fn parses_run_without_prompt() {
        let cli = Cli::parse_from(["tau", "run", "agent-x"]);
        let Command::Run(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.agent_id, "agent-x");
        assert_eq!(args.prompt, None);
    }

    #[test]
    fn parses_run_max_turns_override() {
        let cli = Cli::parse_from(["tau", "run", "agent-x", "hi", "--max-turns", "5"]);
        let Command::Run(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.max_turns, Some(5));
    }

    #[test]
    fn parses_list_default_resource_is_packages() {
        let cli = Cli::parse_from(["tau", "list"]);
        let Command::List(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.resource, ListResource::Packages));
    }

    #[test]
    fn parses_list_agents_resource() {
        let cli = Cli::parse_from(["tau", "list", "agents"]);
        let Command::List(args) = cli.command else {
            panic!()
        };
        assert!(matches!(args.resource, ListResource::Agents));
    }

    #[test]
    fn parses_list_agents_with_capabilities_flag() {
        let args = Cli::try_parse_from(["tau", "list", "agents", "--capabilities"]).unwrap();
        let Command::List(args) = args.command else {
            panic!("expected List command")
        };
        assert!(matches!(args.resource, ListResource::Agents));
        assert!(args.capabilities);
    }

    #[test]
    fn parses_chat() {
        let cli = Cli::parse_from(["tau", "chat", "agent-x"]);
        let Command::Chat(args) = cli.command else {
            panic!()
        };
        assert_eq!(args.agent_id, "agent-x");
    }

    #[test]
    fn parses_init_with_force() {
        let cli = Cli::parse_from(["tau", "init", "--force"]);
        let Command::Init(args) = cli.command else {
            panic!()
        };
        assert!(args.force);
    }

    #[test]
    fn parses_resolve_check_sandbox() {
        let cli = Cli::parse_from(["tau", "resolve", "--check-sandbox"]);
        let Command::Resolve(args) = cli.command else {
            panic!("expected Resolve: {:?}", cli.command)
        };
        assert!(args.check_sandbox);
        assert!(!args.no_install);
        assert!(!args.dry_run);
    }

    #[test]
    fn parses_global_verbose_flag() {
        let cli = Cli::parse_from(["tau", "-v", "list"]);
        assert_eq!(cli.verbose, 1);
    }

    #[test]
    fn parses_global_double_verbose_flag() {
        let cli = Cli::parse_from(["tau", "-vv", "list"]);
        assert_eq!(cli.verbose, 2);
    }

    #[test]
    fn parses_global_color_flag() {
        let cli = Cli::parse_from(["tau", "--color", "never", "list"]);
        assert!(matches!(cli.color, ColorMode::Never));
    }

    #[test]
    fn list_global_and_all_are_mutually_exclusive() {
        let result = Cli::try_parse_from(["tau", "list", "--global", "--all"]);
        assert!(result.is_err(), "expected mutual exclusion error");
    }

    #[test]
    fn parses_record_protocol_global_flag() {
        let cli = Cli::parse_from(["tau", "--record-protocol", "/tmp/p.jsonl", "list"]);
        assert_eq!(
            cli.record_protocol.as_deref().and_then(|p| p.to_str()),
            Some("/tmp/p.jsonl")
        );
    }

    #[test]
    fn parses_plugin_describe() {
        let cli = Cli::parse_from(["tau", "plugin", "describe", "anthropic"]);
        let Command::Plugin {
            action: PluginAction::Describe(args),
        } = cli.command
        else {
            panic!("expected Plugin::Describe: {:?}", cli.command);
        };
        assert_eq!(args.name, "anthropic");
    }

    #[test]
    fn parses_plugin_run_interactive() {
        let cli = Cli::parse_from([
            "tau",
            "plugin",
            "run",
            "/usr/local/bin/echo-llm",
            "--interactive",
        ]);
        let Command::Plugin {
            action: PluginAction::Run(args),
        } = cli.command
        else {
            panic!()
        };
        assert!(args.interactive);
        assert!(args.script.is_none());
        assert_eq!(args.binary.to_str(), Some("/usr/local/bin/echo-llm"));
    }

    #[test]
    fn parses_plugin_run_script() {
        let cli = Cli::parse_from([
            "tau",
            "plugin",
            "run",
            "/usr/local/bin/echo-llm",
            "--script",
            "/tmp/script.jsonl",
        ]);
        let Command::Plugin {
            action: PluginAction::Run(args),
        } = cli.command
        else {
            panic!()
        };
        assert!(!args.interactive);
        assert_eq!(
            args.script.as_deref().and_then(|p| p.to_str()),
            Some("/tmp/script.jsonl")
        );
    }

    #[test]
    fn plugin_run_interactive_and_script_are_mutually_exclusive() {
        let result = Cli::try_parse_from([
            "tau",
            "plugin",
            "run",
            "/bin/x",
            "--interactive",
            "--script",
            "/tmp/x.jsonl",
        ]);
        assert!(result.is_err(), "expected mutual exclusion error");
    }

    #[test]
    fn parses_plugin_protocol_decode_with_filters() {
        let cli = Cli::parse_from([
            "tau",
            "plugin",
            "protocol",
            "decode",
            "/tmp/rec.jsonl",
            "--filter",
            "plugin=echo",
            "--filter",
            "method=llm.complete",
            "--from",
            "1.5",
            "--to",
            "2.5",
            "--json",
        ]);
        let Command::Plugin {
            action:
                PluginAction::Protocol {
                    action: PluginProtocolAction::Decode(args),
                },
        } = cli.command
        else {
            panic!()
        };
        assert_eq!(args.path.to_str(), Some("/tmp/rec.jsonl"));
        assert_eq!(args.filter, vec!["plugin=echo", "method=llm.complete"]);
        assert_eq!(args.from, Some(1.5));
        assert_eq!(args.to, Some(2.5));
        assert!(args.json);
    }
}
