//! Per-target durability resolution (EPIC 6.1).
//!
//! The IR carries a [`Durability`] *intent* (or an explicit form). The host
//! resolves it to a concrete `(checkpoint, store)` for a given
//! [`TargetTriple`] at run time (`ir_dispatcher`) and at build/check time
//! (`tau check --target`). Keeping resolution here — the one `no_std` crate
//! that sees both `tau_ir::durable` and `tau_ports::target` — means `tau
//! check` prints exactly what the runtime will do (the transparency bar).

use alloc::string::{String, ToString};
use tau_ir::durable::{CheckpointGranularity, Durability, DurabilityIntent, DurableStore};
use tau_ports::target::TargetTriple;

/// Whether a target can honor a requested durability.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    /// The target provides the resolved granularity + store.
    Honored,
    /// The target cannot durably provide the requested store. `tau check
    /// --target` reports Error; the runtime refuses to start the run.
    Unsupported {
        /// Static, human-readable reason.
        reason: &'static str,
    },
}

/// Concrete durability resolved for a specific target (EPIC 6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDurability {
    /// Resolved checkpoint granularity.
    pub checkpoint: CheckpointGranularity,
    /// Resolved durable store.
    pub store: DurableStore,
    /// Whether the target honors the request.
    pub support: Support,
    /// `Some(..)` when the author used an intent; `None` for an explicit form.
    pub from_intent: Option<DurabilityIntent>,
}

/// Error returned by [`ResolvedDurability::require_supported`].
#[derive(Debug, Clone)]
pub struct DurabilityUnsupported {
    /// Why the target cannot honor the request.
    pub reason: &'static str,
    /// The target that could not honor it (for the error message).
    pub target: String,
}

impl core::fmt::Display for DurabilityUnsupported {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "target `{}` cannot honor the requested durability: {}",
            self.target, self.reason
        )
    }
}

impl ResolvedDurability {
    /// Convert an `Unsupported` resolution into an error; pass `Honored`
    /// through. Used by the host to refuse a run it cannot make durable, and
    /// by `tau check` to fail the build.
    pub fn require_supported(self, target: &TargetTriple) -> Result<Self, DurabilityUnsupported> {
        match &self.support {
            Support::Honored => Ok(self),
            Support::Unsupported { reason } => Err(DurabilityUnsupported {
                reason,
                target: target.to_string(),
            }),
        }
    }
}

/// Resolve a [`Durability`] against a target.
///
/// - `Explicit { checkpoint, store }` resolves to itself; `support` checks the
///   target can provide `store`.
/// - `Intent(SurviveRestarts)` maps to the coarsest checkpoint + store the
///   target durably provides.
///
/// A-minimal policy: every triple in the target registry provides the `File`
/// store (host filesystem or host-mediated wasi preopen), so all registered
/// targets honor `survive-restarts` → `PerTurn + File`. This includes
/// `Reserved` entries (e.g. `windows-native-strict`): `tau_ports::target::lookup`
/// finds them and they count as present/Honored even though no shipping adapter
/// exists yet. Any triple *absent* from the registry (not found by `lookup`)
/// has no shipping store and is `Unsupported`. The policy diverges the moment
/// a `Kv` store or a no-persistence target lands.
pub fn resolve_durability(d: &Durability, target: &TargetTriple) -> ResolvedDurability {
    let provides_file = target_provides_file(target);
    match d {
        Durability::Intent(intent) => ResolvedDurability {
            checkpoint: CheckpointGranularity::PerTurn,
            store: DurableStore::File,
            support: if provides_file {
                Support::Honored
            } else {
                Support::Unsupported {
                    reason: "target has no durable file store for survive-restarts",
                }
            },
            from_intent: Some(*intent),
        },
        Durability::Explicit { checkpoint, store } => ResolvedDurability {
            checkpoint: *checkpoint,
            store: *store,
            support: match store {
                DurableStore::File if provides_file => Support::Honored,
                _ => Support::Unsupported {
                    reason: "target has no durable file store",
                },
            },
            from_intent: None,
        },
        _ => ResolvedDurability {
            checkpoint: CheckpointGranularity::PerTurn,
            store: DurableStore::File,
            support: Support::Unsupported {
                reason: "unknown durability variant",
            },
            from_intent: None,
        },
    }
}

/// A-minimal: a target provides the `File` store iff it is a registered triple.
fn target_provides_file(target: &TargetTriple) -> bool {
    tau_ports::target::lookup(target).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn honored_for(t: &TargetTriple) {
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let r = resolve_durability(&d, t);
        assert_eq!(r.checkpoint, CheckpointGranularity::PerTurn);
        assert_eq!(r.store, DurableStore::File);
        assert!(matches!(r.support, Support::Honored), "target {t} should honor");
        assert_eq!(r.from_intent, Some(DurabilityIntent::SurviveRestarts));
    }

    #[test]
    fn every_registered_target_honors_survive_restarts() {
        for entry in tau_ports::target::list_all() {
            honored_for(&entry.triple);
        }
    }

    #[test]
    fn explicit_resolves_to_itself_on_a_registered_target() {
        let d = Durability::Explicit {
            checkpoint: CheckpointGranularity::PerToolCall,
            store: DurableStore::File,
        };
        let r = resolve_durability(&d, &TargetTriple::PASSTHROUGH);
        assert_eq!(r.checkpoint, CheckpointGranularity::PerToolCall);
        assert_eq!(r.store, DurableStore::File);
        assert!(matches!(r.support, Support::Honored));
        assert_eq!(r.from_intent, None);
    }

    #[test]
    fn unregistered_target_is_unsupported_and_require_errs() {
        use tau_ports::capability_gate::CapabilityTier;
        use tau_ports::target::adapter_family::AdapterFamily;
        use tau_ports::target::platform::Platform;
        // A triple not present in the registry (no shipping store).
        let off = TargetTriple {
            platform: Platform::Windows,
            adapter_family: AdapterFamily::Wasi,
            tier: CapabilityTier::None,
        };
        let d = Durability::Intent(DurabilityIntent::SurviveRestarts);
        let r = resolve_durability(&d, &off);
        assert!(matches!(r.support, Support::Unsupported { .. }));
        assert_eq!(r.from_intent, Some(DurabilityIntent::SurviveRestarts));
        assert!(r.clone().require_supported(&off).is_err());
    }
}
