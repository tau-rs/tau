//! `tau build` — see spec `2026-05-27-tau-build-design.md`.
//!
//! Thin CLI shim over [`tau_pkg::bundle::build`]. Resolves the project
//! root from the current directory, calls the bundle builder with the
//! host target + default output path, prints progress to stderr and
//! the bundle path to stdout, then exits with the appropriate code
//! per spec §6 (0 success, 2 config/parse, 3 install-state, 70 internal).

use anyhow::Result;

use tau_pkg::bundle::{build, BuildError, BuildOptions, BundleArtifact, IrPayload};
use tau_ports::target::TargetTriple;

use crate::cli::BuildArgs;
use crate::output::Output;

/// CLI entry point for `tau build`. The function is async to match the
/// dispatcher's signature, but the underlying builder is synchronous.
pub async fn run(args: &BuildArgs, output: &mut Output) -> Result<()> {
    let project_root = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            let _ = output.error(format!("cannot determine current directory: {e}"));
            std::process::exit(70);
        }
    };

    let target = match resolve_target(args) {
        Ok(t) => t,
        Err(msg) => {
            let _ = output.error(msg);
            std::process::exit(2);
        }
    };

    // Map the repeatable `--agent` flag to the builder's filter. Empty
    // → None (build all). Parse each id to AgentId; a malformed id is a
    // config-level input error (exit 2).
    let agent_filter = if args.agents.is_empty() {
        None
    } else {
        let mut parsed = Vec::with_capacity(args.agents.len());
        for raw in &args.agents {
            match raw.parse::<tau_domain::AgentId>() {
                Ok(id) => parsed.push(id),
                Err(e) => {
                    let _ = output.error(format!("invalid agent id '{raw}': {e}"));
                    std::process::exit(2);
                }
            }
        }
        Some(parsed)
    };

    // Lower the project IR. Load the project config (same pipeline the
    // bundle builder uses), then call lower_project with a permissive
    // cache that returns a stub hash for any native tool name (the
    // conformance suite in β.2.6 will wire real caches). On IrError,
    // render a human-readable diagnostic and exit 2.
    let ir_payload = lower_ir(&project_root, &target);

    let opts = BuildOptions {
        project_root,
        target,
        output_path: args.output.clone(),
        agent_filter,
        ir_payload,
    };

    let _ = output.status("Building bundle…");

    match build(opts) {
        Ok(artifact) => {
            emit_artifact(&artifact, output);
            Ok(())
        }
        Err(e) => {
            let _ = output.error(format!("{e}"));
            std::process::exit(exit_code_for(&e) as i32);
        }
    }
}

/// Attempt to lower the project IR, returning `Some(IrPayload)` on
/// success or `None` if lowering fails (non-fatal — the bundle is still
/// built, but without an IR payload; a warning is logged).
///
/// Native-tool content hashes are derived from `sha2::Sha256(symbolic_name)`.
/// This is a deterministic, non-zero stand-in until an actual native-tool
/// registry lands in `tau-pkg`: when that registry exists, replace the
/// `native_tool` closure with the registry's source-content hash. Bundles
/// produced before the switch will rebuild on the next `tau build` because
/// their `canonical_ir_hash` will change — that's the honest forward-stability
/// semantic (D-6): a change in tool identity is a change in workflow identity.
pub(crate) fn lower_ir(project_root: &std::path::Path, target: &TargetTriple) -> Option<IrPayload> {
    use tau_pkg::project::project::UncheckedProjectConfig;

    let tau_toml_path = project_root.join("tau.toml");
    let tau_toml_str = match std::fs::read_to_string(&tau_toml_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("IR lowering: failed to read tau.toml: {e}");
            return None;
        }
    };
    let unchecked: UncheckedProjectConfig = match toml::from_str(&tau_toml_str) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("IR lowering: failed to parse tau.toml: {e}");
            return None;
        }
    };
    let config = match unchecked.validate() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("IR lowering: project config validation failed: {e}");
            return None;
        }
    };

    // Deterministic stand-in cache (β.2.6.1): hash the symbolic name with
    // SHA-256. Two distinct names produce two distinct hashes (so future
    // drift detection actually works), the value is non-zero (typecheck's
    // sentinel check passes), and it's stable across builds (so the IR
    // module hash stays reproducible). When a real native-tool registry
    // lands in tau-pkg, replace this closure with the registry's
    // source-content hash — see this fn's doc-comment.
    let caches = tau_ir::lower::Caches {
        native_tool: &|name: &str| Some(sha256_name(name)),
        mcp_contract: &|_url| None,
        skill: &|_name| None,
    };

    match tau_ir::lower::lower_project(&config, target, &caches) {
        Ok(module) => {
            let bytes = tau_ir::to_canonical_bytes(&module);
            let hash_bytes = tau_ir::compute_hash(&module);
            // Encode both as lowercase hex for TOML-safe storage.
            let canonical_ir_hash = hex_lower(&hash_bytes);
            let canonical_ir_bytes_hex = hex_lower(&bytes);
            Some(IrPayload {
                ir_format: module.ir_format.0.clone(),
                canonical_ir_hash,
                canonical_ir_bytes_hex,
            })
        }
        Err(e) => {
            tracing::warn!("IR lowering failed (bundle built without IR payload): {e}");
            None
        }
    }
}

