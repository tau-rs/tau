//! `tau build` — see spec `2026-05-27-tau-build-design.md`.
//!
//! Thin CLI shim over [`tau_pkg::bundle::build`]. Resolves the project
//! root from the current directory, calls the bundle builder with the
//! host target + default output path, prints progress to stderr and
//! the bundle path to stdout, then exits with the appropriate code
//! per spec §6 (0 success, 2 config/parse, 3 install-state, 70 internal).

use anyhow::Result;

use tau_pkg::bundle::{build, BuildError, BuildOptions, BundleArtifact};
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

    let opts = BuildOptions {
        project_root,
        target,
        output_path: args.output.clone(),
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
        BuildError::MissingLockfile | BuildError::PackageNotInstalled { .. } => 3,
        BuildError::ProjectConfig(_)
        | BuildError::LockfileLoad(_)
        | BuildError::ManifestInvalid(_) => 2,
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
    }
}
