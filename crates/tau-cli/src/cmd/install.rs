//! `tau install` — install a package from a git URL.
//!
//! Per spec §3.13 install row:
//!
//! - Parses the URL into a [`PackageSource`].
//! - Resolves the active [`Scope`] (project or global).
//! - For `--dry-run`: clones the source into a [`tempfile::TempDir`],
//!   parses the manifest, prints a preview of what would be installed,
//!   and writes nothing to the scope.
//! - Otherwise: delegates to [`tau_pkg::install_with_options`] and
//!   prints either a one-line summary or, when `--json` is set, a
//!   structured payload.
//!
//! Errors propagate via `anyhow::Error` and exit 2 through the
//! top-level `dispatch` in `crate::run_main`.

use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

use tau_domain::{
    AgentCapability, Capability, FsCapability, NetCapability, PackageKind, PackageManifest,
    PackageSource, ProcessCapability,
};
use tau_pkg::{install_with_options, read_manifest, InstallOptions, Scope, ScopeKind};
use tau_ports::{CapabilityProbe, CapabilityTier};
use tau_runtime_tokio::process_gate::resolve_adapter;

use crate::cli::InstallArgs;
use crate::cmd::install_sandbox::RuntimeInstallSandbox;
use crate::output::Output;

/// Run `tau install`.
pub async fn run(args: &InstallArgs, output: &mut Output) -> anyhow::Result<()> {
    // 1. Parse URL into a PackageSource.
    let source = PackageSource::from_str(&args.url)
        .map_err(|e| anyhow::anyhow!("invalid package URL {:?}: {}", args.url, e))?;

    // 2. Resolve scope.
    let scope = if args.global {
        Scope::global()?
    } else {
        let cwd = std::env::current_dir()?;
        Scope::resolve(&cwd)?
    };

    // 3. Dry-run path: clone to tmp, parse manifest, print preview.
    if args.dry_run {
        let preview = preview_install(&source, &scope)?;
        emit_dry_run(&preview, output)?;
        return Ok(());
    }

    // 4. Real install. Resolve the host's sandbox adapter and inject it so the
    //    install-time `cargo build` + cross-check run sandboxed (audit S2).
    //    The build path fails closed unless --allow-unsandboxed-build is set.
    output.status(format!("installing from {}...", args.url))?;

    // The strict-tier egress proxy task spawned by `wrap_spawn` runs on this
    // (the ambient) runtime; the gate's guard aborts it on drop. We pass a
    // `Handle` rather than owning a `Runtime` so nothing is dropped in async
    // context (which would panic in tokio's blocking shutdown).
    let handle = tokio::runtime::Handle::current();
    let requirements = tau_pkg::scope::SandboxRequirements::default();
    let adapter = resolve_adapter(&requirements, &[])
        .await
        .map_err(|e| anyhow::anyhow!("resolving install sandbox adapter: {e}"))?;
    let enforced = matches!(
        adapter.probe().await,
        CapabilityProbe::Available { tier, .. } if tier > CapabilityTier::None
    );
    let gate = RuntimeInstallSandbox::new(adapter, handle, enforced);

    let mut options = InstallOptions::default();
    options.sandbox = Some(std::sync::Arc::new(gate));
    options.allow_unsandboxed_build = args.allow_unsandboxed_build;

    let installed = install_with_options(&source, &scope, options)
        .map_err(|e| anyhow::anyhow!("install failed: {e}"))?;

    if output.is_json() {
        let payload = serde_json::json!({
            "name": installed.name.as_str(),
            "version": installed.version.to_string(),
            "scope": scope_kind_str(&scope),
            "path": installed.installed_path,
        });
        output.json(&payload)?;
    } else {
        output.human(&format!(
            "installed: {} {} ({})",
            installed.name,
            installed.version,
            scope_kind_str(&scope),
        ))?;
    }

    Ok(())
}

/// Return `"global"` or `"project"` for the current scope's kind.
fn scope_kind_str(scope: &Scope) -> &'static str {
    match scope.kind() {
        ScopeKind::Global => "global",
        ScopeKind::Project => "project",
        // ScopeKind is `#[non_exhaustive]`; cover unknown future variants.
        _ => "unknown",
    }
}

/// Preview shape used for both human and JSON dry-run output.
struct DryRunPreview {
    name: String,
    version: String,
    url: String,
    target_path: PathBuf,
    kind: String,
    capabilities: Vec<String>,
    dependencies: Vec<String>,
}

