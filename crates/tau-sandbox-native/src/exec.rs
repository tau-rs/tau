//! Per-command exec gating helpers for the Strict sandbox tier.
//!
//! # Architecture — landlock-based exec gating
//!
//! True per-command exec gating is implemented by granting `AccessFs::Execute`
//! on the **listed** paths only via landlock's per-path filesystem rules, and
//! deliberately omitting `Execute` from all other paths in the ruleset.
//!
//! This works because:
//! 1. `AccessFs::Execute` is part of landlock **V1** (kernel ≥ 5.13) — no
//!    V2/V3 upgrade is needed.
//! 2. landlock V1 covers every filesystem object by its path at `restrict_self()`
//!    time; any path not covered by an `AccessFs::Execute`-granting rule will
//!    return `EACCES` on `execve(2)`.
//!
//! ## What this module provides
//!
//! [`collect_exec_paths`] — extracts the allow-listed executable paths from
//! the plan's capabilities:
//!
//! - `Capability::Filesystem(Exec { paths })` — each entry is used verbatim
//!   after trimming trailing glob suffixes (same convention as read/write paths
//!   in `light.rs::resolve_anchors`).
//! - `Capability::Process(Spawn { commands })` — each entry is either:
//!   - a full path (starts with `/`) → used directly.
//!   - a bare command name (`git`, `ls`) → resolved via `PATH` environment
//!     variable; unresolvable names are silently skipped.
//!
//! The resolved paths are returned as `Vec<PathBuf>`. `light.rs` passes them
//! to `install_landlock` which grants each one `AccessFs::Execute`.
//!
//! ## seccomp (unchanged)
//!
//! [`extend_with_exec_rules`] remains a **no-op** on the seccomp BPF rule
//! map. `execve` and `execveat` must remain unconditionally allowed at the
//! seccomp layer so the initial plugin exec succeeds. The actual per-path
//! restriction is landlock's job (see above). See the in-function doc for
//! the full seccomp rationale.

use std::collections::BTreeMap;
use std::path::PathBuf;

use seccompiler::SeccompRule;
use tau_domain::{Capability, FsCapability, ProcessCapability};
use tau_ports::CapabilityPlan;

/// Collect the paths that should receive `AccessFs::Execute`-only landlock
/// rules from the plan's exec-related capabilities.
///
/// Two capability variants contribute:
/// - `Capability::Filesystem(Exec { paths })` — paths are trimmed of trailing
///   glob suffixes and used directly.
/// - `Capability::Process(Spawn { commands })` — full-path commands are used
///   directly; bare command names are resolved through the `PATH` environment
///   variable at rule-collection time (mirroring shell-resolution at spawn
///   time). Commands that cannot be resolved are silently skipped.
///
/// The caller (`light.rs::install_landlock`) is responsible for adding each
/// returned path to the landlock ruleset with `AccessFs::Execute` access.
pub(crate) fn collect_exec_paths(plan: &CapabilityPlan) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();

    for cap in &plan.capabilities {
        match cap {
            Capability::Filesystem(FsCapability::Exec {
                paths: exec_paths, ..
            }) => {
                for p in exec_paths {
                    // Trim trailing glob suffix — landlock's PathFd works on
                    // directory roots or exact file paths, not glob patterns.
                    let trimmed = p.trim_end_matches("/**").trim_end_matches("/*").to_string();
                    paths.push(PathBuf::from(trimmed));
                }
            }
            Capability::Process(ProcessCapability::Spawn { commands, .. }) => {
                for cmd in commands {
                    if cmd.starts_with('/') {
                        // Full path — use directly.
                        paths.push(PathBuf::from(cmd));
                    } else {
                        // Bare command name — resolve via PATH.
                        if let Some(resolved) = resolve_command_in_path(cmd) {
                            paths.push(resolved);
                        }
                        // Unresolvable commands are silently skipped; the
                        // plugin simply cannot exec them under landlock, which
                        // is the correct secure behavior.
                    }
                }
            }
            _ => {}
        }
    }

    paths
}

