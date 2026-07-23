//! Capability gate port — the `tau_ports::CapabilityGate` trait + supporting types.
//!
//! Hexagonal port: `tau-runtime` consumes this trait; `tau-sandbox-native`,
//! `tau-sandbox-container`, and `MockCapabilityGate` (in [`crate::fixtures`])
//! implement it. The runtime selects an adapter via a probe-based chain
//! configured in `<scope>/.tau/config.toml`.
//!
//! Stable as of v0.1 of the sandboxing sub-project. Variant evolution is
//! handled by `#[non_exhaustive]` on every public type.

#[cfg(feature = "process")]
pub mod dyn_process;
#[cfg(feature = "process")]
pub mod passthrough;
#[cfg(feature = "process")]
pub mod process;

use alloc::collections::BTreeMap;

use tau_domain::{Capability, CapabilityShapeSet};

use crate::error::CapabilityError;

/// Plan provided to [`crate::ProcessCapabilityGate::wrap_spawn`].
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CapabilityPlan {
    /// Capabilities the sandboxed code is allowed to exercise. The runtime
    /// composes this from the package's `compute_effective` capability set
    /// before calling `wrap_spawn`.
    pub capabilities: alloc::vec::Vec<Capability>,
    /// Optional working-context hint (working dir + env).
    pub context: Option<WorkingContext>,
    /// Optional resource limits.
    pub limits: Option<ResourceLimits>,
}

impl CapabilityPlan {
    /// Construct a [`CapabilityPlan`].
    ///
    /// `#[non_exhaustive]` blocks struct-literal construction outside
    /// `tau-ports`; use this constructor instead.
    pub fn new(
        capabilities: alloc::vec::Vec<Capability>,
        context: Option<WorkingContext>,
        limits: Option<ResourceLimits>,
    ) -> Self {
        Self {
            capabilities,
            context,
            limits,
        }
    }
}

/// Working-context hint for the gated execution.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkingContext {
    /// Working directory hint. Only meaningful when the `process` feature
    /// is enabled (no_std hosts have no filesystem path semantics).
    #[cfg(feature = "process")]
    pub working_dir: Option<std::path::PathBuf>,
    /// Environment variables to seed the gated execution.
    pub env: BTreeMap<alloc::string::String, alloc::string::String>,
}

/// Resource limits for the gated execution.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResourceLimits {
    /// Maximum memory, in bytes.
    pub memory_bytes: Option<u64>,
    /// Maximum CPU time, in seconds.
    pub cpu_seconds: Option<u32>,
    /// Maximum wall-clock time, in seconds.
    pub wall_clock_seconds: Option<u32>,
    /// Maximum concurrent subprocesses.
    pub max_subprocesses: Option<u32>,
}

/// Probe result describing an adapter's runtime availability.
#[non_exhaustive]
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CapabilityProbe {
    /// Adapter is usable on this host with the indicated tier.
    Available {
        /// Best tier the adapter can guarantee right now.
        tier: CapabilityTier,
        /// Free-form diagnostic ("landlock V1; seccomp BPF; user_ns ok").
        details: alloc::string::String,
    },
    /// Adapter is not usable on this host.
    Unavailable {
        /// Human-readable reason ("kernel < 5.13", "no docker on PATH").
        reason: alloc::string::String,
    },
}

/// Enforcement tier an adapter can deliver. Forms a total order: `None` <
/// `Light` < `Strict`. Higher tiers are stricter; project config can RAISE
/// but never WEAKEN the tier the adapter advertises.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CapabilityTier {
    /// No enforcement (only valid for the mock adapter).
    None,
    /// Filesystem isolation only (e.g. landlock without seccomp).
    Light,
    /// Filesystem + syscall + namespace isolation (full Strict tier).
    Strict,
}

