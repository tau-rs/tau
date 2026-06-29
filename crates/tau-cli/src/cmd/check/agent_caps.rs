//! Resolve an agent's package capabilities for the governance lattice
//! (Story 1.5 L1/L2): lockfile → installed manifest → compute_effective →
//! materialize EffectiveCapability back into concrete Capability.

use serde_json::json;

use crate::cmd::check::runner::CheckCtx;
use tau_domain::Capability;
use tau_pkg::capability_override::{compute_effective, EffectiveCapability, OverrideExpandError};
use tau_pkg::project::AgentEntry;
use tau_pkg::{read_manifest, LockFile};

/// Outcome of resolving an agent's package capabilities.
#[allow(dead_code)] // wired up by Task 3 (governance.rs)
pub(crate) enum AgentCaps {
    /// Package installed; `manifest` = declared caps, `effective` = manifest ∩ override.
    Resolved {
        manifest: Vec<Capability>,
        effective: Vec<Capability>,
    },
    /// No lockfile / package not in lockfile / install dir or manifest missing.
    NotInstalled,
    /// The agent's override expanded its package grant (compute_effective failed).
    OverrideExpands(OverrideExpandError),
}

/// Materialize an EffectiveCapability into a concrete Capability by applying
/// the override deltas to `source` via the serde bridge.
fn materialize(e: &EffectiveCapability) -> Capability {
    if e.allow_override.is_none() && e.deny.is_empty() && e.max_bytes_override.is_none() {
        return e.source.clone();
    }
    let mut v = match serde_json::to_value(&e.source) {
        Ok(serde_json::Value::Object(o)) => o,
        _ => return e.source.clone(),
    };
    let field = match v.get("kind").and_then(|k| k.as_str()) {
        Some("fs.read") | Some("fs.write") | Some("fs.exec") => Some("paths"),
        Some("net.http") => Some("hosts"),
        Some("process.spawn") => Some("commands"),
        _ => None,
    };
    if let Some(field) = field {
        let mut list: Vec<String> = match &e.allow_override {
            Some(a) => a.clone(),
            None => v
                .get(field)
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };
        list.retain(|s| !e.deny.contains(s));
        v.insert(field.to_string(), json!(list));
    }
    if let Some(mb) = e.max_bytes_override {
        v.insert("max_bytes".to_string(), json!(mb));
    }
    serde_json::from_value::<Capability>(serde_json::Value::Object(v))
        .unwrap_or_else(|_| e.source.clone())
}

/// Resolve the agent's package capabilities (manifest + effective).
#[allow(dead_code)] // wired up by Task 3 (governance.rs)
pub(crate) fn resolve_agent_caps(agent: &AgentEntry, ctx: &CheckCtx) -> AgentCaps {
    let pkg_name = agent.package.split('@').next().unwrap_or(&agent.package);

    let lockfile_path = ctx.scope.lockfile_path();
    let Ok(lockfile) = LockFile::load(&lockfile_path) else {
        return AgentCaps::NotInstalled;
    };
    let Some(pkg) = lockfile
        .packages
        .iter()
        .find(|p| p.name.as_str() == pkg_name)
    else {
        return AgentCaps::NotInstalled;
    };

    let toml_path = ctx
        .scope
        .package_dir(&pkg.name, &pkg.active_version)
        .join("tau.toml");
    let Ok(manifest) = read_manifest(&toml_path) else {
        return AgentCaps::NotInstalled;
    };
    let manifest_caps: Vec<Capability> = manifest.capabilities().to_vec();

    match compute_effective(&manifest_caps, &agent.capability_overrides) {
        Ok(eff) => AgentCaps::Resolved {
            manifest: manifest_caps,
            effective: eff.iter().map(materialize).collect(),
        },
        Err(e) => AgentCaps::OverrideExpands(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(j: &str) -> Capability {
        serde_json::from_str(j).unwrap()
    }

    #[test]
    fn materialize_passthrough_when_no_override() {
        // EffectiveCapability is #[non_exhaustive] + constructed by compute_effective;
        // build one via compute_effective to keep within the public surface.
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let eff = compute_effective(&pkg, &[]).unwrap();
        let m = materialize(&eff[0]);
        assert_eq!(m, cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#));
    }

    #[test]
    fn materialize_applies_allow_override_and_deny() {
        use tau_pkg::capability_override::CapabilityOverride;
        let pkg = vec![cap(r#"{"kind":"fs.read","paths":["/proj/**"]}"#)];
        let ov = vec![CapabilityOverride::new(
            "fs.read".into(),
            Some(vec!["/proj/src/**".into(), "/proj/docs/**".into()]),
            vec!["/proj/docs/**".into()],
            None,
        )];
        let eff = compute_effective(&pkg, &ov).unwrap();
        let m = materialize(&eff[0]);
        // allow_override minus deny = ["/proj/src/**"]
        assert_eq!(m, cap(r#"{"kind":"fs.read","paths":["/proj/src/**"]}"#));
    }
}