/// Deterministic content-hash stand-in for a native tool's symbolic name.
///
/// Returns `SHA-256(name.as_bytes())`. Used by [`lower_ir`]'s `Caches::native_tool`
/// closure until a real native-tool registry lands in `tau-pkg`. Distinct
/// names always produce distinct hashes, and the value is non-zero so
/// `tau_ir::lower::typecheck` won't reject it as the unknown-tool sentinel.
fn sha256_name(name: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.finalize().into()
}

/// Encode a byte slice as lowercase hex.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Resolve the build target from CLI args. `None` → host; `Some(s)` →
/// parse + validate it's an Available triple (ADR-0034). Returns a
/// human-readable error string on invalid input.
fn resolve_target(args: &BuildArgs) -> Result<TargetTriple, String> {
    match &args.target {
        None => Ok(TargetTriple::host()),
        Some(s) => {
            let triple: TargetTriple = s
                .parse()
                .map_err(|e| format!("invalid target triple '{s}': {e}"))?;
            let available = tau_ports::target::lookup(&triple)
                .is_some_and(|e| matches!(e.status, tau_ports::target::TripleStatus::Available));
            if !available {
                return Err(format!(
                    "target '{triple}' is not an Available build target; available: {}",
                    available_triples_joined(),
                ));
            }
            Ok(triple)
        }
    }
}

/// Comma-joined Display list of Available registry triples (sorted).
fn available_triples_joined() -> String {
    let mut v: Vec<String> = tau_ports::target::list_available()
        .map(|e| e.triple.to_string())
        .collect();
    v.sort();
    v.join(", ")
}

/// Artifact rendering with JSON support. Emits JSON under --json,
/// human-readable text otherwise.
fn emit_artifact(artifact: &BundleArtifact, output: &mut Output) {
    if output.is_json() {
        let obj = serde_json::json!({
            "path": artifact.path.display().to_string(),
            "sha256": artifact.sha256,
            "size_bytes": artifact.size_bytes,
        });
        let _ = output.json(&obj);
    } else {
        let sha = &artifact.sha256;
        let head = &sha[..sha.len().min(6)];
        let tail = &sha[sha.len().saturating_sub(6)..];
        let _ = output.status(format!(
            "Wrote bundle: {} (sha256: {head}…{tail}, {} bytes)",
            artifact.path.display(),
            artifact.size_bytes,
        ));
        let _ = output.human(&artifact.path.display().to_string());
    }
}

