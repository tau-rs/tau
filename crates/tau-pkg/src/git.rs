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
//! Revision pinning: [`Git::clone`] dispatches on [`clone_strategy`].
//! Branch and tag names use `git clone --branch <rev> --single-branch`.
//! A commit SHA — which `--branch` rejects with "Remote branch ... not
//! found" — uses a full `git clone` followed by `git checkout --detach
//! <sha>` (D4), so installs can pin to an immutable commit object.

use std::path::Path;
use std::process::Command;

use tau_domain::PackageSource;

use crate::error::GitError;

/// How [`Git::clone`] should materialize a revision.
///
/// `--branch <rev> --single-branch` only accepts branch and tag names.
/// A commit SHA must be fetched via a full clone followed by a detached
/// `git checkout <sha>` (D4): pinning to an immutable commit is the
/// security-relevant case and previously failed with git's "Remote
/// branch ... not found".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneStrategy {
    /// No `rev` — clone the remote's default branch.
    Default,
    /// A branch or tag name — `git clone --branch <name> --single-branch`.
    Branch(String),
    /// A commit SHA — full clone, then `git checkout --detach <sha>`.
    ///
    /// Limitation: the clone fetches the remote's default branch, so a
    /// SHA reachable only from a *non-default* branch (never merged to
    /// the default) is not present and the checkout fails with
    /// [`GitError::CheckoutFailed`]. Pinning a commit on the default
    /// branch — the security-relevant case — works.
    CommitSha(String),
}

/// True when `rev` is a full git object name (40-hex SHA-1 or 64-hex
/// SHA-256). Abbreviated SHAs are intentionally NOT matched: they are
/// ambiguous with short branch names, and commit pinning always uses
/// the full immutable object name.
pub(crate) fn is_sha_shaped(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Decide the clone strategy for an optional `rev`.
pub(crate) fn clone_strategy(rev: Option<&str>) -> CloneStrategy {
    match rev {
        None => CloneStrategy::Default,
        Some(r) if is_sha_shaped(r) => CloneStrategy::CommitSha(r.to_owned()),
        Some(r) => CloneStrategy::Branch(r.to_owned()),
    }
}

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
    /// Dispatches on [`clone_strategy`]:
    /// - `rev = None` → `git clone <url> <dest>` (default branch).
    /// - branch / tag name → `git clone --branch <rev> --single-branch
    ///   <url> <dest>`.
    /// - commit SHA (40-hex / 64-hex) → `git clone <url> <dest>` then
    ///   `git checkout --detach <sha>` (D4): `--branch` rejects SHAs, so
    ///   pinning an install to an immutable commit needs a post-clone
    ///   checkout. A bad SHA surfaces as [`GitError::CheckoutFailed`].
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

        let strategy = clone_strategy(rev_opt.as_deref());
        match &strategy {
            // SHA pinning needs the full history reachable, so we clone
            // plainly here and `git checkout <sha>` below.
            CloneStrategy::Default | CloneStrategy::CommitSha(_) => {}
            CloneStrategy::Branch(name) => {
                cmd.arg("--branch").arg(name).arg("--single-branch");
            }
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

        // D4: a commit SHA cannot be requested via `--branch`; check it
        // out explicitly after the full clone above.
        if let CloneStrategy::CommitSha(sha) = &strategy {
            Self::checkout_detached(dest, sha)?;
        }

        Ok(())
    }

    /// Detached-checkout `sha` in the freshly-cloned repo at `repo` (D4).
    ///
    /// Runs `git -C <repo> checkout --detach <sha>`. Used only for the
    /// [`CloneStrategy::CommitSha`] path — pinning an install to an
    /// immutable commit object.
    fn checkout_detached(repo: &Path, sha: &str) -> Result<(), GitError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("checkout")
            .arg("--detach")
            .arg(sha)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitMissing
                } else {
                    GitError::Io {
                        message: format!("spawning `git checkout --detach {sha}`: {e}"),
                    }
                }
            })?;

        if !output.status.success() {
            return Err(GitError::CheckoutFailed {
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
    fn clone_strategy_distinguishes_sha_from_branch_and_tag() {
        assert!(matches!(clone_strategy(None), CloneStrategy::Default));
        assert!(matches!(
            clone_strategy(Some("main")),
            CloneStrategy::Branch(_)
        ));
        assert!(matches!(
            clone_strategy(Some("v1.2.0")),
            CloneStrategy::Branch(_)
        ));
        // 40-char SHA-1.
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert!(matches!(
            clone_strategy(Some(sha1)),
            CloneStrategy::CommitSha(_)
        ));
        // 64-char SHA-256.
        let sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(matches!(
            clone_strategy(Some(sha256)),
            CloneStrategy::CommitSha(_)
        ));
        // Hex-ish but wrong length → treated as a branch/tag name.
        assert!(matches!(
            clone_strategy(Some("abc123")),
            CloneStrategy::Branch(_)
        ));
        // 40 chars but not all hex → branch/tag.
        assert!(matches!(
            clone_strategy(Some("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")),
            CloneStrategy::Branch(_)
        ));
    }

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

    /// D4: a SHA-shaped `rev` is cloned-then-checked-out, pinning the
    /// install to that immutable commit object (not the branch tip).
    /// Exercised against a local two-commit `file://` repo.
    #[test]
    fn clone_checks_out_a_pinned_commit_sha() {
        use std::str::FromStr as _;

        if Git::version_check().is_err() {
            eprintln!("warning: `git` not on PATH, skipping SHA-pin clone test");
            return;
        }

        // Build a source repo with two commits; remember commit #1's SHA.
        let src = tau_ports::fixtures::scratch_dir("d4-sha-src");
        let repo = src.path();
        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .expect("git spawn");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
        std::fs::write(repo.join("tau.toml"), "name = \"x\"\n").unwrap();
        run(&["add", "tau.toml"]);
        run(&["commit", "-q", "-m", "commit 1"]);
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let first_sha = String::from_utf8_lossy(&head.stdout).trim().to_owned();
        assert_eq!(
            first_sha.len(),
            40,
            "expected a 40-char SHA-1, got {first_sha:?}"
        );
        // Commit #2 adds a marker file absent from commit #1.
        std::fs::write(repo.join("COMMIT2.txt"), "two\n").unwrap();
        run(&["add", "COMMIT2.txt"]);
        run(&["commit", "-q", "-m", "commit 2"]);

        // Clone, pinning commit #1's SHA.
        let dest_dir = tau_ports::fixtures::scratch_dir("d4-sha-dest");
        let dest = dest_dir.path().join("clone");
        let url = local_file_url(repo);
        let source = PackageSource::from_str(&format!("{url}#{first_sha}")).unwrap();
        Git::clone(&source, &dest).expect("SHA-pinned clone+checkout must succeed");

        assert!(
            dest.join("tau.toml").is_file(),
            "pinned commit #1 must contain tau.toml"
        );
        assert!(
            !dest.join("COMMIT2.txt").exists(),
            "pinned commit #1 must NOT contain commit #2's file"
        );
        assert_eq!(
            Git::resolve_head(&dest).unwrap(),
            first_sha,
            "HEAD must be the pinned commit, not the branch tip"
        );
    }

    /// Build a `file://` URL for a local path (forward slashes; triple
    /// slash for non-absolute / Windows drive paths). Mirrors the
    /// integration-test fixture helper so this unit test is host-portable.
    fn local_file_url(path: &Path) -> String {
        let s = path
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }
}
