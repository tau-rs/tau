//! Subprocess wrapper around the system `git` binary.
//!
//! The wrapper is intentionally thin — it spawns `git` via
//! [`std::process::Command`], captures stdout/stderr, inspects exit
//! codes, and produces typed [`GitError`] values. Authentication is
//! inherited from the user's git configuration (SSH keys, credential
//! helpers, etc.) per NG9: tau does not manage credentials.
//!
//! Visibility: `pub(crate)`. The install/uninstall lifecycles in
//! [`crate::install`] (Task 10) use this internally; external callers
//! never see [`Git`].
//!
//! Revision-pinning limitation: `Git::clone` translates a non-`None`
//! `PackageSource::Git { rev }` to `--branch <rev> --single-branch`,
//! which works for branch names and tag names but NOT arbitrary
//! commit SHAs. v0.1 accepts this limitation; if a user pins a SHA,
//! the install lifecycle (Task 10) can either error out or do a
//! second `git checkout <sha>` step.

use std::path::Path;
use std::process::Command;

use tau_domain::PackageSource;

use crate::error::GitError;

/// Zero-sized handle for namespacing the git-binary subprocess wrapper.
///
/// All methods are associated functions (no instance state).
pub(crate) struct Git;

impl Git {
    /// Verify the `git` binary is on `PATH` and return its version string.
    ///
    /// Returns `Ok(stdout-trimmed)` on success (typically
    /// `"git version 2.43.0"` or similar). Returns
    /// [`GitError::GitMissing`] if the binary is not found,
    /// [`GitError::Io`] for other I/O errors,
    /// [`GitError::CommandFailed`] if `git --version` exits non-zero
    /// (vanishingly rare).
    pub(crate) fn version_check() -> Result<String, GitError> {
        let output = Command::new("git").arg("--version").output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitMissing
            } else {
                GitError::Io {
                    message: format!("running `git --version`: {e}"),
                }
            }
        })?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                what: "git --version".into(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Clone `source` into `dest`.
    ///
    /// If `source.rev` is `None`, runs `git clone <url> <dest>`. If
    /// `Some(rev)`, runs `git clone --branch <rev> --single-branch <url> <dest>`
    /// — `--branch` accepts branch names and tag names but NOT
    /// arbitrary commit SHAs. A user passing a 40-char SHA as `rev`
    /// will get [`GitError::CloneFailed`] with git's
    /// "Remote branch ... not found" stderr. v0.1 documents this as
    /// a known limitation; Phase-1+ may add SHA-pinning support via
    /// a post-clone `git checkout` step.
    ///
    /// For non-`PackageSource::Git` variants (none exist at v0.1, but
    /// the enum is `#[non_exhaustive]`), returns
    /// [`GitError::CommandFailed`] without spawning a process —
    /// callers must not pass non-`Git` sources.
    pub(crate) fn clone(source: &PackageSource, dest: &Path) -> Result<(), GitError> {
        let (url_string, rev_opt) = match source {
            PackageSource::Git { location, rev } => (location.to_string(), rev.clone()),
            _ => {
                return Err(GitError::CommandFailed {
                    what: "git clone (precondition: source must be PackageSource::Git)".into(),
                    stderr: format!("unsupported source variant: {source:?}"),
                });
            }
        };

        let mut cmd = Command::new("git");
        // Allow file:// protocol even when the system or CI git config sets
        // protocol.file.allow = user (the post-git-2.38 default) or never.
        // tau intentionally supports file://-based local sources (e.g. for
        // test fixtures and air-gapped installs).
        cmd.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
            .env("GIT_CONFIG_VALUE_0", "always");
        cmd.arg("clone");
        if let Some(rev) = &rev_opt {
            cmd.arg("--branch").arg(rev).arg("--single-branch");
        }
        cmd.arg(&url_string).arg(dest);

        // Note: blocks until git exits; no timeout at v0.1 (sync API).
        let output = cmd.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::GitMissing
            } else {
                GitError::Io {
                    message: format!("spawning `git clone`: {e}"),
                }
            }
        })?;

        if !output.status.success() {
            return Err(GitError::CloneFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        Ok(())
    }

    /// Resolve the HEAD of the cloned repo at `repo` to a 40-char SHA.
    ///
    /// Runs `git -C <repo> rev-parse HEAD`. Used to populate
    /// [`crate::lockfile::LockedVersion::resolved_commit`] regardless
    /// of whether the user-supplied `rev` was a branch, tag, or SHA.
    pub(crate) fn resolve_head(repo: &Path) -> Result<String, GitError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitMissing
                } else {
                    GitError::Io {
                        message: format!("running `git rev-parse HEAD` in {}: {e}", repo.display()),
                    }
                }
            })?;

        if !output.status.success() {
            return Err(GitError::CommandFailed {
                what: "git rev-parse HEAD".into(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_check_returns_a_version_string_on_dev_machines() {
        // CI runners and dev machines have git installed. This test is
        // a smoke test only; if `git` is unavailable in some unusual
        // env, we tolerate GitMissing without failing the suite.
        match Git::version_check() {
            Ok(s) => assert!(s.starts_with("git version "), "got: {s:?}"),
            Err(GitError::GitMissing) => {
                eprintln!("warning: `git` not on PATH, skipping git smoke test");
            }
            Err(other) => panic!("unexpected git error: {other:?}"),
        }
    }

    #[test]
    fn resolve_head_errors_on_non_repo_dir() {
        let tmp = tau_ports::fixtures::scratch_dir("git-resolve-head-non-repo");
        let result = Git::resolve_head(tmp.path());
        match result {
            Err(GitError::CommandFailed { what, .. }) => {
                assert_eq!(what, "git rev-parse HEAD");
            }
            Err(GitError::GitMissing) => {
                eprintln!("warning: `git` not on PATH, skipping git smoke test");
            }
            Err(other) => panic!("expected CommandFailed, got: {other:?}"),
            Ok(commit) => panic!("expected error on non-repo dir, got Ok({commit:?})"),
        }
    }

    // Note: a real `clone` test would exercise the network or a
    // file://-based local fixture. Both belong in Task 14's
    // integration test suite (tests/install_lifecycle.rs), where the
    // test infrastructure for `git init --bare` already exists.
}
