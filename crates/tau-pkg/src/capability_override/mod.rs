//! Capability override — narrows a package manifest's grants under a
//! project tau.toml `[agents.<id>.capabilities]` table.
//!
//! See `docs/superpowers/specs/2026-04-30-capability-override-design.md` §6, §7.3.
//!
//! `Capability` and inner enums are `#[non_exhaustive]` — variant fields
//! cannot be constructed cross-crate. The override layer side-loads the
//! narrowed allow-list and deny-list onto an `EffectiveCapability` rather
//! than re-constructing variants.

pub mod subset;
pub use subset::{capability_set_subset, CeilingViolation};

use tau_domain::{Capability, FsCapability, NetCapability, ProcessCapability};

/// Override entry parsed from project tau.toml. Constructed by tau-cli at
/// parse time and passed through to the runtime via `RunOptions.project_override`.
///
/// # Example
///
/// ```
/// use tau_pkg::capability_override::CapabilityOverride;
///
/// let ov = CapabilityOverride::new(
///     "fs.read".to_string(),
///     Some(vec!["/proj/src/**".to_string()]),
///     vec!["/proj/secrets/**".to_string()],
///     None,
/// );
/// assert_eq!(ov.kind, "fs.read");
/// assert_eq!(ov.allow.as_deref().unwrap(), &["/proj/src/**".to_string()]);
/// assert_eq!(ov.deny, vec!["/proj/secrets/**".to_string()]);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // wired up by Task 3 (tau-cli) and Task 5 (RunOptions)
pub struct CapabilityOverride {
    /// Capability kind discriminator (`fs.read`, `fs.write`, `fs.exec`,
    /// `net.http`, `process.spawn`).
    pub kind: String,
    /// Narrowed allow-list. `None` means "use the source's own field".
    pub allow: Option<Vec<String>>,
    /// Strings to subtract from the effective allow-list.
    pub deny: Vec<String>,
    /// Narrowed `max_bytes` for `fs.write`. `None` means "use the source's value".
    pub max_bytes: Option<u64>,
}

impl CapabilityOverride {
    /// Construct a `CapabilityOverride`. `#[non_exhaustive]` blocks struct-literal
    /// construction outside this crate.
    #[allow(dead_code)] // wired up by Task 3 (tau-cli) and Task 5 (RunOptions)
    pub fn new(
        kind: String,
        allow: Option<Vec<String>>,
        deny: Vec<String>,
        max_bytes: Option<u64>,
    ) -> Self {
        Self {
            kind,
            allow,
            deny,
            max_bytes,
        }
    }
}

/// Effective capability after applying the project override.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired up by Task 5 (RunOptions) and Task 7/8 (deny enforcement)
pub struct EffectiveCapability {
    /// The package-side capability as-given. Field values inside this
    /// struct are NOT narrowed — they remain the package's grant.
    pub source: Capability,
    /// Narrowed allow-list. Same shape as the strings inside `source`
    /// (paths/hosts/commands). `None` means use `source`'s own field.
    pub allow_override: Option<Vec<String>>,
    /// Deny-list to subtract. Empty = no carve-outs.
    pub deny: Vec<String>,
    /// Narrowed `max_bytes` for `fs.write`. `None` means use source's value.
    pub max_bytes_override: Option<u64>,
}

/// Error returned when a project override expands the package's grants.
///
/// # Example
///
/// ```
/// use tau_pkg::capability_override::OverrideExpandError;
///
/// let err = OverrideExpandError {
///     kind: "fs.read".to_string(),
///     reason: "allow entry \"/etc/**\" is not a subset of any package grant".to_string(),
/// };
/// let display = format!("{err}");
/// assert!(display.contains("fs.read"));
/// assert!(display.contains("expands package grant"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // wired up by Task 5 (RunOptions) and Task 6 (RuntimeError)
pub struct OverrideExpandError {
    /// The capability kind that expanded.
    pub kind: String,
    /// Human-readable reason.
    pub reason: String,
}

impl std::fmt::Display for OverrideExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "capability override on {:?} expands package grant: {}",
            self.kind, self.reason
        )
    }
}

impl std::error::Error for OverrideExpandError {}

