//! `tau skill show <name>` — inspect one installed skill.
//!
//! Reads the lockfile + the package's tau.toml (one disk seek for
//! the requested skill). With `--body`, also reads SKILL.md and
//! either renders via termimad (default) or dumps raw bytes (--raw).

use std::path::PathBuf;

use anyhow::Context;
use serde::Serialize;

use crate::cli::SkillShowArgs;
use crate::cmd::skill::{levenshtein, render};
use crate::output::Output;

#[derive(Debug, Serialize)]
struct CapabilityJson {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hosts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    methods: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commands: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_kinds: Option<Vec<String>>,
}

fn cap_to_json(c: &tau_domain::Capability) -> CapabilityJson {
    use tau_domain::Capability as C;
    use tau_domain::FsCapability as Fs;
    use tau_domain::NetCapability as Net;
    use tau_domain::ProcessCapability as Proc;
    let mut out = CapabilityJson {
        kind: String::new(),
        paths: None,
        mode: None,
        hosts: None,
        methods: None,
        commands: None,
        allowed_kinds: None,
    };
    match c {
        C::Filesystem(Fs::Read { paths, .. }) => {
            out.kind = "fs.read".into();
            out.paths = Some(paths.clone());
        }
        C::Filesystem(Fs::Write { paths, .. }) => {
            out.kind = "fs.write".into();
            out.paths = Some(paths.clone());
        }
        C::Filesystem(Fs::Exec { paths, .. }) => {
            out.kind = "fs.exec".into();
            out.paths = Some(paths.clone());
        }
        C::Network(Net::Http { hosts, methods, .. }) => {
            out.kind = "net.http".into();
            out.hosts = Some(match hosts {
                tau_domain::NetHosts::Any => vec!["any".to_string()],
                tau_domain::NetHosts::List(h) => h.clone(),
            });
            out.methods = Some(methods.clone());
        }
        C::Process(Proc::Spawn { commands, .. }) => {
            out.kind = "process.spawn".into();
            out.commands = Some(commands.clone());
        }
        C::Agent(tau_domain::AgentCapability::Spawn { allowed_kinds, .. }) => {
            out.kind = "agent.spawn".into();
            out.allowed_kinds = Some(allowed_kinds.clone());
        }
        C::TaskList { mode } => {
            out.kind = "task_list".into();
            out.mode = Some(mode.clone());
        }
        C::Plan { mode } => {
            out.kind = "plan".into();
            out.mode = Some(mode.clone());
        }
        C::Custom { name, .. } => {
            out.kind = name.clone();
        }
        _ => {
            out.kind = "unknown".into();
        }
    }
    out
}

#[derive(Debug, Serialize)]
struct PackageDepJson {
    name: String,
    version_req: String,
}

#[derive(Debug, Serialize)]
struct SkillShowJson {
    name: String,
    version: String,
    description: String,
    source: String,
    install_path: String,
    capabilities: Vec<CapabilityJson>,
    requires_tools: Vec<PackageDepJson>,
    requires_skills: Vec<PackageDepJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    synthesized_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

/// Run `tau skill show`.
pub async fn run(args: &SkillShowArgs, _output: &mut Output) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    let scope = tau_pkg::Scope::resolve(&cwd).context("resolving scope")?;
    let lockfile_path = scope.lockfile_path();
    let lockfile = if lockfile_path.exists() {
        tau_pkg::lockfile::LockFile::load(&lockfile_path).context("loading lockfile")?
    } else {
        tau_pkg::lockfile::LockFile::default()
    };

    let installed_skills: Vec<String> = lockfile
        .packages
        .iter()
        .filter(|p| p.skill.is_some())
        .map(|p| p.name.as_str().to_string())
        .collect();

