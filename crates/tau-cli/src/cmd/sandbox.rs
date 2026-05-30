//! `tau sandbox` subcommand group — diagnostic + scaffolding.

use crate::cli::{SandboxArgs, SandboxCommand, SandboxSetupArgs};
use crate::output::Output;

/// Dispatch `tau sandbox <subcommand>`.
pub async fn run(args: &SandboxArgs, output: &mut Output) -> anyhow::Result<()> {
    match &args.command {
        SandboxCommand::Status => run_status(output).await,
        SandboxCommand::Setup(setup_args) => run_setup(setup_args, output).await,
    }
}

async fn run_status(output: &mut Output) -> anyhow::Result<()> {
    use tau_pkg::scope::{SandboxRequirements, ScopeConfig};
    use tau_runtime::sandbox::registry::{detect_platform, REGISTRY};

    let platform = detect_platform();

    // Platform line
    output.human(&format!("platform: {platform}"))?;
    output.human("")?;

    // Per-adapter probe table
    output.human("adapters detected:")?;
    for entry in REGISTRY.iter() {
        let name = entry.kind.name();
        // Pad to 14 chars so columns align
        let pad = if name.len() < 14 {
            " ".repeat(14 - name.len())
        } else {
            " ".to_string()
        };

        let status = if !entry.platforms.includes(platform) {
            "not applicable on this platform".to_string()
        } else {
            match tau_runtime::sandbox::instantiate_for_probe(entry.kind) {
                Ok(adapter) => match adapter.probe().await {
                    tau_ports::CapabilityProbe::Available { tier, details } => {
                        if details.is_empty() {
                            format!("available, tier={tier:?}")
                        } else {
                            format!("available, tier={tier:?}; {details}")
                        }
                    }
                    tau_ports::CapabilityProbe::Unavailable { reason } => {
                        format!("unavailable: {reason}")
                    }
                    other => format!("probe returned: {other:?}"),
                },
                Err(msg) => format!("unavailable: {msg}"),
            }
        };
        output.human(&format!("  {name}:{pad}{status}"))?;
    }
    output.human("")?;

    // Project requirements — read scope config; render errors inline (never bail).
    output.human("project requirements:")?;

    let scope_and_config: Result<(tau_pkg::scope::Scope, ScopeConfig), String> =
        std::env::current_dir()
            .map_err(|e| format!("could not read cwd: {e}"))
            .and_then(|cwd| {
                tau_pkg::scope::Scope::resolve(&cwd).map_err(|e| format!("scope resolve: {e}"))
            })
            .and_then(|scope| {
                let config_path = scope.config_path();
                if config_path.exists() {
                    std::fs::read_to_string(&config_path)
                        .map_err(|e| format!("reading scope config at {config_path:?}: {e}"))
                        .and_then(|text| {
                            ScopeConfig::read_from_str(&text).map_err(|e| {
                                format!("parsing scope config at {config_path:?}: {e}")
                            })
                        })
                        .map(|cfg| (scope, cfg))
                } else {
                    // No config.toml — use defaults.
                    let kind = scope.kind();
                    Ok((scope, ScopeConfig::new(kind)))
                }
            });

    match scope_and_config {
        Ok((_scope, cfg)) => {
            output.human(&format!("  required_tier: {:?}", cfg.sandbox.required_tier))?;
            if cfg.sandbox.required_shapes.is_empty() {
                output.human("  required_shapes: (auto-derived from plugins)")?;
            } else {
                output.human(&format!(
                    "  required_shapes: {:?}",
                    cfg.sandbox.required_shapes
                ))?;
            }
            output.human("")?;

            // Resolution outcome — no plugin-specific requirements for status.
            output.human("resolution:")?;
            let plugin_reqs: Vec<tau_domain::PluginSandboxRequirements> = vec![];
            let resolution =
                tau_runtime::sandbox::resolve_adapter(&cfg.sandbox, &plugin_reqs).await;
            match resolution {
                Ok(adapter) => {
                    output.human(&format!("  selected adapter: {}", adapter.name()))?;
                }
                Err(e) => {
                    output.human(&format!("  error: {e}"))?;
                }
            }
        }
        Err(e) => {
            output.human(&format!("  (could not read config: {e})"))?;
            // Use defaults for resolution section.
            output.human("")?;
            output.human("resolution:")?;
            let plugin_reqs: Vec<tau_domain::PluginSandboxRequirements> = vec![];
            let resolution = tau_runtime::sandbox::resolve_adapter(
                &SandboxRequirements::default(),
                &plugin_reqs,
            )
            .await;
            match resolution {
                Ok(adapter) => {
                    output.human(&format!("  selected adapter: {}", adapter.name()))?;
                }
                Err(e) => {
                    output.human(&format!("  error: {e}"))?;
                }
            }
        }
    }

    Ok(())
}

