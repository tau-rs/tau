//! `tau plugin describe <name>` — print a plugin's handshake metadata.
//!
//! Per spec §9 (debug tier): resolves the package by name in the
//! current scope's lockfile, spawns the plugin binary, drives one
//! `meta.handshake`, prints the advertised name / version / port /
//! method list / schemas / source, then sends `meta.shutdown` for a
//! clean exit. The plugin process does not stay alive after this
//! command returns.

use std::path::PathBuf;

use anyhow::Context;
use tau_pkg::{LockFile, Scope};
use tau_plugin_protocol::handshake::TraceContext;
use tau_runtime::plugin_host::{self, PluginHostOptions, RecordingSink};

use crate::cli::PluginDescribeArgs;
use crate::output::Output;

/// Run `tau plugin describe`.
pub async fn run(
    args: &PluginDescribeArgs,
    record_protocol: Option<PathBuf>,
    output: &mut Output,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let scope = Scope::resolve(&cwd).context("resolving package scope")?;

    let lockfile = LockFile::load(&scope.lockfile_path())
        .with_context(|| format!("loading lockfile {}", scope.lockfile_path().display()))?;

    let pkg_name: tau_domain::PackageName = args.name.parse().with_context(|| {
        format!(
            "invalid package name {:?} (must be lowercase ASCII kebab-case)",
            args.name
        )
    })?;

    let pkg = lockfile.find(&pkg_name).ok_or_else(|| {
        anyhow::anyhow!(
            "package {:?} not installed in scope (run `tau install <url>`)",
            args.name
        )
    })?;

    let plugin = pkg.plugin.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "package {:?} has no [plugin] table in its tau.toml \
             (it is a data-only package; nothing to describe)",
            args.name
        )
    })?;

    // Build host options. `--record-protocol` is honored here so a
    // describe invocation is itself capturable in the same JSONL
    // format an operator inspects via `tau plugin protocol decode`.
    let mut host_options = PluginHostOptions::default();
    if let Some(path) = record_protocol {
        host_options.recording = Some(RecordingSink::JsonlFile { path });
    }

    let trace_context = TraceContext::new(
        format!(
            "tau-describe-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ),
        "describe".to_string(),
        "root".to_string(),
    );

    let response = plugin_host::describe_plugin(plugin, trace_context, host_options)
        .await
        .with_context(|| format!("describing plugin {:?}", args.name))?;

    // Flush the process-global `PluginRecordingLayer` (if any) so the
    // describe invocation's recorded frames reach disk before the
    // CLI exits.
    crate::cmd::plugin_loader::flush_recorders().await;

    if output.is_json() {
        let payload = serde_json::json!({
            "package": args.name,
            "package_version": pkg.active_version.to_string(),
            "source": pkg.source.to_string(),
            "binary_path": plugin.binary_path,
            "manifest": {
                "provides": format!("{:?}", plugin.manifest.provides),
                "kind": format!("{:?}", plugin.manifest.kind),
                "bin": plugin.manifest.bin,
            },
            "handshake": {
                "protocol_version": response.protocol_version,
                "provides": format!("{:?}", response.provides),
                "plugin_name": response.plugin_name,
                "plugin_version": response.plugin_version,
                "methods": response.methods,
                "schemas": response.schemas,
            },
        });
        output.json(&payload)?;
    } else {
        output.human(&format!("plugin:           {}", args.name))?;
        output.human(&format!("package version:  {}", pkg.active_version))?;
        output.human(&format!("source:           {}", pkg.source))?;
        output.human(&format!(
            "binary:           {}",
            plugin.binary_path.display()
        ))?;
        output.human(&format!("provides:         {:?}", response.provides))?;
        output.human(&format!("protocol version: {}", response.protocol_version))?;
        output.human(&format!(
            "plugin name:      {} {}",
            response.plugin_name, response.plugin_version
        ))?;
        if response.methods.is_empty() {
            output.human("methods:          (none beyond meta.*)")?;
        } else {
            output.human("methods:")?;
            for method in &response.methods {
                output.human(&format!("  - {method}"))?;
            }
        }
        if response.schemas.is_empty() {
            output.human("schemas:          (none advertised)")?;
        } else {
            output.human("schemas:")?;
            for (method, schema) in &response.schemas {
                output.human(&format!("  {method}:"))?;
                let params = serde_json::to_string_pretty(&schema.params)
                    .unwrap_or_else(|_| "<unrenderable>".into());
                let result = serde_json::to_string_pretty(&schema.result)
                    .unwrap_or_else(|_| "<unrenderable>".into());
                for line in params.lines() {
                    output.human(&format!("    params: {line}"))?;
                }
                for line in result.lines() {
                    output.human(&format!("    result: {line}"))?;
                }
            }
        }
    }

    Ok(())
}
