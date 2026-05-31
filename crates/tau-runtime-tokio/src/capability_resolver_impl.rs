//! `tau_pkg`-backed implementation of the [`CapabilityResolver`] port.
//!
//! Wraps `tau_pkg::capability_override::compute_effective` so the
//! kernel can call into the project-override system through a stable
//! trait without linking `tau-pkg` directly (which is std-only).
//!
//! Construct with [`TauPkgCapabilityResolver::new`] from a vector of
//! [`CapabilityOverride`] entries — typically the
//! `[agents.<id>.capabilities]` table parsed out of the project's
//! `tau.toml` by tau-cli.

use std::sync::Arc;

use tau_domain::{Capability, FsCapability, NetCapability, ProcessCapability};
use tau_pkg::capability_override::{compute_effective, CapabilityOverride};
use tau_ports::{CapabilityResolver, CapabilityResolverError, DenyEntry, ResolvedGrants};

use crate::run::narrowed_capability_for_session;

/// Production capability resolver: applies project-level
/// `CapabilityOverride` entries to a package manifest's declared
/// capabilities via `tau_pkg::capability_override::compute_effective`.
///
/// Build once per run from the agent's `project_override` list and
/// stuff `Arc<TauPkgCapabilityResolver>` into
/// `RunOptions.capability_resolver`. The kernel calls `resolve()` once
/// at the start of `run_streaming_with_history` to compute the
/// `(for_kernel, for_session, deny_entries)` triple.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use tau_runtime::capability_resolver_impl::TauPkgCapabilityResolver;
/// use tau_ports::CapabilityResolver;
///
/// // Empty override list — a passthrough resolver in practice.
/// let resolver: Arc<dyn CapabilityResolver> = Arc::new(TauPkgCapabilityResolver::new(vec![]));
/// let grants = resolver.resolve(&[]).expect("resolve");
/// assert!(grants.for_kernel.is_empty());
/// assert!(grants.for_session.is_empty());
/// assert!(grants.deny_entries.is_empty());
/// ```
pub struct TauPkgCapabilityResolver {
    overrides: Vec<CapabilityOverride>,
}

impl TauPkgCapabilityResolver {
    /// Construct from the agent's project-side capability override list.
    pub fn new(overrides: Vec<CapabilityOverride>) -> Self {
        Self { overrides }
    }
}

impl CapabilityResolver for TauPkgCapabilityResolver {
    fn resolve(
        &self,
        manifest_capabilities: &[Capability],
    ) -> Result<ResolvedGrants, CapabilityResolverError> {
        let effective = compute_effective(manifest_capabilities, &self.overrides).map_err(|e| {
            CapabilityResolverError {
                kind: e.kind,
                reason: e.reason,
            }
        })?;

        let for_kernel: Vec<Capability> = effective.iter().map(|e| e.source.clone()).collect();
        let for_session: Vec<Capability> = effective
            .iter()
            .map(narrowed_capability_for_session)
            .collect();
        let deny_entries: Vec<DenyEntry> = effective
            .iter()
            .filter(|e| !e.deny.is_empty())
            .map(|e| {
                let kind = match &e.source {
                    Capability::Filesystem(FsCapability::Read { .. }) => "fs.read",
                    Capability::Filesystem(FsCapability::Write { .. }) => "fs.write",
                    Capability::Filesystem(FsCapability::Exec { .. }) => "fs.exec",
                    Capability::Network(NetCapability::Http { .. }) => "net.http",
                    Capability::Process(ProcessCapability::Spawn { .. }) => "process.spawn",
                    _ => "unknown",
                };
                DenyEntry::new(kind.to_string(), e.deny.clone())
            })
            .collect();

        Ok(ResolvedGrants {
            for_kernel,
            for_session,
            deny_entries,
        })
    }
}

/// Convenience: build an `Arc<dyn CapabilityResolver>` from an override
/// list. Equivalent to `Arc::new(TauPkgCapabilityResolver::new(o))` but
/// returns the trait object directly.
pub fn resolver_from_overrides(overrides: Vec<CapabilityOverride>) -> Arc<dyn CapabilityResolver> {
    Arc::new(TauPkgCapabilityResolver::new(overrides))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use tau_domain::Capability;

    fn parse_cap(toml: &str) -> Capability {
        #[derive(serde::Deserialize)]
        struct W {
            cap: Capability,
        }
        toml::from_str::<W>(toml).expect("parse").cap
    }

    #[test]
    fn empty_overrides_pass_through_manifest_unchanged() {
        let cap = parse_cap(
            r#"[cap]
kind = "fs.read"
paths = ["/src/**"]
"#,
        );
        let manifest = vec![cap.clone()];
        let resolver = TauPkgCapabilityResolver::new(vec![]);
        let resolved = resolver.resolve(&manifest).unwrap();
        assert_eq!(resolved.for_kernel.len(), 1);
        assert_eq!(resolved.for_session.len(), 1);
        assert!(resolved.deny_entries.is_empty());
    }

    #[test]
    fn deny_only_override_emits_deny_entry() {
        let cap = parse_cap(
            r#"[cap]
kind = "fs.read"
paths = ["/src/**"]
"#,
        );
        let manifest = vec![cap];
        // CapabilityOverride is non_exhaustive — use constructor.
        let ov = CapabilityOverride::new(
            String::from_str("fs.read").unwrap(),
            None,
            vec!["/src/secrets/**".into()],
            None,
        );
        let resolver = TauPkgCapabilityResolver::new(vec![ov]);
        let resolved = resolver.resolve(&manifest).unwrap();
        assert_eq!(resolved.deny_entries.len(), 1);
        assert_eq!(resolved.deny_entries[0].kind, "fs.read");
    }
}
