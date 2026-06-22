//! Workspace task runner. Currently exposes:
//!
//! - `cargo xtask build-base-image` — builds `tau-plugin-base:dev`.
//! - `cargo xtask build-plugin-images [--name <bin>]` — builds the base if
//!   missing, then builds each plugin's image (or just the named one).
//!
//! Auto-detects the container runtime (podman first, docker fallback) by
//! probing `<runtime> --version` with a 2-second timeout. Mirrors the
//! convention from `tau-sandbox-container::probe`.
//!
//! Honors `BUILDX_CACHE_FROM` / `BUILDX_CACHE_TO` env vars: when set, each
//! `<runtime> build` invocation gets `--cache-from=<value>` /
//! `--cache-to=<value>` flags appended. CI sets these to
//! `type=gha` / `type=gha,mode=max` to use buildx's GitHub Actions cache.

use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "Workspace task runner.")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build the shared `tau-plugin-base:dev` image.
    BuildBaseImage,
    /// Build per-plugin images (all, or just `--name <bin>`).
    BuildPluginImages {
        /// Cargo `[[bin]]` name (e.g. `shell-plugin`). Builds all if omitted.
        #[arg(long)]
        name: Option<String>,
    },
}

/// All plugins shipped in-tree that get a Dockerfile in Phase 1.
const PLUGINS: &[&str] = &[
    "shell-plugin",
    "fs-read-plugin",
    "anthropic-plugin",
    "ollama-plugin",
    "openai-plugin",
];

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = detect_runtime()?;
    eprintln!("xtask: using container runtime `{runtime}`");

    match cli.command {
        Cmd::BuildBaseImage => build_base(&runtime),
        Cmd::BuildPluginImages { name } => match name {
            Some(n) => {
                ensure_base(&runtime)?;
                build_plugin(&runtime, &n)
            }
            None => {
                ensure_base(&runtime)?;
                for p in PLUGINS {
                    build_plugin(&runtime, p)?;
                }
                Ok(())
            }
        },
    }
}

/// Probe `podman` first, then `docker`. Mirrors PR #40's
/// `ContainerRuntime::Auto` ordering.
fn detect_runtime() -> Result<String> {
    // Honor explicit override first. CI sets TAU_CONTAINER_RUNTIME=docker
    // because GHA Linux runners ship both podman and docker, and the
    // BUILDX_CACHE_FROM/TO `type=gha` directive is buildx-specific (podman
    // rejects it). Local dev leaves the env unset → probe order applies.
    if let Ok(forced) = std::env::var("TAU_CONTAINER_RUNTIME") {
        let forced = forced.trim();
        if !forced.is_empty() {
            if probe_one(forced) {
                return Ok(forced.to_string());
            }
            bail!("TAU_CONTAINER_RUNTIME=`{forced}` is not on PATH or unresponsive");
        }
    }
    for bin in ["podman", "docker"] {
        if probe_one(bin) {
            return Ok(bin.to_string());
        }
    }
    bail!("no container runtime found on PATH (tried podman, docker)")
}

fn probe_one(bin: &str) -> bool {
    let res = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match res {
        Ok(c) => c,
        Err(_) => return false,
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(2) {
                    let _ = child.kill();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

fn ensure_base(runtime: &str) -> Result<()> {
    // Always rebuild the base; the buildx cache makes warm rebuilds cheap.
    build_base(runtime)
}

/// Build args helper. Honors `BUILDX_CACHE_FROM` / `BUILDX_CACHE_TO`
/// env vars (set by CI). If unset, builds with default (no GHA cache).
fn build_args(dockerfile: &str, tag: &str) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if let Ok(v) = std::env::var("BUILDX_CACHE_FROM") {
        if !v.is_empty() {
            args.push(format!("--cache-from={v}"));
        }
    }
    if let Ok(v) = std::env::var("BUILDX_CACHE_TO") {
        if !v.is_empty() {
            args.push(format!("--cache-to={v}"));
        }
    }
    args.push("-f".to_string());
    args.push(dockerfile.to_string());
    args.push("-t".to_string());
    args.push(tag.to_string());
    args.push(".".to_string());
    args
}

fn build_base(runtime: &str) -> Result<()> {
    eprintln!("xtask: building tau-plugin-base:dev ...");
    let args = build_args("crates/tau-plugin-base/Dockerfile", "tau-plugin-base:dev");
    let status = Command::new(runtime)
        .args(&args)
        .status()
        .with_context(|| format!("invoke {runtime} build (base image)"))?;
    if !status.success() {
        bail!("`{runtime} build` for tau-plugin-base failed (exit {status})");
    }
    Ok(())
}

fn build_plugin(runtime: &str, bin_name: &str) -> Result<()> {
    let crate_subdir = bin_name.strip_suffix("-plugin").unwrap_or(bin_name);
    let dockerfile = format!("crates/tau-plugins/{crate_subdir}/Dockerfile");
    let tag = format!("tau-plugin-{bin_name}:dev");
    eprintln!("xtask: building {tag} ...");
    let args = build_args(&dockerfile, &tag);
    let status = Command::new(runtime)
        .args(&args)
        .status()
        .with_context(|| format!("invoke {runtime} build for {bin_name}"))?;
    if !status.success() {
        bail!("`{runtime} build` for {bin_name} failed (exit {status})");
    }
    Ok(())
}