/// Resolve a bare command name (e.g. `"git"`) to a full path by searching
/// the `PATH` environment variable, mirroring standard shell resolution.
///
/// Returns the first matching executable path, or `None` if the command
/// cannot be found. Entries in `PATH` that don't exist or aren't
/// accessible are silently skipped.
fn resolve_command_in_path(cmd: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Extend the seccomp rules map with exec-gating rules for the given plan.
///
/// # Seccomp cannot gate exec by path — this is intentionally a no-op
///
/// `execve` and `execveat` remain in the allow-list unconditionally (as set
/// by `baseline_syscall_map`). The reasons are:
///
/// 1. **Plugin-startup chicken-and-egg.** The seccomp filter is installed in
///    `pre_exec` — before the kernel runs `execve(<plugin binary>)`. If we
///    deny `execve` at the seccomp layer, the plugin can never start.
///
/// 2. **Pointer-vs-content.** `seccomp SeccompCondition::Arg(0, ...)` compares
///    the *pointer value* of `execve`'s `path` argument, not the string
///    contents the pointer addresses. A per-path allow-list is impossible via
///    BPF arg matching.
///
/// The per-path exec restriction is enforced via landlock in
/// `light.rs::install_landlock` using the paths from [`collect_exec_paths`].
/// This function exists only to satisfy the call site in `strict.rs` and as
/// the placeholder for any future seccomp-level exec-adjacent rules.
#[allow(unused_variables)]
pub(crate) fn extend_with_exec_rules(
    rules: &mut BTreeMap<i64, Vec<SeccompRule>>,
    plan: &CapabilityPlan,
) {
    // No-op: exec path gating is enforced by landlock (collect_exec_paths +
    // install_landlock), not by seccomp arg matching. See function doc.
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::fixtures::{cap_fs_exec, cap_process_spawn};
    use tau_ports::fixtures::plan_from_capabilities;

    fn plan_with_caps(capabilities: Vec<tau_domain::Capability>) -> CapabilityPlan {
        plan_from_capabilities(capabilities)
    }

    // ---------- collect_exec_paths ----------

    /// Empty plan → empty exec path list.
    #[test]
    fn collect_exec_paths_empty_plan_returns_empty() {
        let plan = plan_with_caps(vec![]);
        let paths = collect_exec_paths(&plan);
        assert!(paths.is_empty(), "expected empty, got {paths:?}");
    }

    /// `Capability::Filesystem(Exec { paths })` → those paths appear in the result.
    #[test]
    fn collect_exec_paths_fs_exec_capability_yields_paths() {
        let plan = plan_with_caps(vec![cap_fs_exec(&["/usr/bin/git", "/usr/local/bin/rg"])]);
        let paths = collect_exec_paths(&plan);
        assert!(
            paths.contains(&PathBuf::from("/usr/bin/git")),
            "expected /usr/bin/git in paths, got {paths:?}"
        );
        assert!(
            paths.contains(&PathBuf::from("/usr/local/bin/rg")),
            "expected /usr/local/bin/rg in paths, got {paths:?}"
        );
    }

    /// `Capability::Process(Spawn { commands })` with full paths → those paths appear.
    #[test]
    fn collect_exec_paths_process_spawn_full_path_yields_paths() {
        let plan = plan_with_caps(vec![cap_process_spawn(&["/bin/echo", "/usr/bin/env"])]);
        let paths = collect_exec_paths(&plan);
        assert!(
            paths.contains(&PathBuf::from("/bin/echo")),
            "expected /bin/echo in paths, got {paths:?}"
        );
        assert!(
            paths.contains(&PathBuf::from("/usr/bin/env")),
            "expected /usr/bin/env in paths, got {paths:?}"
        );
    }

    /// Glob suffixes are stripped from `Filesystem(Exec)` paths.
    #[test]
    fn collect_exec_paths_trims_glob_suffix() {
        let plan = plan_with_caps(vec![cap_fs_exec(&["/usr/bin/*"])]);
        let paths = collect_exec_paths(&plan);
        assert!(
            paths.contains(&PathBuf::from("/usr/bin")),
            "expected /usr/bin (trimmed), got {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.to_string_lossy().contains('*')),
            "glob chars must be stripped, got {paths:?}"
        );
    }

    /// Both `Filesystem(Exec)` and `Process(Spawn)` in one plan → union of paths.
    #[test]
    fn collect_exec_paths_combines_fs_exec_and_process_spawn() {
        let plan = plan_with_caps(vec![
            cap_fs_exec(&["/usr/bin/git"]),
            cap_process_spawn(&["/bin/sh"]),
        ]);
        let paths = collect_exec_paths(&plan);
        assert!(
            paths.contains(&PathBuf::from("/usr/bin/git")),
            "expected /usr/bin/git from fs.exec, got {paths:?}"
        );
        assert!(
            paths.contains(&PathBuf::from("/bin/sh")),
            "expected /bin/sh from process.spawn, got {paths:?}"
        );
    }

    /// Non-exec capabilities are ignored by `collect_exec_paths`.
    #[test]
    fn collect_exec_paths_ignores_non_exec_capabilities() {
        let plan_json = serde_json::json!({
            "capabilities": [
                { "kind": "fs.read", "paths": ["/tmp"] },
                { "kind": "net.http", "hosts": ["x.example"], "methods": ["GET"] },
            ],
            "context": null,
            "limits": null,
        });
        let plan: CapabilityPlan = serde_json::from_value(plan_json).expect("valid plan");
        let paths = collect_exec_paths(&plan);
        assert!(
            paths.is_empty(),
            "fs.read and net.http must not contribute exec paths, got {paths:?}"
        );
    }

    // ---------- extend_with_exec_rules (seccomp no-op) ----------

    /// execve must remain in baseline after extend_with_exec_rules runs.
    #[cfg(target_os = "linux")]
    #[test]
    fn execve_remains_in_baseline_after_exec_rules() {
        use crate::strict::baseline_syscall_map;
        let plan = plan_with_caps(vec![
            cap_fs_exec(&["/usr/bin/git"]),
            cap_process_spawn(&["echo"]),
        ]);
        let mut rules = baseline_syscall_map();
        extend_with_exec_rules(&mut rules, &plan);
        assert!(
            rules.contains_key(&libc::SYS_execve),
            "SYS_execve must remain in allow-list (needed for plugin startup)"
        );
    }

    /// Rules map is unchanged after extend_with_exec_rules (seccomp no-op).
    #[cfg(target_os = "linux")]
    #[test]
    fn extend_with_exec_rules_is_seccomp_noop() {
        use crate::strict::baseline_syscall_map;
        let plan = plan_with_caps(vec![
            cap_fs_exec(&["/usr/bin/git"]),
            cap_process_spawn(&["/bin/echo"]),
        ]);
        let mut rules = baseline_syscall_map();
        let keys_before: Vec<i64> = rules.keys().copied().collect();
        extend_with_exec_rules(&mut rules, &plan);
        let keys_after: Vec<i64> = rules.keys().copied().collect();
        assert_eq!(
            keys_before, keys_after,
            "extend_with_exec_rules must not modify the seccomp rules map"
        );
    }
}
