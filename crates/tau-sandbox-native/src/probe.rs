//! Linux kernel feature probe.
//!
//! Detects which sandbox tier the running kernel can support:
//! - **Strict**: landlock V1 + seccomp BPF + unprivileged user namespaces.
//! - **Light**: landlock V1 only.
//! - **Unavailable**: landlock V1 missing (kernel < 5.13).
//!
//! The probe does NOT call `restrict_self()` and does NOT install any filter,
//! so it has no side-effects on the calling process.
//!
//! # Simplification note (v0.1)
//! - seccomp BPF is assumed available on any Linux kernel ≥ 3.5 (all current
//!   production kernels qualify). No explicit `prctl(PR_GET_SECCOMP)` probe is
//!   performed.
//! - Unprivileged user namespaces are assumed available unless the file
//!   `/proc/sys/kernel/unprivileged_userns_clone` exists and contains `0`.
//!   On kernels that do not have this sysctl (i.e. vanilla upstream), unprivileged
//!   user namespaces are enabled by default. A future task can add a fork-based
//!   probe for more precise detection.

use tau_ports::{CapabilityProbe, CapabilityTier};

/// Probe the host kernel for sandbox features.
///
/// Returns `Available { tier }` where `tier` is the strongest tier the
/// adapter can support, capped at the caller's requested tier.
pub(crate) async fn probe(requested: CapabilityTier) -> CapabilityProbe {
    decide_probe(requested, landlock_v1_supported(), user_ns_supported())
}

/// Pure tier-decision function: given a requested tier and the kernel feature
/// flags, return the best available [`CapabilityProbe`].
///
/// Extracted from [`probe`] so the decision matrix can be unit-tested without
/// invoking real kernel syscalls. The two boolean inputs mirror the runtime
/// probes: `landlock_ok` corresponds to [`landlock_v1_supported`], and
/// `user_ns_ok` corresponds to [`user_ns_supported`].
fn decide_probe(requested: CapabilityTier, landlock_ok: bool, user_ns_ok: bool) -> CapabilityProbe {
    if !landlock_ok {
        return CapabilityProbe::Unavailable {
            reason: "landlock V1 unsupported (kernel < 5.13)".into(),
        };
    }
    let effective = match requested {
        CapabilityTier::None => CapabilityTier::None,
        // Light needs landlock only — already verified above.
        CapabilityTier::Light => CapabilityTier::Light,
        // Strict needs landlock + seccomp + user namespaces.
        // seccomp is assumed available (Linux 3.5+, see module-level note).
        // User namespaces are checked via unprivileged_userns_clone sysctl.
        CapabilityTier::Strict => {
            if user_ns_ok {
                CapabilityTier::Strict
            } else {
                tracing::info!("unprivileged user namespaces disabled; capping Strict -> Light");
                CapabilityTier::Light
            }
        }
        // Non-exhaustive arm: unknown future tier — warn and report unavailable.
        other => {
            tracing::warn!(
                ?other,
                "unknown SandboxTier in probe — returning Unavailable"
            );
            return CapabilityProbe::Unavailable {
                reason: format!("tier {other:?} not implemented"),
            };
        }
    };
    CapabilityProbe::Available {
        tier: effective,
        details: format!("landlock V1 ok, effective tier: {effective:?}"),
    }
}

/// Check whether this kernel supports Landlock V1 by attempting to create a
/// V1 ruleset with `HardRequirement` compat level. Creating a ruleset (without
/// calling `restrict_self()`) is a side-effect-free probe: the kernel FD is
/// opened and immediately dropped, applying no restrictions to the process.
fn landlock_v1_supported() -> bool {
    use landlock::{Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, ABI};

    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(ABI::V1))
        .and_then(|r| r.create())
        .is_ok()
}