/// Opaque handle returned by [`crate::ProcessCapabilityGate::wrap_spawn`]. Drops automatically
/// release any resources the adapter holds (e.g. cgroup, namespace fd).
///
/// `nested`: drop guards that run LIFO before the main cleanup closure.
/// NativeSandbox uses this to nest a proxy task guard whose Drop signals
/// the proxy to shut down.
#[non_exhaustive]
pub struct CapabilityHandle {
    cleanup: Option<alloc::boxed::Box<dyn FnOnce() + Send + 'static>>,
    nested: alloc::vec::Vec<alloc::boxed::Box<dyn Send>>,
}

impl CapabilityHandle {
    /// Construct a handle from an adapter-defined cleanup closure.
    /// The closure runs exactly once when the handle is dropped.
    pub fn new<F: FnOnce() + Send + 'static>(cleanup: F) -> Self {
        Self {
            cleanup: Some(alloc::boxed::Box::new(cleanup)),
            nested: alloc::vec::Vec::new(),
        }
    }

    /// A handle that releases nothing (mock / no-op).
    pub fn noop() -> Self {
        Self {
            cleanup: None,
            nested: alloc::vec::Vec::new(),
        }
    }

    /// Add a drop guard nested inside this handle's lifetime.
    ///
    /// Drop order: nested guards drop LIFO (latest-attached drops first)
    /// before the main cleanup closure. NativeSandbox uses this to nest
    /// a proxy task guard whose Drop signals the proxy to shut down.
    pub fn nest_handle(&mut self, guard: alloc::boxed::Box<dyn Send>) {
        self.nested.push(guard);
    }
}

impl Drop for CapabilityHandle {
    fn drop(&mut self) {
        // Drop nested guards LIFO (latest-attached drops first).
        for guard in self.nested.drain(..).rev() {
            drop(guard);
        }

        // Run main cleanup closure.
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

impl core::fmt::Debug for CapabilityHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CapabilityHandle").finish_non_exhaustive()
    }
}

/// Trait implemented by capability gate adapters. The runtime calls these
/// methods in this order:
///
/// 1. [`CapabilityGate::probe`] at startup (cached) — discover what the adapter can do.
/// 2. [`CapabilityGate::supported_shapes`] for static cross-checks.
/// 3. [`CapabilityGate::validate_plan`] before spawning a plugin process.
///
/// Process-flavored methods (`wrap_spawn`, `apply_post_spawn`) are on the
/// `ProcessCapabilityGate` extension trait (Task 1.3).
#[allow(async_fn_in_trait)]
pub trait CapabilityGate: Send + Sync {
    /// Plugin-visible name (matches the package name; for diagnostics).
    fn name(&self) -> &str;

    /// Probe the host for adapter availability. Cached by the runtime.
    async fn probe(&self) -> CapabilityProbe;

    /// Capability shapes this adapter can enforce. Used at install time
    /// (Layer 2) and at `tau check` time (Layer 3) to refuse plans this
    /// adapter cannot honor.
    fn supported_shapes(&self) -> CapabilityShapeSet;

    /// Validate that this plan can be executed by this adapter.
    /// Returns `Err(CapabilityError::ShapeUnsupported)` if any required shape
    /// is not in [`CapabilityGate::supported_shapes`].
    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn nest_handle_drops_in_lifo_order() {
        use std::sync::{Arc, Mutex};

        let order: Arc<Mutex<alloc::vec::Vec<&'static str>>> =
            Arc::new(Mutex::new(alloc::vec::Vec::new()));
        let order_main = Arc::clone(&order);

        let mut handle = CapabilityHandle::new(move || {
            order_main.lock().unwrap().push("main_cleanup");
        });

        // Add 2 nested guards. Each pushes its label on Drop.
        struct Guard(Arc<Mutex<alloc::vec::Vec<&'static str>>>, &'static str);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.lock().unwrap().push(self.1);
            }
        }
        handle.nest_handle(alloc::boxed::Box::new(Guard(
            Arc::clone(&order),
            "first_nested",
        )));
        handle.nest_handle(alloc::boxed::Box::new(Guard(
            Arc::clone(&order),
            "second_nested",
        )));

        drop(handle);

        // Expected order: LIFO of nested, then main cleanup.
        assert_eq!(
            *order.lock().unwrap(),
            vec!["second_nested", "first_nested", "main_cleanup"]
        );
    }
}