/// Compute the effective capability set by intersecting `package_caps` with
/// `project_override`. Returns the effective list, or `OverrideExpandError`
/// if any override entry expands the corresponding package grant.
///
/// # Example
///
/// ```
/// use tau_pkg::capability_override::{CapabilityOverride, compute_effective};
/// use tau_domain::Capability;
///
/// // Package declares fs.read on /proj/**; no override → effective passes through.
/// let pkg: Vec<Capability> = serde_json::from_str(
///     r#"[{"kind":"fs.read","paths":["/proj/**"]}]"#
/// ).expect("parse capability");
/// let result = compute_effective(&pkg, &[]).expect("compute_effective");
/// assert_eq!(result.len(), 1);
/// assert!(result[0].allow_override.is_none());
/// ```
#[allow(dead_code)] // wired up by Task 5 (run.rs)
pub fn compute_effective(
    package_caps: &[Capability],
    project_override: &[CapabilityOverride],
) -> Result<Vec<EffectiveCapability>, OverrideExpandError> {
    // Reject duplicate kinds in the override itself.
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for ov in project_override {
        if !seen.insert(ov.kind.as_str()) {
            return Err(OverrideExpandError {
                kind: ov.kind.clone(),
                reason: "duplicate kind in project override".into(),
            });
        }
    }

    // Reject override entries that have no matching package cap, or that
    // target a Capability::Custom.
    for ov in project_override {
        match find_package_cap(package_caps, &ov.kind) {
            None => {
                return Err(OverrideExpandError {
                    kind: ov.kind.clone(),
                    reason: "no matching capability in package manifest".into(),
                });
            }
            Some(Capability::Custom { .. }) => {
                return Err(OverrideExpandError {
                    kind: ov.kind.clone(),
                    reason: "custom capabilities are not narrowable at v0.1".into(),
                });
            }
            _ => {}
        }
    }

    // Build the effective list: each package cap with its matching override
    // applied (if any).
    let mut effective: Vec<EffectiveCapability> = Vec::with_capacity(package_caps.len());
    for cap in package_caps {
        // Match override entries to package caps using the same identity
        // function as `find_package_cap` (Custom by `name`, others by
        // `cap_kind`). Keeps the validation pass and the build pass in
        // logical sync — without this, a future maintainer adding a new
        // identity rule has to remember to update two call sites.
        let ov = project_override.iter().find(|o| match cap {
            Capability::Custom { name, .. } => &o.kind == name,
            _ => o.kind == cap_kind(cap),
        });
        let kind = match cap {
            Capability::Custom { name, .. } => name.as_str(),
            _ => cap_kind(cap),
        };
        let entry = match ov {
            None => EffectiveCapability {
                source: cap.clone(),
                allow_override: None,
                deny: Vec::new(),
                max_bytes_override: None,
            },
            Some(ov) => {
                if let Some(allow) = &ov.allow {
                    validate_allow_subset(cap, allow).map_err(|reason| OverrideExpandError {
                        kind: kind.to_string(),
                        reason,
                    })?;
                }
                if let Some(mb) = ov.max_bytes {
                    validate_max_bytes(cap, mb).map_err(|reason| OverrideExpandError {
                        kind: kind.to_string(),
                        reason,
                    })?;
                }
                EffectiveCapability {
                    source: cap.clone(),
                    allow_override: ov.allow.clone(),
                    deny: ov.deny.clone(),
                    max_bytes_override: ov.max_bytes,
                }
            }
        };
        effective.push(entry);
    }
    Ok(effective)
}

#[allow(dead_code)] // wired up by compute_effective, itself wired up by Task 5
fn find_package_cap<'a>(caps: &'a [Capability], kind: &str) -> Option<&'a Capability> {
    caps.iter().find(|c| match c {
        Capability::Custom { name, .. } => name == kind,
        _ => cap_kind(c) == kind,
    })
}

#[allow(dead_code)] // wired up by compute_effective, itself wired up by Task 5
fn cap_kind(cap: &Capability) -> &'static str {
    match cap {
        Capability::Filesystem(FsCapability::Read { .. }) => "fs.read",
        Capability::Filesystem(FsCapability::Write { .. }) => "fs.write",
        Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec",
        Capability::Network(NetCapability::Http { .. }) => "net.http",
        Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn",
        Capability::Agent(_) => "agent.spawn",
        Capability::Custom { .. } => "custom",
        // Catch-all for future Capability variants — typed as expand-rejected
        // until support is added explicitly.
        _ => "unknown",
    }
}