    let pkg = match lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == args.name && p.skill.is_some())
    {
        Some(p) => p,
        None => {
            return emit_unknown_name(&args.name, args.json, &installed_skills);
        }
    };

    let locked_skill = pkg.skill.as_ref().expect("filtered to Some(skill) above");

    // install_path: <scope>/.tau/packages/<name>/<version>/
    let install_path: PathBuf = scope
        .path()
        .join("packages")
        .join(pkg.name.as_str())
        .join(pkg.active_version.to_string());

    // Read tau.toml.
    let toml_path = install_path.join("tau.toml");
    if !toml_path.exists() {
        anyhow::bail!(
            "skill {:?} lockfile entry points at {:?} but no tau.toml found there.\n  \
             the skill may have been manually removed — re-run `tau install` to restore",
            args.name,
            install_path
        );
    }
    let toml_text =
        std::fs::read_to_string(&toml_path).with_context(|| format!("reading {toml_path:?}"))?;
    let unchecked: tau_domain::UncheckedManifest =
        toml::from_str(&toml_text).with_context(|| format!("parsing {toml_path:?}"))?;
    let manifest = unchecked
        .validate()
        .with_context(|| format!("validating {toml_path:?}"))?;

    // Optional --body.
    let body_raw: Option<String> = if args.body {
        let skill_block = manifest.skill().expect("skill kind verified via lockfile");
        let skill_md_path = install_path.join(&skill_block.content);
        let text = std::fs::read_to_string(&skill_md_path)
            .with_context(|| format!("reading {skill_md_path:?}"))?;
        let parsed = tau_domain::parse_skill_md(&text)
            .with_context(|| format!("parsing {skill_md_path:?}"))?;
        Some(parsed.body)
    } else {
        None
    };

    if args.json {
        let caps: Vec<CapabilityJson> = manifest.capabilities().iter().map(cap_to_json).collect();
        let skill_block = manifest.skill().expect("skill kind verified");
        let synthesized_from_str = pkg.synthesized_from.as_ref().map(|syn| match syn {
            tau_pkg::SynthesizedSource::Anthropic => "anthropic".to_string(),
            _ => "unknown".to_string(),
        });
        let json_obj = SkillShowJson {
            name: pkg.name.as_str().to_string(),
            version: pkg.active_version.to_string(),
            description: locked_skill.frontmatter.description.clone(),
            source: pkg.source.to_string(),
            install_path: install_path.display().to_string(),
            capabilities: caps,
            requires_tools: skill_block
                .requires_tools
                .iter()
                .map(|d| PackageDepJson {
                    name: d.name.as_str().to_string(),
                    version_req: d.version_req.to_string(),
                })
                .collect(),
            requires_skills: skill_block
                .requires_skills
                .iter()
                .map(|d| PackageDepJson {
                    name: d.name.as_str().to_string(),
                    version_req: d.version_req.to_string(),
                })
                .collect(),
            synthesized_from: synthesized_from_str,
            body: body_raw,
        };
        let json = serde_json::to_string_pretty(&json_obj).context("serializing show to JSON")?;
        println!("{json}");
        return Ok(());
    }

    // Human output.
    println!("{} {}", pkg.name.as_str(), pkg.active_version);
    println!("─────────────────────────────────────────────────");
    println!("description    {}", locked_skill.frontmatter.description);
    println!("source         {}", pkg.source);
    if let Some(syn) = &pkg.synthesized_from {
        println!(
            "source format  synthesized ({})",
            match syn {
                tau_pkg::SynthesizedSource::Anthropic => "Anthropic Agent Skills",
                _ => "unknown",
            }
        );
    }
    println!("install path   {}", install_path.display());

    if !manifest.capabilities().is_empty() {
        println!();
        println!("capabilities");
        for c in manifest.capabilities() {
            let cj = cap_to_json(c);
            let detail = if let Some(paths) = &cj.paths {
                paths.join(", ")
            } else if let Some(mode) = &cj.mode {
                mode.clone()
            } else if let Some(hosts) = &cj.hosts {
                hosts.join(", ")
            } else if let Some(cmds) = &cj.commands {
                cmds.join(", ")
            } else if let Some(ak) = &cj.allowed_kinds {
                ak.join(", ")
            } else {
                String::new()
            };
            println!("  {:<10} {}", cj.kind, detail);
        }
    }

    let skill_block = manifest.skill().expect("skill kind verified");
    if !skill_block.requires_tools.is_empty() {
        println!();
        println!("requires tools");
        for d in &skill_block.requires_tools {
            println!("  {:<12} {}", d.name.as_str(), d.version_req);
        }
    }
    if !skill_block.requires_skills.is_empty() {
        println!();
        println!("requires skills");
        for d in &skill_block.requires_skills {
            println!("  {:<12} {}", d.name.as_str(), d.version_req);
        }
    }

    if let Some(body_text) = body_raw {
        println!();
        if args.raw {
            println!("body (raw)");
            println!("─────────────────────────────────────────────────");
            println!();
            print!("{body_text}");
        } else {
            println!("body");
            println!("─────────────────────────────────────────────────");
            println!();
            print!("{}", render::render_markdown(&body_text));
        }
    }

    Ok(())
}

/// Emit the unknown-name error + suggestions, then bail with exit 2.
fn emit_unknown_name(name: &str, json: bool, installed: &[String]) -> anyhow::Result<()> {
    let suggestion = levenshtein::closest_match(name, installed, 2);
    if json {
        let body = if let Some(s) = suggestion {
            serde_json::json!({
                "error": format!("skill not found: {name}"),
                "suggestion": s,
                "installed": installed,
            })
        } else {
            serde_json::json!({
                "error": format!("skill not found: {name}"),
                "installed": installed,
            })
        };
        eprintln!("{}", serde_json::to_string_pretty(&body).unwrap());
    } else {
        eprintln!("error: skill not found: {name}");
        if let Some(s) = suggestion {
            eprintln!("  did you mean: {s}?");
        }
        if !installed.is_empty() {
            eprintln!();
            eprintln!("  installed skills:");
            for s in installed {
                eprintln!("    {s}");
            }
        }
    }
    anyhow::bail!("skill not found")
}
