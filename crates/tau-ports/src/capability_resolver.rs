//! Capability resolver port — turns a package manifest's declared
//! capabilities into the post-override grant set that flows into the
//! agent loop.
//!
//! The kernel doesn't know about project tau.toml overrides — that's
//! a host-shell concern (tau-runtime ships a `tau_pkg`-backed impl).
//! Embassy/wasm shells with no override system can either supply a
//! passthrough impl, or omit the resolver entirely (the kernel falls
//! back to the manifest's declared capabilities unchanged).
//!
//! Routing capability resolution through a port is what lets
//! `tau-runtime-core::Runtime` drive the agent loop without linking
//! `tau-pkg` (which is std-only).

use alloc::string::String;
use alloc::vec::Vec;

use tau_domain::Capability;

use crate::DenyEntry;

/// Resolved capability grant set after applying project-level overrides
/// to a package manifest's declared capabilities.
///
/// Produced by [`CapabilityResolver::resolve`] and stuffed into
/// `RunOptions` by the host shell; the kernel reads it directly when
/// dispatching tool calls.
///
/// Three views, one per consumer:
/// - `for_kernel` — the un-narrowed source capabilities; the kernel's
///   structural capability check (`fs.read ⊇ fs.read`) reads these.
/// - `for_session` — the narrowed views handed to plugins via
///   `SessionContext.granted_capabilities`. Same shape as `for_kernel`
///   but with allow-lists narrowed to the project override.
/// - `deny_entries` — explicit deny carve-outs, one entry per
///   capability kind with a non-empty deny list.
#[derive(Debug, Clone)]
pub struct ResolvedGrants {
    /// Un-narrowed source capabilities — fed into the kernel's
    /// structural capability check.
    pub for_kernel: Vec<Capability>,
    /// Narrowed views handed to plugins via session context.
    pub for_session: Vec<Capability>,
    /// Per-kind deny carve-outs.
    pub deny_entries: Vec<DenyEntry>,
}

/// Resolve a package manifest's declared capabilities into the
/// effective grant set for an agent run.
///
/// Host shells implement this against their override system:
/// - tau-runtime ships `TauPkgCapabilityResolver` wrapping
///   `tau_pkg::capability_override::compute_effective`.
/// - Embassy / wasm shells with no override layer can ship a noop impl
///   that maps the input through to `for_kernel = for_session =
///   manifest_capabilities; deny_entries = []`.
///
/// Errors are reported as [`CapabilityResolverError`] (kind + reason)
/// so the kernel can surface them as `CapabilityOverrideExpands` without
/// depending on the host's concrete error type.
pub trait CapabilityResolver: Send + Sync {
    /// Compute the post-override grant set for a run.
    fn resolve(
        &self,
        manifest_capabilities: &[Capability],
    ) -> Result<ResolvedGrants, CapabilityResolverError>;
}

/// Error returned by [`CapabilityResolver::resolve`] when an override
/// would expand the package manifest's grant set (forbidden by the
/// subset law).
///
/// `kind` is the capability discriminator (e.g. `"fs.read"`); `reason`
/// is a human-readable explanation suitable for the runtime error's
/// `Display` impl.
#[derive(Debug, Clone)]
pub struct CapabilityResolverError {
    /// Capability kind that triggered the rejection (`fs.read`,
    /// `fs.write`, `fs.exec`, `net.http`, `process.spawn`, ...).
    pub kind: String,
    /// Human-readable reason — e.g. the offending glob and the
    /// override path.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;

    struct PassthroughResolver;

    impl CapabilityResolver for PassthroughResolver {
        fn resolve(
            &self,
            manifest_capabilities: &[Capability],
        ) -> Result<ResolvedGrants, CapabilityResolverError> {
            Ok(ResolvedGrants {
                for_kernel: manifest_capabilities.to_vec(),
                for_session: manifest_capabilities.to_vec(),
                deny_entries: vec![],
            })
        }
    }

    #[test]
    fn passthrough_resolver_round_trips() {
        let r: Arc<dyn CapabilityResolver> = Arc::new(PassthroughResolver);
        let resolved = r.resolve(&[]).expect("resolve");
        assert!(resolved.for_kernel.is_empty());
        assert!(resolved.for_session.is_empty());
        assert!(resolved.deny_entries.is_empty());
    }
}