/// Clone the source into a temp dir, parse the manifest, and produce the
/// preview that the dry-run path emits. The temp dir auto-removes on drop,
/// so the dry-run leaves no on-disk state behind.
fn preview_install(source: &PackageSource, scope: &Scope) -> anyhow::Result<DryRunPreview> {
    let tmp_dir = tempfile::TempDir::new()?;

    // Render the source's URL/scp string for the clone subprocess. v0.1
    // PackageSource has only the Git variant, but it is `#[non_exhaustive]`,
    // so we surface a clear error for hypothetical future variants.
    let (location_str, rev_opt) = match source {
        PackageSource::Git { location, rev } => (location.to_string(), rev.clone()),
        other => {
            anyhow::bail!("unsupported package source for dry-run: {other:?}");
        }
    };

    // Shell out to `git` directly. We mirror tau-pkg's clone flags
    // (protocol.file.allow=always for file:// fixtures + `--branch <rev>
    // --single-branch` for revision pins) so dry-run accepts the same
    // sources as the real install path.
    let dest = tmp_dir.path();
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always");
    cmd.arg("clone");
    if let Some(rev) = &rev_opt {
        cmd.arg("--branch").arg(rev).arg("--single-branch");
    }
    cmd.arg(&location_str).arg(dest);

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("spawning `git clone`: {e}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git clone failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim(),
        );
    }

    let manifest = read_manifest(&dest.join("tau.toml"))
        .map_err(|e| anyhow::anyhow!("reading cloned manifest: {e}"))?;

    let target_path = scope.package_dir(manifest.name(), manifest.version());

    Ok(DryRunPreview {
        name: manifest.name().as_str().to_owned(),
        version: manifest.version().to_string(),
        url: source.to_string(),
        target_path,
        kind: kind_str(manifest.kind()).to_owned(),
        capabilities: manifest
            .capabilities()
            .iter()
            .map(capability_kind_str)
            .collect(),
        dependencies: dep_strings(&manifest),
    })
}

/// Render a [`PackageKind`] as its canonical kind string (e.g. `"tool"`,
/// `"llm-backend"`). v0.1 ships only `Custom { kind }`, but the type is
/// `#[non_exhaustive]`; future typed variants fall back to `Debug`.
fn kind_str(kind: &PackageKind) -> &str {
    match kind {
        PackageKind::Custom { kind } => kind.as_str(),
        // Forward-compat for typed variants added later.
        _ => "unknown",
    }
}

/// Render a [`Capability`] as its dot-namespaced kind string (matches the
/// manifest `kind = "..."` form in [`Capability`]'s serde).
fn capability_kind_str(cap: &Capability) -> String {
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read".into(),
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write".into(),
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec".into(),
        Capability::Network(NetCapability::Http { .. }) => "net.http".into(),
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn".into(),
        Capability::Agent(AgentCapability::Spawn { .. }) => "agent.spawn".into(),
        Capability::TaskList { .. } => "task_list".into(),
        Capability::Plan { .. } => "plan".into(),
        Capability::Custom { name, .. } => name.clone(),
        // Forward-compat: future typed variants fall back to debug.
        other => format!("{other:?}"),
    }
}

/// Render dependencies as `"<name>@<version_req>"` strings.
fn dep_strings(manifest: &PackageManifest) -> Vec<String> {
    manifest
        .dependencies()
        .iter()
        .map(|dep| format!("{}@{}", dep.name.as_str(), dep.version_req))
        .collect()
}

/// Emit the dry-run preview to the user. Format per task spec:
///
/// ```text
/// [dry-run] would install: <name> <version>
/// [dry-run] from:          <url>
/// [dry-run] to:            <target-path>
/// [dry-run] kind:          <kind>
/// [dry-run] capabilities:  <list> (or "none")
/// [dry-run] dependencies:  <list> (or "none")
/// [dry-run] no changes written.
/// ```
///
/// With `--json`, emits a single JSON object on stdout instead.
fn emit_dry_run(preview: &DryRunPreview, output: &mut Output) -> anyhow::Result<()> {
    if output.is_json() {
        let payload = serde_json::json!({
            "dry_run": true,
            "name": preview.name,
            "version": preview.version,
            "url": preview.url,
            "target_path": preview.target_path,
            "kind": preview.kind,
            "capabilities": preview.capabilities,
            "dependencies": preview.dependencies,
        });
        output.json(&payload)?;
        return Ok(());
    }

    output.dry_run(format!(
        "would install: {} {}",
        preview.name, preview.version
    ))?;
    output.dry_run(format!("from:          {}", preview.url))?;
    output.dry_run(format!("to:            {}", preview.target_path.display()))?;
    output.dry_run(format!("kind:          {}", preview.kind))?;
    let caps = if preview.capabilities.is_empty() {
        "none".to_string()
    } else {
        preview.capabilities.join(", ")
    };
    output.dry_run(format!("capabilities:  {caps}"))?;
    let deps = if preview.dependencies.is_empty() {
        "none".to_string()
    } else {
        preview.dependencies.join(", ")
    };
    output.dry_run(format!("dependencies:  {deps}"))?;
    output.dry_run("no changes written.")?;
    Ok(())
}
