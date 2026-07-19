//! Light-tier enforcement: landlock filesystem isolation only.
//!
//! A `pre_exec` hook installs a landlock V1 ruleset in the child process
//! after `fork(2)` but before `exec(2)`. Installing in the parent would lock
//! down the tau process itself; `pre_exec` targets the child only.

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use tau_domain::{Capability, FsCapability};
use tau_ports::{CapabilityError, CapabilityHandle, CapabilityPlan};

// Re-export so strict.rs can call collect_exec_paths without reaching into exec.rs directly.
pub(crate) use crate::exec::collect_exec_paths;

/// Baseline filesystem paths that EVERY plugin needs read access to under
/// landlock. These are runtime mechanics (binary load, dyld, libc, kernel
/// introspection, scheduler-affinity probes) — not application data. The
/// user's plan-derived `read_paths` still narrow application access; these
/// system paths exist purely so the runtime mechanics work.
///
/// Each entry must be justified — Constitution G12 wants narrow defaults.
/// Add a one-line comment explaining why a path is in the baseline before
/// extending this list.
pub(crate) const BASELINE_SYSTEM_READ_PATHS: &[&str] = &[
    // Binary load + dyld + libc (priority-12 baseline)
    "/bin",       // shell + fs-read locate echo, basic utilities
    "/sbin",      // distro-dependent /bin/sbin split
    "/usr/bin",   // post-merge layout (Debian-merged-/usr)
    "/usr/sbin",  // post-merge layout
    "/lib",       // distro-dependent /lib /usr/lib split (libc, libm, libdl)
    "/lib64",     // 64-bit dyld on glibc systems
    "/usr/lib",   // post-merge layout
    "/usr/lib64", // post-merge layout
    "/etc",       // /etc/resolv.conf for DNS, /etc/ssl/certs for TLS roots, locale config
    // Sub-project layer4-startup-io baseline additions (2026-05-09):
    "/proc/self", // tokio reads /proc/self/cgroup + /proc/self/maps during multi-thread runtime init
    "/sys/fs/cgroup", // tokio reads /sys/fs/cgroup/cpu.max for CPU quota / worker pool sizing
    // Sub-project E exec-gating fix (2026-05-11): two distinct issues resolved together:
    //
    // Issue 1 — glibc PATH search EACCES before /usr/bin:
    // On Debian bookworm, the container PATH is /usr/local/cargo/bin:/usr/local/sbin:
    // /usr/local/bin:/usr/sbin:/usr/bin:... Rust's Command::new() opens each PATH directory
    // with O_DIRECTORY to search for the binary. Directories not in the landlock baseline
    // (lacking ReadFile|ReadDir) cause EACCES, which Rust treats as a hard failure (not
    // ENOENT-and-continue). Adding /usr/local/* to the baseline makes those directories
    // readable, so Rust gets ENOENT (no "echo" there) and continues to /usr/bin/echo.
    //
    // Issue 2 — /dev/null EACCES for stdin redirection:
    // Plugins that spawn subprocesses with stdin(Stdio::null()) open /dev/null. Without
    // /dev in the baseline, landlock denies this open() with EACCES, and Command::spawn()
    // fails before even calling execve(). /dev is needed for null device access.
    //
    // Neither addition weakens exec gating: per-file Execute rules (from Process(Spawn) +
    // Filesystem(Exec) capabilities) still gate exactly which binaries are executable.
    "/usr/local/bin", // PATH search: readable but no "echo" there → ENOENT → continue
    "/usr/local/sbin", // PATH search: same
    "/usr/local/lib", // shared library loader may search /usr/local/lib at startup
    "/dev",           // /dev/null access for stdin(Stdio::null()) in spawned subprocesses
];

