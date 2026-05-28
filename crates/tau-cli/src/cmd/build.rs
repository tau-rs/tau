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

    let target = TargetTriple::host(); // Task 2 → resolve_target(args)

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

/// Human-mode artifact rendering. (JSON branch added in Task 3.)
fn emit_artifact(artifact: &BundleArtifact, output: &mut Output) {
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