#[allow(dead_code)] // wired up by compute_effective, itself wired up by Task 5
fn validate_allow_subset(cap: &Capability, allow: &[String]) -> Result<(), String> {
    // A `net.http` grant of `HostSet::Any` subsumes every host, so any override
    // list is a valid narrowing — short-circuit before the flat string-subset
    // compare below. Without this, `Any` flattens to an EMPTY `exact_hosts()`
    // list and `string_set_subset` would reject every non-empty override
    // (falsely treating a narrowing as a grant expansion).
    if let Capability::Network(NetCapability::Http { hosts, .. }) = cap {
        if hosts.is_any() {
            return Ok(());
        }
    }
    // `Vec<String>` for the fs/process shapes; `net.http`'s `Exact` hosts are
    // flattened to their canonical string form here (`exact_hosts()`).
    let parents: Vec<String> = match cap {
        Capability::Filesystem(FsCapability::Read { paths, .. })
        | Capability::Filesystem(FsCapability::Write { paths, .. })
        | Capability::Filesystem(FsCapability::Exec { paths, .. }) => paths.clone(),
        Capability::Network(NetCapability::Http { hosts, .. }) => hosts.exact_hosts(),
        Capability::Process(ProcessCapability::Spawn { commands, .. }) => commands.clone(),
        _ => {
            return Err("allow narrowing not supported for this capability kind".into());
        }
    };
    if matches!(cap, Capability::Filesystem(_)) {
        subset::paths_subset(allow, &parents).map_err(|offender| {
            format!("allow entry {offender:?} is not a subset of any package grant")
        })
    } else {
        subset::string_set_subset(allow, &parents)
            .map_err(|offender| format!("allow entry {offender:?} is not in package grant"))
    }
}