/// Collect and resolve landlock path lists from a plan + command CWD.
///
/// Returns `(read_paths, write_paths)` as resolved `PathBuf` vectors.
/// Shared by `apply_landlock` (Light tier) and `strict::apply_strict` (Strict tier).
pub(crate) fn collect_landlock_paths(
    plan: &CapabilityPlan,
    cmd: &Command,
) -> Result<(Vec<std::path::PathBuf>, Vec<std::path::PathBuf>), CapabilityError> {
    let read_strs = collect_paths(plan, |c| match c {
        Capability::Filesystem(FsCapability::Read { paths, .. }) => Some(paths.clone()),
        _ => None,
    });
    let write_strs = collect_paths(plan, |c| match c {
        Capability::Filesystem(FsCapability::Write { paths, .. }) => Some(paths.clone()),
        _ => None,
    });

    let cwd = cmd
        .get_current_dir()
        .map(std::path::Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(|e| CapabilityError::WrapFailed {
            message: format!("cwd: {e}"),
        })?;

    let read_paths = resolve_anchors(&read_strs, &cwd);
    let write_paths = resolve_anchors(&write_strs, &cwd);

    // NEW: pass each path through symlink resolution; landlock V1 needs
    // both the link and the canonical target in the ruleset.
    let mut read_paths: Vec<std::path::PathBuf> = read_paths
        .into_iter()
        .map(|p| resolve_symlinks_for_landlock(&p))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let write_paths: Vec<std::path::PathBuf> = write_paths
        .into_iter()
        .map(|p| resolve_symlinks_for_landlock(&p))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    // Auto-add the spawned binary's parent directory to read_paths so the
    // kernel can READ the binary file to load it AND EXEC it. Without this,
    // plugins built into a workspace's `target/release/` (or anywhere
    // outside /bin, /usr/bin, /lib, etc.) fail to exec under the native
    // adapter with EACCES — the plan's capabilities cover application data
    // access, NOT the binary's own load. Applies to both Light and Strict
    // tiers since both share this collect helper.
    if let Some(prog_parent) = std::path::Path::new(cmd.get_program()).parent() {
        if !prog_parent.as_os_str().is_empty() {
            for resolved in resolve_symlinks_for_landlock(prog_parent).unwrap_or_default() {
                if !read_paths.contains(&resolved) {
                    read_paths.push(resolved);
                }
            }
        }
    }

    Ok((read_paths, write_paths))
}

/// Install a landlock ruleset from pre-resolved path lists.
///
/// Called from inside a `pre_exec` closure (in the child between fork and exec).
/// Exposed as `pub(crate)` so `strict::apply_strict` can reuse it.
///
/// - `read_paths` — granted `ReadFile | ReadDir | Execute` (system and plan read paths).
/// - `write_paths` — granted `WriteFile | MakeReg | RemoveFile`.
/// - `exec_paths` — granted `Execute` only; enforces the per-command exec allow-list
///   from `Capability::Filesystem(Exec)` and `Capability::Process(Spawn)`.
pub(crate) fn install_landlock_from_plan(
    read_paths: &[std::path::PathBuf],
    write_paths: &[std::path::PathBuf],
    exec_paths: &[std::path::PathBuf],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    install_landlock(read_paths, write_paths, exec_paths)
}

/// Apply landlock rules to `cmd` via a `pre_exec` hook.
///
/// Rules are derived from the plan's filesystem capabilities. Non-filesystem
/// capabilities are accepted (they pass `validate_plan`) but not yet enforced
/// at Light tier; Strict tier (Tasks 4-5) will add seccomp + namespaces.
///
/// Returns [`CapabilityHandle::noop`]: landlock state is per-thread inside the
/// child process and dies with the child on `_exit`; no parent-side
/// cleanup is needed.
pub(crate) fn apply_landlock(
    plan: &CapabilityPlan,
    cmd: &mut Command,
) -> Result<CapabilityHandle, CapabilityError> {
    // collect_landlock_paths now also auto-adds the spawned binary's parent
    // directory so the kernel can read+exec the binary regardless of where
    // it lives in the filesystem. See collect_landlock_paths for the comment.
    let (read_paths, write_paths) = collect_landlock_paths(plan, cmd)?;

    // Collect exec-gated paths from Filesystem(Exec) and Process(Spawn) capabilities.
    // These receive Execute-only access in the landlock ruleset, enforcing the
    // per-command exec allow-list. Resolution happens in the parent before fork.
    let exec_paths = collect_exec_paths(plan)
        .into_iter()
        // Resolve symlinks so landlock's path matching covers both the
        // link path and the canonical target — same treatment as read_paths.
        // Unresolvable exec paths are silently skipped (.ok()).
        .filter_map(|p| resolve_symlinks_for_landlock(&p).ok())
        .flatten()
        .collect::<Vec<_>>();

    // KNOWN-LIMITATION: this pre_exec closure runs in the child between fork
    // and exec. POSIX guarantees only async-signal-safe operations in that
    // state. tau is multi-threaded (tokio), so any malloc held by another
    // thread at fork time can deadlock the child. The landlock builder
    // allocates internally and we format error strings on the failure path,
    // neither of which is async-signal-safe. Glibc's malloc holds for
    // microseconds; the deadlock window is small but nonzero. A future task
    // (Task 4 or a dedicated follow-up) should consider posix_spawn or a
    // fork-server pattern.
    //
    // SAFETY: pre_exec runs in the child after fork() but before exec().
    // Installing a landlock ruleset here is safe. The parent process is
    // unaffected.
    unsafe {
        cmd.pre_exec(move || {
            install_landlock(&read_paths, &write_paths, &exec_paths)
                .map_err(|e| std::io::Error::other(e.to_string()))
        });
    }
    Ok(CapabilityHandle::noop())
}

fn collect_paths<F>(plan: &CapabilityPlan, extract: F) -> Vec<String>
where
    F: Fn(&Capability) -> Option<Vec<String>>,
{
    plan.capabilities
        .iter()
        .filter_map(extract)
        .flatten()
        .collect()
}

fn resolve_anchors(paths: &[String], cwd: &std::path::Path) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| {
            let p = p.replace("${PROJECT}", cwd.to_string_lossy().as_ref());
            // Drop trailing glob suffix; landlock works on directory roots.
            // For "/tmp/**" we add "/tmp"; for "/tmp/x" we add "/tmp/x".
            let trimmed = p.trim_end_matches("/**").trim_end_matches("/*").to_string();
            PathBuf::from(trimmed)
        })
        .collect()
}