/// Maps a [`BuildError`] to its CLI exit code per spec §6.
fn exit_code_for(err: &BuildError) -> u8 {
    match err {
        BuildError::MissingLockfile
        | BuildError::PackageNotInstalled { .. }
        | BuildError::AgentHomePackageMissing { .. } => 3,
        BuildError::ProjectConfig(_)
        | BuildError::LockfileLoad(_)
        | BuildError::ManifestInvalid(_)
        | BuildError::UnknownAgent { .. }
        | BuildError::AgentHomePackageManifest { .. } => 2,
        BuildError::TreeHashFailed { .. }
        | BuildError::PromptResolveFailed { .. }
        | BuildError::CapabilityOverrideFailed { .. }
        | BuildError::WriteFailed { .. } => 70,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::BuildArgs;
    use tau_ports::target::TargetTriple;

    fn args_with_target(t: Option<&str>) -> BuildArgs {
        BuildArgs {
            target: t.map(|s| s.to_string()),
            output: None,
            agents: vec![],
        }
    }

    #[test]
    fn resolve_target_defaults_to_host() {
        assert_eq!(
            resolve_target(&args_with_target(None)).unwrap(),
            TargetTriple::host()
        );
    }

    #[test]
    fn resolve_target_accepts_available_triple() {
        // Use `passthrough` — Available on every host. Don't use
        // `host()`: on Windows `host()` is `windows-native-strict`,
        // which the registry marks Reserved (scaffold-only), so it
        // would (correctly) be rejected by the Available gate.
        let available = TargetTriple::PASSTHROUGH;
        assert_eq!(
            resolve_target(&args_with_target(Some(&available.to_string()))).unwrap(),
            available,
        );
    }

    #[test]
    fn resolve_target_rejects_unparseable() {
        let err = resolve_target(&args_with_target(Some("not a triple!!!"))).unwrap_err();
        assert!(err.contains("invalid target triple"), "got {err}");
    }

    #[test]
    fn resolve_target_rejects_reserved_or_unknown() {
        // "windows-native-strict" parses the platform-adapter-tier grammar
        // and IS in the registry, but with status Reserved (not Available)
        // — exercises the lookup-Some(Reserved) branch of the Available check.
        let err = resolve_target(&args_with_target(Some("windows-native-strict"))).unwrap_err();
        assert!(err.contains("not an Available"), "got {err}");
    }

    #[test]
    fn exit_code_mapping_per_spec() {
        // Install-state errors → 3.
        assert_eq!(exit_code_for(&BuildError::MissingLockfile), 3);
        assert_eq!(
            exit_code_for(&BuildError::PackageNotInstalled {
                name: "foo".into(),
                path: "/nowhere".into(),
            }),
            3,
        );

        // Config/parse/manifest errors → 2.
        assert_eq!(exit_code_for(&BuildError::ProjectConfig("x".into())), 2);
        assert_eq!(exit_code_for(&BuildError::LockfileLoad("x".into())), 2);
        assert_eq!(exit_code_for(&BuildError::ManifestInvalid("x".into())), 2);

        // Internal / IO errors → 70.
        assert_eq!(
            exit_code_for(&BuildError::WriteFailed {
                path: "/dev/null".into(),
                source: std::io::Error::other("x"),
            }),
            70,
        );
        assert_eq!(
            exit_code_for(&BuildError::PromptResolveFailed {
                id: "a".into(),
                source: std::io::Error::other("x"),
            }),
            70,
        );

        // Unknown agent (bad --agent input) → 2.
        assert_eq!(
            exit_code_for(&BuildError::UnknownAgent {
                id: "ghost".into(),
                available: vec!["alpha".into()],
            }),
            2,
        );

        // Override-agent home package missing -> install-state -> 3.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageMissing {
                id: "r".into(),
                package: "homepkg".into(),
            }),
            3,
        );
        // Home-package manifest unreadable -> config/parse -> 2.
        assert_eq!(
            exit_code_for(&BuildError::AgentHomePackageManifest {
                id: "r".into(),
                package: "homepkg".into(),
                source: tau_pkg::error::ManifestReadError::NotFound { path: "x".into() },
            }),
            2,
        );
    }

    /// `sha256_name` must return the same bytes for the same input — this
    /// is what keeps `canonical_ir_hash` reproducible across `tau build`
    /// invocations of the same source tree.
    #[test]
    fn sha256_name_is_deterministic_per_input() {
        assert_eq!(sha256_name("ReadTemp"), sha256_name("ReadTemp"));
        assert_eq!(sha256_name(""), sha256_name(""));
    }

    /// `sha256_name` must distinguish symbolic names — two distinct tools
    /// must produce two distinct content hashes so any future drift-
    /// detection layer can actually tell them apart.
    #[test]
    fn sha256_name_distinguishes_distinct_names() {
        assert_ne!(sha256_name("A"), sha256_name("B"));
        assert_ne!(sha256_name("ReadTemp"), sha256_name("ReadHumidity"));
    }

    /// `sha256_name` is never the zero sentinel — that's the
    /// `tau_ir::lower::typecheck` "unknown native tool" tripwire and
    /// would re-introduce the silent-IR-loss bug A.2 is fixing.
    #[test]
    fn sha256_name_is_never_zero_sentinel() {
        assert_ne!(sha256_name("ReadTemp"), [0u8; 32]);
        assert_ne!(sha256_name(""), [0u8; 32]);
    }

    /// End-to-end regression for A.2: a project with a `[tools.<x>] native = "…"`
    /// entry must lower to an `IrPayload` instead of falling through to
    /// the `None` warn-and-continue path. Before A.2, the zero-sentinel
    /// cache caused this to return `None`.
    #[test]
    fn lower_ir_yields_payload_for_native_tool_project() {
        let scratch = tempfile::tempdir().unwrap();
        let project = scratch.path();
        // Minimal native-tool project: one agent + one [tools.<x>] native
        // entry. The agent doesn't have to reference the tool — lowering
        // emits every project-level [tools.<x>] entry into the workflow.
        std::fs::write(
            project.join("tau.toml"),
            r#"
[project]
name = "native_smoke"
version = "0.1.0"

[agents.solo]
display_name = "Solo"
package = "native_smoke@^0.1"
llm_backend = "anthropic"

[agents.solo.prompt]
system = "hi"

[tools.read_temp]
native = "ReadTemp"
description = "reads the temperature"
capabilities = []
"#,
        )
        .unwrap();

        let target = TargetTriple::PASSTHROUGH;
        let payload = lower_ir(project, &target);
        assert!(
            payload.is_some(),
            "lower_ir must return Some(IrPayload) for a project with a [tools.<x>] native = ... entry; \
             was None — did the native_tool cache regress to the zero sentinel?",
        );
        let payload = payload.unwrap();
        assert!(!payload.canonical_ir_hash.is_empty());
        assert!(!payload.canonical_ir_bytes_hex.is_empty());
    }
}