async fn run_setup(args: &SandboxSetupArgs, output: &mut Output) -> anyhow::Result<()> {
    use tau_pkg::scope::{Scope, ScopeConfig};

    // 1. Resolve the scope (must succeed before we can write to it).
    let cwd = std::env::current_dir()?;
    let scope =
        Scope::resolve(&cwd).map_err(|e| anyhow::anyhow!("could not resolve scope: {e}"))?;

    // 2. Determine the desired tier.
    let tier = if let Some(tier_arg) = args.tier {
        // --tier was provided; use it directly.
        cli_tier_arg_to_required(tier_arg)
    } else if args.non_interactive {
        // --non-interactive without --tier is an error.
        anyhow::bail!("--non-interactive requires --tier to be set");
    } else {
        // Interactive mode: probe adapters, prompt the user.
        run_interactive_prompt(&scope, output).await?
    };

    // 3. Read the existing config; mutate the sandbox block; write back.
    let config_path = scope.config_path();
    let mut cfg = if config_path.exists() {
        let text = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("could not read scope config: {e}"))?;
        ScopeConfig::read_from_str(&text)
            .map_err(|e| anyhow::anyhow!("could not parse scope config: {e}"))?
    } else {
        ScopeConfig::new(scope.kind())
    };
    cfg.sandbox.required_tier = tier;

    // 4. Atomic write.
    let toml = cfg
        .to_toml_string()
        .map_err(|e| anyhow::anyhow!("could not serialize config: {e}"))?;
    write_scope_config_atomic(&scope, &toml)?;

    // 5. Confirm to user.
    let msg = format!(
        "wrote <scope>/config.toml with [sandbox] required_tier = {:?}\n",
        tier
    );
    output.human(&msg)?;
    Ok(())
}

fn cli_tier_arg_to_required(
    arg: crate::cli::SandboxRequiredTierArg,
) -> tau_pkg::scope::SandboxRequiredTier {
    use crate::cli::SandboxRequiredTierArg;
    use tau_pkg::scope::SandboxRequiredTier;
    match arg {
        SandboxRequiredTierArg::None => SandboxRequiredTier::None,
        SandboxRequiredTierArg::Light => SandboxRequiredTier::Light,
        SandboxRequiredTierArg::Strict => SandboxRequiredTier::Strict,
    }
}

async fn run_interactive_prompt(
    _scope: &tau_pkg::scope::Scope,
    output: &mut Output,
) -> anyhow::Result<tau_pkg::scope::SandboxRequiredTier> {
    use std::io::{BufRead, Write};
    use tau_pkg::scope::SandboxRequiredTier;
    use tau_runtime::sandbox::registry::{detect_platform, REGISTRY};

    let platform = detect_platform();
    let mut intro = String::new();
    intro.push_str(&format!("detecting platform... {platform}\n"));
    intro.push_str("probing adapters...\n");
    for entry in REGISTRY.iter() {
        let name = entry.kind.name();
        let mark = if !entry.platforms.includes(platform) {
            "✗ (not applicable)"
        } else {
            match tau_runtime::sandbox::instantiate_for_probe(entry.kind) {
                Ok(adapter) => match adapter.probe().await {
                    tau_ports::CapabilityProbe::Available { .. } => "✓ available",
                    _ => "✗ unavailable",
                },
                Err(_) => "✗ unavailable",
            }
        };
        intro.push_str(&format!("  {mark:18} {name}\n"));
    }
    output.human(&intro)?;

    eprintln!("\nselect required tier for this project:");
    eprintln!("  [1] strict (recommended for production)");
    eprintln!("  [2] light (filesystem isolation only)");
    eprintln!("  [3] none (no enforcement; for development)");
    eprint!("> ");
    std::io::stderr().flush().ok();

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| anyhow::anyhow!("read stdin: {e}"))?;
    let trimmed = line.trim();
    match trimmed {
        "1" | "strict" => Ok(SandboxRequiredTier::Strict),
        "2" | "light" => Ok(SandboxRequiredTier::Light),
        "3" | "none" => Ok(SandboxRequiredTier::None),
        other => anyhow::bail!("unknown selection: {other:?}; expected 1, 2, or 3"),
    }
}

fn write_scope_config_atomic(scope: &tau_pkg::scope::Scope, content: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let config_path = scope.config_path();
    // Atomic write via temp file + rename.
    let dir = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| anyhow::anyhow!("create temp file: {e}"))?;
    tmp.write_all(content.as_bytes())
        .map_err(|e| anyhow::anyhow!("write temp file: {e}"))?;
    tmp.persist(&config_path)
        .map_err(|e| anyhow::anyhow!("persist temp file as config.toml: {e}"))?;
    Ok(())
}