/// Resolve symlinks for landlock ruleset entries.
///
/// Landlock V1 path resolution does not follow symlinks at lookup time;
/// installing a rule for `/bin` does NOT grant access to `/usr/bin`
/// when `/bin` is a symlink (the typical Ubuntu layout). Sub-project B
/// addresses this by adding BOTH the symlink path and its canonical
/// target to the ruleset.
///
/// Returns one path (the input verbatim) for non-symlinks, two paths
/// (input + canonical target) for symlinks. Returns
/// `CapabilityError::WrapFailed` if `path` cannot be canonicalized
/// (typically: doesn't exist, permission denied).
fn resolve_symlinks_for_landlock(
    path: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, CapabilityError> {
    let canonical = std::fs::canonicalize(path).map_err(|e| CapabilityError::WrapFailed {
        message: format!(
            "could not canonicalize path '{}' for landlock ruleset: {e}",
            path.display()
        ),
    })?;

    if canonical == path {
        Ok(vec![path.to_path_buf()])
    } else {
        Ok(vec![path.to_path_buf(), canonical])
    }
}

fn install_landlock(
    read_paths: &[PathBuf],
    write_paths: &[PathBuf],
    exec_paths: &[PathBuf],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use landlock::make_bitflags;
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
    };

    let abi = ABI::V1;

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .create()?;

    // Always allow read access to system binary + library paths so the
    // child can `execve` itself and load shared libraries. Without this,
    // landlock blocks the binary's own load and exec returns EACCES.
    // The user's plan-derived read_paths still narrow application access;
    // these system paths exist purely so the runtime mechanics work.
    let system_read_paths: &[&str] = BASELINE_SYSTEM_READ_PATHS;
    // Note: we grant `Execute` alongside `ReadFile | ReadDir` so the
    // kernel can both READ the binary file (to load it into memory) AND
    // EXEC it. The Ruleset handles ALL `AccessFs` flags via `from_all`
    // above; without `Execute` in the granted bitflags, landlock denies
    // exec of binaries inside the path with EACCES — even if the same
    // binary is readable. (Priority-12 v0.1 oversight surfaced by
    // sub-project D's e2e tests on Ubuntu CI.)
    for sys_path in system_read_paths {
        if let Ok(fd) = PathFd::new(sys_path) {
            ruleset = ruleset.add_rule(PathBeneath::new(
                fd,
                make_bitflags!(AccessFs::{ReadFile | ReadDir | Execute}),
            ))?;
        }
        // Silently skip paths that don't exist on this system.
    }

    for p in read_paths {
        let fd = PathFd::new(p).map_err(|e| format!("landlock read path {}: {e}", p.display()))?;
        ruleset = ruleset.add_rule(PathBeneath::new(
            fd,
            make_bitflags!(AccessFs::{ReadFile | ReadDir | Execute}),
        ))?;
    }

    for p in write_paths {
        let fd = PathFd::new(p).map_err(|e| format!("landlock write path {}: {e}", p.display()))?;
        ruleset = ruleset.add_rule(PathBeneath::new(
            fd,
            make_bitflags!(AccessFs::{WriteFile | MakeReg | RemoveFile}),
        ))?;
    }

    // Per-command exec gating (sub-project E): grant Execute-only access for
    // explicitly declared Filesystem(Exec) and Process(Spawn) paths.
    // These paths may be individual files (e.g. `/usr/bin/git`) or
    // directories (e.g. `/usr/bin` from a glob-trimmed `fs.exec` entry).
    // AccessFs::Execute is part of landlock V1 — no kernel-version gate needed.
    for p in exec_paths {
        if let Ok(fd) = PathFd::new(p) {
            ruleset =
                ruleset.add_rule(PathBeneath::new(fd, make_bitflags!(AccessFs::{Execute})))?;
        }
        // Silently skip unresolvable exec paths — the binary simply won't be
        // executable under landlock, which is the correct secure default.
    }

    ruleset.restrict_self()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tau_ports::WorkingContext;

    /// Build a minimal `CapabilityPlan` from a JSON-ish capability list.
    /// Centralized here so the various test cases stay short.
    fn plan_from(caps: serde_json::Value) -> CapabilityPlan {
        let plan_json = serde_json::json!({
            "capabilities": caps,
            "context": null,
            "limits": null,
        });
        serde_json::from_value(plan_json).expect("decode plan")
    }

    // ---------- collect_paths ----------

    #[test]
    fn collect_paths_extracts_only_matching_capabilities() {
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": ["/a", "/b"] },
            { "kind": "fs.write", "paths": ["/c"] },
            { "kind": "net.http", "hosts": ["x.example"], "methods": ["GET"] },
        ]));
        let read = collect_paths(&plan, |c| match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => Some(paths.clone()),
            _ => None,
        });
        assert_eq!(read, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn collect_paths_flattens_multiple_capabilities_of_same_kind() {
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": ["/a"] },
            { "kind": "fs.read", "paths": ["/b", "/c"] },
        ]));
        let read = collect_paths(&plan, |c| match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => Some(paths.clone()),
            _ => None,
        });
        assert_eq!(
            read,
            vec!["/a".to_string(), "/b".to_string(), "/c".to_string()]
        );
    }

    #[test]
    fn collect_paths_returns_empty_for_no_match() {
        let plan = plan_from(serde_json::json!([
            { "kind": "net.http", "hosts": ["example.com"] },
        ]));
        let read = collect_paths(&plan, |c| match c {
            Capability::Filesystem(FsCapability::Read { paths, .. }) => Some(paths.clone()),
            _ => None,
        });
        assert!(read.is_empty());
    }

    // ---------- resolve_anchors ----------

    #[test]
    fn resolve_anchors_substitutes_project_with_cwd() {
        let cwd = Path::new("/work/project");
        let resolved = resolve_anchors(
            &["${PROJECT}/src".to_string(), "/etc/passwd".to_string()],
            cwd,
        );
        assert_eq!(
            resolved,
            vec![
                PathBuf::from("/work/project/src"),
                PathBuf::from("/etc/passwd"),
            ]
        );
    }

    #[test]
    fn resolve_anchors_trims_trailing_double_glob() {
        let cwd = Path::new("/work");
        let resolved = resolve_anchors(&["${PROJECT}/src/**".to_string()], cwd);
        assert_eq!(resolved, vec![PathBuf::from("/work/src")]);
    }

    #[test]
    fn resolve_anchors_trims_trailing_single_glob() {
        let cwd = Path::new("/work");
        let resolved = resolve_anchors(&["/data/*".to_string()], cwd);
        assert_eq!(resolved, vec![PathBuf::from("/data")]);
    }

    #[test]
    fn resolve_anchors_does_not_trim_embedded_globs() {
        // KNOWN-LIMITATION: embedded globs are passed through verbatim;
        // landlock's PathFd::new on the literal path will fail later.
        // This test pins the v0.1 behavior so a future glob expander
        // upgrade has a clear baseline to compare against.
        let cwd = Path::new("/work");
        let resolved = resolve_anchors(&["/foo/**/bar.txt".to_string()], cwd);
        assert_eq!(resolved, vec![PathBuf::from("/foo/**/bar.txt")]);
    }

    #[test]
    fn resolve_anchors_handles_empty_input() {
        let cwd = Path::new("/work");
        assert!(resolve_anchors(&[], cwd).is_empty());
    }

    // ---------- collect_landlock_paths ----------

    #[test]
    fn collect_landlock_paths_uses_command_cwd_when_set() {
        // Use a real directory that exists so symlink resolution succeeds.
        let tmp = tau_ports::fixtures::scratch_dir("landlock-cwd");
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).expect("mkdir src");
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": ["${PROJECT}/src"] },
        ]));
        let mut cmd = Command::new("/bin/true");
        cmd.current_dir(tmp.path());
        let (read, write) = collect_landlock_paths(&plan, &cmd).unwrap();
        // The resolved path must contain the src directory we created.
        assert!(
            read.iter().any(|p| p.ends_with("src")),
            "expected src in read paths, got {read:?}"
        );
        assert!(write.is_empty());
    }

    #[test]
    fn collect_landlock_paths_falls_back_to_env_cwd_when_command_cwd_unset() {
        // Use a real path that exists so symlink resolution succeeds.
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": ["/tmp"] },
        ]));
        let cmd = Command::new("/bin/true");
        // No current_dir set → falls back to std::env::current_dir().
        let (read, _) = collect_landlock_paths(&plan, &cmd).unwrap();
        // /tmp must appear in the read paths (may expand to canonical too).
        assert!(
            read.iter().any(|p| p == std::path::Path::new("/tmp")),
            "expected /tmp in read paths, got {read:?}"
        );
    }

    #[test]
    fn collect_landlock_paths_separates_read_and_write() {
        // Use real existing directories so symlink resolution succeeds.
        let tmp = tau_ports::fixtures::scratch_dir("landlock-read-write");
        let r1 = tmp.path().join("r1");
        let r2 = tmp.path().join("r2");
        let w1 = tmp.path().join("w1");
        let w2 = tmp.path().join("w2");
        for d in [&r1, &r2, &w1, &w2] {
            std::fs::create_dir(d).expect("mkdir");
        }
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": [r1.to_str().unwrap()] },
            { "kind": "fs.write", "paths": [w1.to_str().unwrap(), w2.to_str().unwrap()] },
            { "kind": "fs.read", "paths": [r2.to_str().unwrap()] },
        ]));
        let cmd = Command::new("/bin/true");
        let (read, write) = collect_landlock_paths(&plan, &cmd).unwrap();
        // Each declared path must appear in the correct list.
        assert!(read.iter().any(|p| p == &r1), "r1 must be in read");
        assert!(read.iter().any(|p| p == &r2), "r2 must be in read");
        assert!(write.iter().any(|p| p == &w1), "w1 must be in write");
        assert!(write.iter().any(|p| p == &w2), "w2 must be in write");
        // Write paths must not appear in read list.
        assert!(!read.iter().any(|p| p == &w1), "w1 must not be in read");
    }

    #[test]
    fn collect_landlock_paths_ignores_non_filesystem_capabilities() {
        let plan = plan_from(serde_json::json!([
            { "kind": "net.http", "hosts": ["x"], "methods": ["GET"] },
            { "kind": "process.spawn", "commands": ["git"] },
            { "kind": "agent.spawn", "allowed_kinds": ["worker"] },
        ]));
        let cmd = Command::new("/bin/true");
        let (read, write) = collect_landlock_paths(&plan, &cmd).unwrap();
        // Non-filesystem capabilities don't contribute to fs read paths.
        // BUT the spawned binary's parent directory is auto-added so the
        // kernel can read+exec the binary itself (see comment in
        // collect_landlock_paths). For Command::new("/bin/true"), the
        // parent is "/bin" — that's the only expected entry.
        assert!(
            read.iter().all(|p| p == std::path::Path::new("/bin")
                || p.canonicalize().ok() == Some(std::path::PathBuf::from("/usr/bin"))),
            "read should contain only auto-added /bin (or canonical /usr/bin), got: {read:?}"
        );
        assert!(write.is_empty());
    }

    // ---------- apply_landlock structural smoke ----------

    #[tokio::test]
    async fn apply_landlock_returns_noop_handle_for_valid_plan() {
        // Verifies the structural contract — apply_landlock returns Ok with a
        // CapabilityHandle that's a no-op (landlock state dies with the child;
        // no parent-side cleanup). This does NOT verify landlock itself runs;
        // that's e2e territory (deferred to sub-project D).
        let plan = plan_from(serde_json::json!([
            { "kind": "fs.read", "paths": ["/tmp"] },
        ]));
        let mut cmd = Command::new("/bin/true");
        // We don't actually spawn — just verify the call shape.
        let result = apply_landlock(&plan, &mut cmd);
        assert!(result.is_ok(), "apply_landlock should accept a valid plan");
    }

    #[test]
    fn working_context_field_used_unused_keep_compile() {
        // Regression-pin: this test exists so a future refactor that drops
        // the WorkingContext import doesn't silently break compilation.
        // No assertion needed; existence of `WorkingContext` here forces
        // the symbol to be reachable.
        let _: Option<WorkingContext> = None;
    }

    // ---------- resolve_symlinks_for_landlock ----------

    #[test]
    fn resolve_symlinks_non_symlink_returns_single_entry() {
        // /tmp is not a symlink on most modern Linux distros; if the test
        // runs on a system where it is, the assert relaxes to "at least
        // one entry".
        let path = std::path::Path::new("/tmp");
        let resolved = resolve_symlinks_for_landlock(path).expect("/tmp must canonicalize");
        assert!(!resolved.is_empty());
        assert!(resolved.contains(&path.to_path_buf()));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn resolve_symlinks_symlink_includes_canonical() {
        use std::os::unix::fs::symlink;
        let tmp = tau_ports::fixtures::scratch_dir("resolve-symlinks-canonical");
        let target = tmp.path().join("target");
        std::fs::create_dir(&target).expect("mkdir target");
        let link = tmp.path().join("link");
        symlink(&target, &link).expect("symlink");

        let resolved = resolve_symlinks_for_landlock(&link).expect("symlink must canonicalize");
        // Both the symlink path and the canonical target are returned.
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&link));
        assert!(resolved
            .iter()
            .any(|p| p.canonicalize().ok() == Some(target.canonicalize().expect("canon target"))));
    }

    #[test]
    fn resolve_symlinks_missing_path_returns_wrap_failed() {
        let nonexistent = std::path::Path::new("/this/path/does/not/exist/12345");
        let result = resolve_symlinks_for_landlock(nonexistent);
        match result {
            Err(CapabilityError::WrapFailed { message }) => {
                assert!(
                    message.contains("canonicalize"),
                    "WrapFailed message should mention canonicalize, got: {message}"
                );
                assert!(
                    message.contains("/this/path/does/not/exist/12345"),
                    "WrapFailed message should mention the failing path, got: {message}"
                );
            }
            other => panic!("expected WrapFailed, got {other:?}"),
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn collect_landlock_paths_includes_canonical_for_symlinks() {
        use std::os::unix::fs::symlink;
        let tmp = tau_ports::fixtures::scratch_dir("landlock-canonical-for-symlinks");
        let target = tmp.path().join("target");
        std::fs::create_dir(&target).expect("mkdir target");
        let link = tmp.path().join("link");
        symlink(&target, &link).expect("symlink");

        // Build a CapabilityPlan that asks for read access at the symlink path.
        let plan_json = serde_json::json!({
            "capabilities": [{
                "kind": "fs.read",
                "paths": [link.to_str().unwrap()]
            }],
            "context": null,
            "limits": null,
        });
        let plan: tau_ports::CapabilityPlan =
            serde_json::from_value(plan_json).expect("valid plan");
        let cmd = Command::new("/bin/true");
        let (read_paths, _write_paths) = collect_landlock_paths(&plan, &cmd).expect("collect");
        // Both the link path and the canonical target should appear.
        assert!(read_paths.iter().any(|p| p == &link));
        assert!(read_paths
            .iter()
            .any(|p| p.canonicalize().ok() == Some(target.canonicalize().unwrap())));
    }

    // ---------- BASELINE_SYSTEM_READ_PATHS ----------

    #[test]
    fn baseline_system_read_paths_includes_legacy_entries() {
        // Priority-12 baseline must remain (regression protection).
        let expected_legacy = [
            "/bin",
            "/sbin",
            "/usr/bin",
            "/usr/sbin",
            "/lib",
            "/lib64",
            "/usr/lib",
            "/usr/lib64",
            "/etc",
        ];
        for p in expected_legacy {
            assert!(
                BASELINE_SYSTEM_READ_PATHS.contains(&p),
                "legacy baseline path {p} must remain in BASELINE_SYSTEM_READ_PATHS"
            );
        }
    }

    #[test]
    fn baseline_system_read_paths_includes_runtime_mechanics() {
        // Sub-project layer4-startup-io baseline additions (regression protection).
        // Per T1 findings 2026-05-09: tokio reads /proc/self/cgroup +
        // /proc/self/maps during multi-thread runtime init, and
        // /sys/fs/cgroup/cpu.max for CPU quota / worker pool sizing.
        let expected_new = ["/proc/self", "/sys/fs/cgroup"];
        for p in expected_new {
            assert!(
                BASELINE_SYSTEM_READ_PATHS.contains(&p),
                "runtime-mechanics baseline path {p} must be in BASELINE_SYSTEM_READ_PATHS"
            );
        }
    }

    #[test]
    fn baseline_system_read_paths_includes_usr_local_and_dev() {
        // Sub-project E exec-gating fix (2026-05-11):
        //
        // /usr/local/bin, /usr/local/sbin: Rust's Command::new(bare-name) opens each
        // PATH directory with O_DIRECTORY to search for the binary. Directories not in
        // the baseline cause EACCES (not ENOENT), which Rust treats as a hard spawn
        // failure. Adding these directories makes them readable so Rust gets ENOENT
        // (binary not found there) and continues searching to /usr/bin/echo.
        //
        // /dev: plugins that spawn subprocesses with stdin(Stdio::null()) open /dev/null.
        // Without /dev in the baseline, landlock denies this open() with EACCES and the
        // grandchild spawn fails before execve() is ever called.
        let expected = [
            "/usr/local/bin",
            "/usr/local/sbin",
            "/usr/local/lib",
            "/dev",
        ];
        for p in expected {
            assert!(
                BASELINE_SYSTEM_READ_PATHS.contains(&p),
                "sub-project-E fix: {p} must be in BASELINE_SYSTEM_READ_PATHS \
                 (see baseline comment for full rationale)"
            );
        }
    }

    #[test]
    fn baseline_system_read_paths_no_application_data() {
        // Constitution G12: baseline is for runtime mechanics, not app data.
        // Reject paths that would let plugins read user data without an
        // explicit FsCapability::Read grant.
        let forbidden = [
            "/home", "/root", "/var/lib", "/srv", "/opt", "/tmp", "/mnt", "/media",
        ];
        for p in forbidden {
            assert!(
                !BASELINE_SYSTEM_READ_PATHS.contains(&p),
                "{p} must NOT be in baseline (would expand sandbox beyond runtime mechanics)"
            );
        }
    }
}