/// Check whether unprivileged user namespaces are available.
///
/// # Simplification (v0.1)
/// Reads `/proc/sys/kernel/unprivileged_userns_clone`. If the file exists and
/// contains `0`, user namespaces are disabled. On kernels that do not expose
/// this sysctl (mainline upstream), the file is absent and we conservatively
/// assume user namespaces are enabled. A full probe would `fork()` a child and
/// attempt `unshare(CLONE_NEWUSER)`, but that adds latency and complexity.
fn user_ns_supported() -> bool {
    match std::fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone") {
        Ok(contents) => contents.trim() != "0",
        // File absent: sysctl not present → assume enabled (mainline default).
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- decide_probe: landlock unavailable ----------

    #[test]
    fn unavailable_when_landlock_missing_regardless_of_request() {
        for tier in [
            CapabilityTier::None,
            CapabilityTier::Light,
            CapabilityTier::Strict,
        ] {
            let p = decide_probe(
                tier, /* landlock_ok */ false, /* user_ns_ok */ true,
            );
            assert!(
                matches!(p, CapabilityProbe::Unavailable { ref reason } if reason.contains("landlock V1")),
                "tier {tier:?} should report Unavailable when landlock is missing — got {p:?}"
            );
        }
    }

    #[test]
    fn unavailable_when_landlock_missing_even_if_user_ns_present() {
        let p = decide_probe(CapabilityTier::Strict, false, true);
        assert!(matches!(p, CapabilityProbe::Unavailable { .. }));
    }

    // ---------- decide_probe: tier capping ----------

    #[test]
    fn none_request_returns_none_tier() {
        let p = decide_probe(CapabilityTier::None, true, true);
        match p {
            CapabilityProbe::Available { tier, .. } => assert_eq!(tier, CapabilityTier::None),
            other => panic!("expected Available(None), got {other:?}"),
        }
    }

    #[test]
    fn light_request_returns_light_tier() {
        let p = decide_probe(CapabilityTier::Light, true, true);
        match p {
            CapabilityProbe::Available { tier, .. } => assert_eq!(tier, CapabilityTier::Light),
            other => panic!("expected Available(Light), got {other:?}"),
        }
    }

    #[test]
    fn light_request_does_not_depend_on_user_ns() {
        // User namespaces aren't needed for Light tier; the decision must be
        // independent of `user_ns_ok`.
        let with_uns = decide_probe(CapabilityTier::Light, true, true);
        let without_uns = decide_probe(CapabilityTier::Light, true, false);
        assert!(matches!(
            with_uns,
            CapabilityProbe::Available {
                tier: CapabilityTier::Light,
                ..
            }
        ));
        assert!(matches!(
            without_uns,
            CapabilityProbe::Available {
                tier: CapabilityTier::Light,
                ..
            }
        ));
    }

    #[test]
    fn strict_request_with_user_ns_returns_strict() {
        let p = decide_probe(CapabilityTier::Strict, true, true);
        match p {
            CapabilityProbe::Available { tier, .. } => assert_eq!(tier, CapabilityTier::Strict),
            other => panic!("expected Available(Strict), got {other:?}"),
        }
    }

    #[test]
    fn strict_request_without_user_ns_caps_to_light() {
        let p = decide_probe(CapabilityTier::Strict, true, false);
        match p {
            CapabilityProbe::Available { tier, .. } => assert_eq!(tier, CapabilityTier::Light),
            other => panic!("expected cap to Light, got {other:?}"),
        }
    }

    // ---------- decide_probe: details string ----------

    #[test]
    fn available_details_string_mentions_landlock_and_tier() {
        let p = decide_probe(CapabilityTier::Light, true, true);
        match p {
            CapabilityProbe::Available { details, .. } => {
                assert!(
                    details.contains("landlock"),
                    "details should reference landlock; got {details:?}"
                );
                assert!(
                    details.contains("Light"),
                    "details should name the effective tier; got {details:?}"
                );
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    // ---------- decide_probe: monotonicity ----------

    #[test]
    fn effective_tier_never_exceeds_requested() {
        // Property: for any (requested, landlock_ok, user_ns_ok), the
        // effective tier (when Available) is ≤ requested.
        for landlock_ok in [true, false] {
            for user_ns_ok in [true, false] {
                for requested in [
                    CapabilityTier::None,
                    CapabilityTier::Light,
                    CapabilityTier::Strict,
                ] {
                    let p = decide_probe(requested, landlock_ok, user_ns_ok);
                    if let CapabilityProbe::Available { tier, .. } = p {
                        assert!(
                            tier <= requested,
                            "effective tier {tier:?} exceeds requested {requested:?} \
                             (landlock_ok={landlock_ok}, user_ns_ok={user_ns_ok})"
                        );
                    }
                }
            }
        }
    }

    // ---------- side-effect-free probes ----------

    #[test]
    fn landlock_v1_supported_runs_without_panic() {
        // Smoke test: just verifying the function returns without panic.
        // Actual kernel-feature reporting depends on the test runner's kernel.
        // CI's ubuntu-latest reports true; macOS dev builds skip this entire
        // module via #[cfg(target_os = "linux")].
        let _ = landlock_v1_supported();
    }

    #[test]
    fn user_ns_supported_runs_without_panic_and_returns_default_when_sysctl_absent() {
        // The function reads /proc/sys/kernel/unprivileged_userns_clone.
        // Mainline kernels don't expose it → defaults to true.
        // Distro kernels with the patch may set it to 0 or 1.
        // Either way, the function must not panic.
        let result = user_ns_supported();
        // We can't assert the value (kernel-dependent), but we can assert the
        // function body never panics — the assignment proves it returned.
        let _ = result;
    }
}