#[allow(dead_code)] // wired up by compute_effective, itself wired up by Task 5
fn validate_max_bytes(cap: &Capability, requested: u64) -> Result<(), String> {
    match cap {
        Capability::Filesystem(FsCapability::Write { max_bytes, .. }) => {
            subset::max_bytes_le(requested, *max_bytes).map_err(|_| {
                format!(
                    "max_bytes={requested} exceeds package grant {}",
                    max_bytes.expect("max_bytes_le only errs when the ceiling is Some")
                )
            })
        }
        _ => Err("max_bytes only meaningful for fs.write".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(json: &str) -> Capability {
        serde_json::from_str(json).expect("test capability JSON must be valid")
    }

    fn ov(
        kind: &str,
        allow: Option<Vec<String>>,
        deny: Vec<String>,
        max_bytes: Option<u64>,
    ) -> CapabilityOverride {
        CapabilityOverride::new(kind.to_string(), allow, deny, max_bytes)
    }

    #[test]
    fn net_http_any_grant_admits_any_narrowing_override() {
        // An `Any` package grant subsumes every host, so a concrete override
        // list is a valid narrowing — must NOT be rejected as a grant expansion.
        let grant = cap(r#"{"kind":"net.http","hosts":"any"}"#);
        assert!(validate_allow_subset(&grant, &["specific.com".to_string()]).is_ok());
        assert!(validate_allow_subset(&grant, &[]).is_ok());
    }

    #[test]
    fn net_http_exact_grant_rejects_override_outside_hosts() {
        let grant = cap(r#"{"kind":"net.http","hosts":["api.x.com"]}"#);
        assert!(validate_allow_subset(&grant, &["api.x.com".to_string()]).is_ok());
        let err = validate_allow_subset(&grant, &["evil.com".to_string()]).unwrap_err();
        assert!(err.contains("evil.com"), "got: {err}");
    }

    #[test]
    fn no_override_returns_package_caps_unchanged() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let eff = compute_effective(&pkg, &[]).unwrap();
        assert_eq!(eff.len(), 1);
        assert!(eff[0].allow_override.is_none());
        assert!(eff[0].deny.is_empty());
    }

    #[test]
    fn well_formed_fs_read_override_narrows() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let over = vec![ov(
            "fs.read",
            Some(vec!["/proj/src/**".into()]),
            vec!["/proj/secrets/**".into()],
            None,
        )];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(
            eff[0].allow_override.as_deref().unwrap(),
            &["/proj/src/**".to_string()]
        );
        assert_eq!(eff[0].deny, vec!["/proj/secrets/**".to_string()]);
    }

    #[test]
    fn allow_outside_package_scope_rejected() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/src/**"]}"#)];
        let over = vec![ov("fs.read", Some(vec!["/etc/**".into()]), vec![], None)];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert_eq!(err.kind, "fs.read");
        assert!(err.reason.contains("not a subset"), "got: {}", err.reason);
    }

    #[test]
    fn override_kind_with_no_matching_package_cap_rejected() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let over = vec![ov("fs.write", Some(vec!["/proj/**".into()]), vec![], None)];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert_eq!(err.kind, "fs.write");
        assert!(err.reason.contains("no matching"), "got: {}", err.reason);
    }

    #[test]
    fn duplicate_kind_in_override_rejected() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let over = vec![
            ov("fs.read", Some(vec!["/proj/src/**".into()]), vec![], None),
            ov("fs.read", Some(vec!["/proj/docs/**".into()]), vec![], None),
        ];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert!(err.reason.contains("duplicate"), "got: {}", err.reason);
    }

    #[test]
    fn custom_capability_not_narrowable() {
        let pkg = vec![cap(r#"{"kind":"custom.mcp.tool.use","tool":"x"}"#)];
        let over = vec![ov("custom.mcp.tool.use", Some(vec![]), vec![], None)];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert!(err.reason.contains("custom"), "got: {}", err.reason);
    }

    #[test]
    fn process_spawn_string_subset_check() {
        let pkg = vec![cap(
            r#"{"kind":"process.spawn","commands":["git","rg","sed"]}"#,
        )];
        let over = vec![ov(
            "process.spawn",
            Some(vec!["git".into(), "rg".into()]),
            vec![],
            None,
        )];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(
            eff[0].allow_override.as_deref().unwrap(),
            &["git".to_string(), "rg".to_string()]
        );
    }

    #[test]
    fn process_spawn_command_outside_package_rejected() {
        let pkg = vec![cap(r#"{"kind":"process.spawn","commands":["git"]}"#)];
        let over = vec![ov("process.spawn", Some(vec!["rm".into()]), vec![], None)];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert!(
            err.reason.contains("not in package grant"),
            "got: {}",
            err.reason
        );
    }

    #[test]
    fn fs_write_max_bytes_lower_accepted() {
        let pkg = vec![cap(
            r#"{"kind":"fs.write","paths":["/proj/build/**"],"max_bytes":5000000}"#,
        )];
        let over = vec![ov("fs.write", None, vec![], Some(1_000_000))];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(eff[0].max_bytes_override, Some(1_000_000));
    }

    #[test]
    fn fs_write_max_bytes_higher_rejected() {
        let pkg = vec![cap(
            r#"{"kind":"fs.write","paths":["/proj/build/**"],"max_bytes":1000000}"#,
        )];
        let over = vec![ov("fs.write", None, vec![], Some(5_000_000))];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert!(
            err.reason.contains("exceeds package grant"),
            "got: {}",
            err.reason
        );
    }

    #[test]
    fn fs_write_max_bytes_with_unlimited_package_accepted() {
        let pkg = vec![cap(r#"{"kind":"fs.write","paths":["/proj/build/**"]}"#)];
        let over = vec![ov("fs.write", None, vec![], Some(1_000_000))];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(eff[0].max_bytes_override, Some(1_000_000));
    }

    #[test]
    fn deny_with_no_matching_package_path_accepted() {
        // Deny is pure subtraction — no subset check.
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let over = vec![ov("fs.read", None, vec!["/totally/elsewhere".into()], None)];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(eff[0].deny, vec!["/totally/elsewhere".to_string()]);
    }

    #[test]
    fn empty_allow_means_zero_scope() {
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let over = vec![ov("fs.read", Some(vec![]), vec![], None)];
        let eff = compute_effective(&pkg, &over).unwrap();
        assert_eq!(eff[0].allow_override.as_deref().unwrap(), &[] as &[String]);
    }

    #[test]
    fn agent_spawn_allow_narrowing_rejected() {
        // agent.spawn cannot be narrowed via this override layer at v0.1.
        // The fall-through arm in validate_allow_subset returns
        // "allow narrowing not supported for this capability kind".
        let pkg = vec![cap(r#"{"kind":"agent.spawn","allowed_kinds":["worker"]}"#)];
        let over = vec![ov("agent.spawn", Some(vec!["worker".into()]), vec![], None)];
        let err = compute_effective(&pkg, &over).unwrap_err();
        assert!(
            err.reason.contains("allow narrowing not supported"),
            "got: {}",
            err.reason
        );
    }
}
