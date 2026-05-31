//! Declarative-requirements adapter resolver.
//!
//! [`resolve_adapter`] implements the Bazel-style filter pipeline from spec §3:
//!
//! 1. **Platform filter** — only consider adapters that apply to the current OS.
//! 2. **Probe filter** — instantiate the adapter and call `probe()`. Adapters
//!    returning [`tau_ports::CapabilityProbe::Unavailable`] are rejected.
//! 3. **Tier filter** — the adapter's delivered tier must be ≥ the effective
//!    required tier (max of project tier and plugin-floor tier).
//! 4. **Shape filter** — every shape in `requirements.required_shapes` must be
//!    in the adapter's `supported_shapes`.
//! 5. **Per-plugin tier filter** — every plugin's `required_tier` must be ≤
//!    the adapter's delivered tier.
//! 6. **Priority sort** — among survivors, pick the one with the highest
//!    [`AdapterRegistration::priority`].
//!
//! Mock injection via `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` bypasses the registry
//! entirely; it is an opt-in, never a silent fallback.

use std::cmp::Reverse;
use std::process::Command;

use tau_domain::CapabilityShapeSet;
use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityTier, ProcessCapabilityGate,
};
use tau_sandbox_container::{ContainerRuntime, ContainerSandbox};
#[cfg(target_os = "macos")]
use tau_sandbox_darwin::DarwinSandbox;
use tau_sandbox_native::NativeSandbox;
#[cfg(target_os = "windows")]
use tau_sandbox_windows::WindowsSandbox;

use crate::process_gate::passthrough::PassthroughSandbox;
use crate::process_gate::registry::{detect_platform, AdapterRegistration, RegistryKind, REGISTRY};
use crate::process_gate::resolution_error::{ResolutionError, ResolutionRejection};

// ---------------------------------------------------------------------------
// SandboxAdapter enum
// ---------------------------------------------------------------------------

/// A concrete sandbox adapter selected by the resolver.
///
/// Enum dispatch — the `Sandbox` trait uses `async fn` in trait (AFIT) which
/// is not dyn-compatible, so we use an explicit enum rather than `Arc<dyn Sandbox>`.
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum SandboxAdapter {
    /// `tau-sandbox-native` Linux adapter.
    Native(NativeSandbox),
    /// `tau-sandbox-darwin` macOS adapter (sandbox-exec + tau-sandbox-proxy).
    /// Selected for `RegistryKind::Native` on macOS hosts.
    #[cfg(target_os = "macos")]
    Darwin(DarwinSandbox),
    /// `tau-sandbox-windows` Windows adapter (AppContainer + tau-sandbox-proxy).
    /// Selected for `RegistryKind::Native` on Windows hosts.
    #[cfg(target_os = "windows")]
    Windows(WindowsSandbox),
    /// `tau-sandbox-container` docker/podman adapter.
    Container(ContainerSandbox),
    /// `tau_ports::fixtures::MockCapabilityGate` — available in all builds but only
    /// instantiable during `cargo test`, when the `test-fixtures` feature is
    /// enabled, or when `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` is set.
    Mock(tau_ports::fixtures::MockCapabilityGate),
    /// No isolation; explicit opt-out path.
    Passthrough(PassthroughSandbox),
}

impl std::fmt::Debug for SandboxAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxAdapter::Native(_) => f.debug_tuple("SandboxAdapter::Native").finish(),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(_) => f.debug_tuple("SandboxAdapter::Darwin").finish(),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(_) => f.debug_tuple("SandboxAdapter::Windows").finish(),
            SandboxAdapter::Container(_) => f.debug_tuple("SandboxAdapter::Container").finish(),
            SandboxAdapter::Mock(_) => f.debug_tuple("SandboxAdapter::Mock").finish(),
            SandboxAdapter::Passthrough(_) => f.debug_tuple("SandboxAdapter::Passthrough").finish(),
        }
    }
}

impl SandboxAdapter {
    /// Returns the adapter's name.
    ///
    /// # Example
    ///
    /// ```
    /// use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;
    /// use tau_runtime_tokio::process_gate::resolver::SandboxAdapter;
    ///
    /// let adapter = SandboxAdapter::Passthrough(PassthroughSandbox::new());
    /// assert_eq!(adapter.name(), "passthrough");
    /// ```
    pub fn name(&self) -> &str {
        match self {
            SandboxAdapter::Native(a) => a.name(),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.name(),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.name(),
            SandboxAdapter::Container(a) => a.name(),
            SandboxAdapter::Mock(a) => a.name(),
            SandboxAdapter::Passthrough(a) => a.name(),
        }
    }

    /// Probe the adapter for availability.
    pub async fn probe(&self) -> CapabilityProbe {
        match self {
            SandboxAdapter::Native(a) => a.probe().await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.probe().await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.probe().await,
            SandboxAdapter::Container(a) => a.probe().await,
            SandboxAdapter::Mock(a) => a.probe().await,
            SandboxAdapter::Passthrough(a) => a.probe().await,
        }
    }

    /// Returns capability shapes this adapter can enforce.
    pub fn supported_shapes(&self) -> CapabilityShapeSet {
        match self {
            SandboxAdapter::Native(a) => a.supported_shapes(),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.supported_shapes(),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.supported_shapes(),
            SandboxAdapter::Container(a) => a.supported_shapes(),
            SandboxAdapter::Mock(a) => a.supported_shapes(),
            SandboxAdapter::Passthrough(a) => a.supported_shapes(),
        }
    }

    /// Validate that the given plan can be executed by this adapter.
    pub fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        match self {
            SandboxAdapter::Native(a) => a.validate_plan(plan),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.validate_plan(plan),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.validate_plan(plan),
            SandboxAdapter::Container(a) => a.validate_plan(plan),
            SandboxAdapter::Mock(a) => a.validate_plan(plan),
            SandboxAdapter::Passthrough(a) => a.validate_plan(plan),
        }
    }

    /// Apply sandbox enforcement to a [`Command`] in preparation for spawn.
    pub async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        match self {
            SandboxAdapter::Native(a) => a.wrap_spawn(plan, cmd).await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.wrap_spawn(plan, cmd).await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Container(a) => a.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Mock(a) => a.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Passthrough(a) => a.wrap_spawn(plan, cmd).await,
        }
    }

    /// Post-spawn sandbox configuration (e.g. per-host network filtering for
    /// the strict tier). Called after `cmd.spawn()` while the child is blocked
    /// on the sync-pipe. Default: no-op.
    pub async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        match self {
            SandboxAdapter::Native(a) => a.apply_post_spawn(plan, child_pid, handle).await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(a) => a.apply_post_spawn(plan, child_pid, handle).await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(a) => a.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Container(a) => a.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Mock(a) => a.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Passthrough(a) => a.apply_post_spawn(plan, child_pid, handle).await,
        }
    }
}

impl CapabilityGate for SandboxAdapter {
    fn name(&self) -> &str {
        match self {
            SandboxAdapter::Native(s) => s.name(),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.name(),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.name(),
            SandboxAdapter::Container(s) => s.name(),
            SandboxAdapter::Mock(s) => s.name(),
            SandboxAdapter::Passthrough(s) => s.name(),
        }
    }

    async fn probe(&self) -> CapabilityProbe {
        match self {
            SandboxAdapter::Native(s) => s.probe().await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.probe().await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.probe().await,
            SandboxAdapter::Container(s) => s.probe().await,
            SandboxAdapter::Mock(s) => s.probe().await,
            SandboxAdapter::Passthrough(s) => s.probe().await,
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        match self {
            SandboxAdapter::Native(s) => s.supported_shapes(),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.supported_shapes(),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.supported_shapes(),
            SandboxAdapter::Container(s) => s.supported_shapes(),
            SandboxAdapter::Mock(s) => s.supported_shapes(),
            SandboxAdapter::Passthrough(s) => s.supported_shapes(),
        }
    }

    fn validate_plan(&self, plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        match self {
            SandboxAdapter::Native(s) => s.validate_plan(plan),
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.validate_plan(plan),
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.validate_plan(plan),
            SandboxAdapter::Container(s) => s.validate_plan(plan),
            SandboxAdapter::Mock(s) => s.validate_plan(plan),
            SandboxAdapter::Passthrough(s) => s.validate_plan(plan),
        }
    }
}

impl ProcessCapabilityGate for SandboxAdapter {
    async fn wrap_spawn(
        &self,
        plan: &CapabilityPlan,
        cmd: &mut Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        match self {
            SandboxAdapter::Native(s) => s.wrap_spawn(plan, cmd).await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.wrap_spawn(plan, cmd).await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Container(s) => s.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Mock(s) => s.wrap_spawn(plan, cmd).await,
            SandboxAdapter::Passthrough(s) => s.wrap_spawn(plan, cmd).await,
        }
    }

    async fn apply_post_spawn(
        &self,
        plan: &CapabilityPlan,
        child_pid: i32,
        handle: &mut CapabilityHandle,
    ) -> Result<(), CapabilityError> {
        match self {
            SandboxAdapter::Native(s) => s.apply_post_spawn(plan, child_pid, handle).await,
            #[cfg(target_os = "macos")]
            SandboxAdapter::Darwin(s) => s.apply_post_spawn(plan, child_pid, handle).await,
            #[cfg(target_os = "windows")]
            SandboxAdapter::Windows(s) => s.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Container(s) => s.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Mock(s) => s.apply_post_spawn(plan, child_pid, handle).await,
            SandboxAdapter::Passthrough(s) => s.apply_post_spawn(plan, child_pid, handle).await,
        }
    }
}

// ---------------------------------------------------------------------------
// instantiate helper
// ---------------------------------------------------------------------------

/// Public wrapper for `instantiate` — used by `tau sandbox status` to
/// probe each registered adapter regardless of platform compatibility.
pub fn instantiate_for_probe(kind: RegistryKind) -> Result<SandboxAdapter, String> {
    instantiate(kind)
}

fn instantiate(kind: RegistryKind) -> Result<SandboxAdapter, String> {
    // The catch-all arm exists for forward-compat: when a new RegistryKind
    // variant is added before the match arms are updated, the compiler will
    // continue to compile (since RegistryKind is #[non_exhaustive] externally).
    // Within the same crate the pattern is currently unreachable, hence the allow.
    #[allow(unreachable_patterns)]
    match kind {
        #[cfg(target_os = "macos")]
        RegistryKind::Native => Ok(SandboxAdapter::Darwin(DarwinSandbox::new("native"))),
        #[cfg(target_os = "windows")]
        RegistryKind::Native => Ok(SandboxAdapter::Windows(WindowsSandbox::new("native"))),
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        RegistryKind::Native => Ok(SandboxAdapter::Native(NativeSandbox::new(
            "native",
            CapabilityTier::Strict,
        ))),
        RegistryKind::Container => Ok(SandboxAdapter::Container(ContainerSandbox::new(
            "container",
            ContainerRuntime::Auto,
        ))),
        RegistryKind::Remote => {
            Err("remote backend not implemented at v0.2 (Phase 2 sub-project F)".into())
        }
        RegistryKind::Passthrough => Ok(SandboxAdapter::Passthrough(PassthroughSandbox::new())),
        // catch-all for #[non_exhaustive]
        other => Err(format!("unknown adapter kind: {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// resolve_adapter — the filter pipeline
// ---------------------------------------------------------------------------

/// Select the best sandbox adapter for the given project requirements and
/// plugin list.
///
/// Implements the Bazel-style filter pipeline from spec §3:
/// platform → probe → tier → shape → plugin-tier-floor → priority sort.
///
/// # Mock injection
///
/// When `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` is set, the registry is bypassed
/// entirely and `SandboxAdapter::Mock` is returned. This is the only way to
/// get a Mock adapter; it is never a silent fallback.
pub async fn resolve_adapter(
    requirements: &tau_pkg::scope::SandboxRequirements,
    plugins: &[tau_domain::PluginSandboxRequirements],
) -> Result<SandboxAdapter, ResolutionError> {
    use tau_pkg::scope::SandboxRequiredTier;

    // Mock injection via env var — bypasses the registry entirely.
    if std::env::var("TAU_TESTING_ALLOW_MOCK_SANDBOX")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(SandboxAdapter::Mock(
            tau_ports::fixtures::MockCapabilityGate::new("mock"),
        ));
    }

    let platform = detect_platform();

    // Map config-side tier types to tau_ports::CapabilityTier
    let project_tier: CapabilityTier = match requirements.required_tier {
        SandboxRequiredTier::None => CapabilityTier::None,
        SandboxRequiredTier::Light => CapabilityTier::Light,
        SandboxRequiredTier::Strict => CapabilityTier::Strict,
        // catch-all for #[non_exhaustive] forward-compat
        other => {
            return Err(ResolutionError::ConfigError {
                message: format!("unknown required_tier variant: {other:?}"),
            })
        }
    };

    // Plugin floor: highest required_tier across all plugins.
    let plugin_floor: CapabilityTier = plugins
        .iter()
        .filter_map(|p| p.required_tier.as_ref())
        .map(|t| match t {
            tau_domain::PluginRequiredTier::None => CapabilityTier::None,
            tau_domain::PluginRequiredTier::Light => CapabilityTier::Light,
            tau_domain::PluginRequiredTier::Strict => CapabilityTier::Strict,
            _ => CapabilityTier::None,
        })
        .max()
        .unwrap_or(CapabilityTier::None);

    let effective_required_tier = std::cmp::max(project_tier, plugin_floor);

    let required_shapes_set = if requirements.required_shapes.is_empty() {
        // Auto-derive: caller has not given explicit shapes. The resolver
        // permits any shape combination; downstream Layer 3 still validates
        // per-plugin against adapter.supported_shapes via `validate_plan`.
        // For the resolver's filter, treat empty-required as "no shape filter"
        // (every adapter passes the shape filter).
        tau_domain::CapabilityShapeSet::new()
    } else {
        let mut s = tau_domain::CapabilityShapeSet::new();
        for shape in &requirements.required_shapes {
            s.insert(shape.clone());
        }
        s
    };

    let mut tried: Vec<(String, ResolutionRejection)> = Vec::new();
    let mut candidates: Vec<&AdapterRegistration> = Vec::new();

    for registration in REGISTRY.iter() {
        let name = registration.kind.name().to_owned();

        // Filter 1: platform match
        if !registration.platforms.includes(platform) {
            tried.push((name.clone(), ResolutionRejection::PlatformMismatch));
            continue;
        }

        // Filter 2: probe (instantiate adapter, await probe)
        let adapter = match instantiate(registration.kind) {
            Ok(a) => a,
            Err(msg) => {
                tried.push((name.clone(), ResolutionRejection::ProbeUnavailable(msg)));
                continue;
            }
        };
        let delivered = match adapter.probe().await {
            tau_ports::CapabilityProbe::Available { tier, .. } => tier,
            tau_ports::CapabilityProbe::Unavailable { reason } => {
                tried.push((name.clone(), ResolutionRejection::ProbeUnavailable(reason)));
                continue;
            }
            other => {
                tried.push((
                    name.clone(),
                    ResolutionRejection::ProbeUnavailable(format!("{other:?}")),
                ));
                continue;
            }
        };

        // Filter 3: delivered tier >= effective required tier
        if delivered < effective_required_tier {
            tried.push((
                name.clone(),
                ResolutionRejection::TierTooLow {
                    delivered,
                    required: effective_required_tier,
                },
            ));
            continue;
        }

        // Filter 4: required shapes ⊆ adapter's supported shapes
        let supported_shapes = (registration.shapes_supported_fn)();
        let mut missing = tau_domain::CapabilityShapeSet::new();
        for shape in required_shapes_set.iter() {
            if !supported_shapes.contains(shape) {
                missing.insert(shape.clone());
            }
        }
        if !missing.is_empty() {
            tried.push((
                name.clone(),
                ResolutionRejection::ShapesUnsupported { missing },
            ));
            continue;
        }

        // Filter 5: every plugin's required_tier <= delivered_tier
        let mut plugin_failure: Option<(String, tau_domain::PluginRequiredTier)> = None;
        for (idx, plugin) in plugins.iter().enumerate() {
            if let Some(p_tier) = &plugin.required_tier {
                let p_tier_mapped = match p_tier {
                    tau_domain::PluginRequiredTier::None => CapabilityTier::None,
                    tau_domain::PluginRequiredTier::Light => CapabilityTier::Light,
                    tau_domain::PluginRequiredTier::Strict => CapabilityTier::Strict,
                    _ => CapabilityTier::None,
                };
                if p_tier_mapped > delivered {
                    // We don't know plugin name in this context — use index
                    // for diagnostic; callers wrap a richer diagnostic
                    // (Task 6 wires plugin names into PluginSandboxRequirements
                    // via context).
                    plugin_failure = Some((format!("plugin[{idx}]"), *p_tier));
                    break;
                }
            }
        }
        if let Some((plugin_id, required)) = plugin_failure {
            tried.push((
                name.clone(),
                ResolutionRejection::PluginTierTooLow {
                    plugin: plugin_id,
                    required,
                },
            ));
            continue;
        }

        candidates.push(registration);
    }

    // §3.5 — pick highest-priority match
    candidates.sort_by_key(|r| Reverse(r.priority));
    let chosen_reg =
        candidates
            .into_iter()
            .next()
            .ok_or_else(|| ResolutionError::NoAdapterMatches {
                tried,
                platform: platform.to_owned(),
                required_tier: effective_required_tier,
            })?;

    instantiate(chosen_reg.kind).map_err(|m| ResolutionError::ConfigError { message: m })
}

// ---------------------------------------------------------------------------
// resolve_strict_for_validation — picks highest-priority non-passthrough adapter
// ---------------------------------------------------------------------------

/// Walk the registry and pick the highest-priority non-passthrough adapter
/// that probes `Available` on the current platform.
///
/// Used by `tau resolve --check-sandbox` when the project's `required_tier`
/// is `None` (so the project's actual resolution would yield passthrough) —
/// `--check-sandbox` should still validate against the strictest available
/// "real" adapter so users see what would happen if they strengthened the
/// requirement.
///
/// When `TAU_TESTING_ALLOW_MOCK_SANDBOX=1` is set, returns the mock adapter
/// (same as `resolve_adapter`) so tests remain reliable across platforms.
pub async fn resolve_strict_for_validation() -> Result<SandboxAdapter, ResolutionError> {
    // Mock injection — mirrors resolve_adapter behaviour.
    if std::env::var("TAU_TESTING_ALLOW_MOCK_SANDBOX")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Ok(SandboxAdapter::Mock(
            tau_ports::fixtures::MockCapabilityGate::new("mock"),
        ));
    }

    let platform = detect_platform();
    let mut tried: Vec<(String, ResolutionRejection)> = Vec::new();

    // Collect non-passthrough candidates in priority order (highest first).
    let mut candidates: Vec<&AdapterRegistration> = REGISTRY
        .iter()
        .filter(|r| r.kind != RegistryKind::Passthrough)
        .collect();
    candidates.sort_by_key(|r| Reverse(r.priority));

    for entry in candidates {
        let name = entry.kind.name().to_owned();

        if !entry.platforms.includes(platform) {
            tried.push((name, ResolutionRejection::PlatformMismatch));
            continue;
        }

        let adapter = match instantiate(entry.kind) {
            Ok(a) => a,
            Err(msg) => {
                tried.push((name, ResolutionRejection::ProbeUnavailable(msg)));
                continue;
            }
        };

        match adapter.probe().await {
            tau_ports::CapabilityProbe::Available { .. } => return Ok(adapter),
            tau_ports::CapabilityProbe::Unavailable { reason } => {
                tried.push((name, ResolutionRejection::ProbeUnavailable(reason)));
            }
            other => {
                tried.push((
                    name,
                    ResolutionRejection::ProbeUnavailable(format!("{other:?}")),
                ));
            }
        }
    }

    Err(ResolutionError::NoAdapterMatches {
        tried,
        platform: platform.to_owned(),
        required_tier: CapabilityTier::None,
    })
}

// ---------------------------------------------------------------------------
// resolve_adapter_forced — forced single-adapter path for --sandbox <kind>
// ---------------------------------------------------------------------------

/// Force-instantiate a specific adapter kind, bypassing the registry filter.
///
/// Probes the named adapter; returns `Ok(adapter)` iff
/// [`tau_ports::CapabilityProbe::Available`], otherwise
/// [`ResolutionError::ConfigError`] with a guided message (e.g.
/// `"--sandbox native is not applicable on macOS"`).
pub async fn resolve_adapter_forced(kind: RegistryKind) -> Result<SandboxAdapter, ResolutionError> {
    let adapter = instantiate(kind).map_err(|m| ResolutionError::ConfigError {
        message: format!("--sandbox {kind:?}: {m}"),
    })?;
    match adapter.probe().await {
        tau_ports::CapabilityProbe::Available { .. } => Ok(adapter),
        tau_ports::CapabilityProbe::Unavailable { reason } => Err(ResolutionError::ConfigError {
            message: format!("--sandbox {kind:?} not applicable: {reason}"),
        }),
        other => Err(ResolutionError::ConfigError {
            message: format!("--sandbox {kind:?}: unexpected probe result: {other:?}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tau_domain::{PluginRequiredTier, PluginSandboxRequirements};
    use tau_pkg::scope::{SandboxRequiredTier, SandboxRequirements};
    use tokio::sync::Mutex;

    // Process-wide lock for tests that read or write TAU_TESTING_ALLOW_MOCK_SANDBOX.
    // cargo test runs tests in parallel within a single process; env vars are
    // process-global, so any test that touches this env var must serialize.
    // tokio::sync::Mutex is used (not std) so the guard is Send across .await.
    static MOCK_ENV_LOCK: Mutex<()> = Mutex::const_new(());

    // 1. default_requirements_resolves_to_some_adapter
    //
    // Default SandboxRequirements has required_tier = Strict.
    // On macOS: native rejected (Linux-only), container may or may not be
    // available; either Ok(_) or NoAdapterMatches is acceptable.
    #[tokio::test]
    async fn default_requirements_resolves_to_some_adapter() {
        let _g = MOCK_ENV_LOCK.lock().await;
        // Ensure mock env var is NOT set so we exercise the real registry path.
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let requirements = SandboxRequirements::default(); // required_tier = Strict
        let result = resolve_adapter(&requirements, &[]).await;
        match result {
            Ok(_) | Err(ResolutionError::NoAdapterMatches { .. }) => {}
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }

    // 2. mock_explicit_via_env_var_resolves_to_mock
    //
    // When TAU_TESTING_ALLOW_MOCK_SANDBOX=1, the resolver bypasses the
    // registry and returns SandboxAdapter::Mock.
    #[tokio::test]
    async fn mock_explicit_via_env_var_resolves_to_mock() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::set_var("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1");
        let result = resolve_adapter(&SandboxRequirements::default(), &[]).await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let adapter = result.expect("mock should be returned when env var is set");
        assert!(
            matches!(adapter, SandboxAdapter::Mock(_)),
            "expected Mock variant, got: {adapter:?}",
        );
    }

    // 3. required_tier_strict_with_only_passthrough_unsatisfiable
    //
    // On macOS with Strict, native is platform-rejected, container is likely
    // probe-rejected (no docker). Passthrough is tier-too-low.
    // Result should be NoAdapterMatches OR Ok(Native/Container) on a real
    // Linux host — never Ok(Passthrough).
    #[tokio::test]
    async fn required_tier_strict_with_only_passthrough_unsatisfiable() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let requirements = SandboxRequirements::with_tier(SandboxRequiredTier::Strict);
        let result = resolve_adapter(&requirements, &[]).await;
        match &result {
            Ok(adapter) => {
                // If an adapter was found it must NOT be Passthrough (Passthrough delivers None).
                assert!(
                    !matches!(adapter, SandboxAdapter::Passthrough(_)),
                    "Passthrough should not satisfy Strict requirement"
                );
            }
            Err(ResolutionError::NoAdapterMatches { tried, .. }) => {
                // Passthrough should appear in `tried` with a TierTooLow rejection.
                let passthrough_rejection = tried.iter().find(|(name, _)| name == "passthrough");
                if let Some((_, rejection)) = passthrough_rejection {
                    assert!(
                        matches!(rejection, ResolutionRejection::TierTooLow { .. }),
                        "passthrough should be rejected with TierTooLow, got: {rejection:?}"
                    );
                }
                // (Passthrough may not appear at all if higher-priority adapters exhaust
                // the `candidates` list before reaching it — still fine.)
            }
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }

    // 4. required_tier_none_resolves_to_passthrough_when_no_other_match
    //
    // With required_tier=None, Passthrough (tier=None) should pass the tier
    // filter and be selected on macOS (where native/container are typically
    // unavailable). On Linux, native may win at higher priority — that's fine.
    #[tokio::test]
    async fn required_tier_none_resolves_to_passthrough_when_no_other_match() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let requirements = SandboxRequirements::with_tier(SandboxRequiredTier::None);
        let result = resolve_adapter(&requirements, &[]).await;
        // Any Ok is fine; on macOS Passthrough should win; on Linux native may.
        match result {
            Ok(_) => {} // any adapter is acceptable
            Err(e) => panic!("expected Ok with tier=None but got: {e:?}"),
        }
    }

    // 5. plugin_tier_strict_rejects_passthrough_only_chain
    //
    // With one plugin requiring Strict and (on macOS) only Passthrough passing
    // tier=None in the registry, the plugin filter should reject Passthrough.
    // Expected: NoAdapterMatches on macOS, or Ok(Native/Container) on Linux.
    #[tokio::test]
    async fn plugin_tier_strict_rejects_passthrough_only_chain() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        let requirements = SandboxRequirements::default(); // Strict
        let mut plugin = PluginSandboxRequirements::default();
        plugin.required_tier = Some(PluginRequiredTier::Strict);
        let result = resolve_adapter(&requirements, &[plugin]).await;
        match &result {
            Ok(adapter) => {
                // If we get an adapter, it must not be Passthrough (which delivers None).
                assert!(
                    !matches!(adapter, SandboxAdapter::Passthrough(_)),
                    "Passthrough cannot satisfy a plugin Strict requirement"
                );
            }
            Err(ResolutionError::NoAdapterMatches { .. }) => {}
            Err(e) => panic!("unexpected error variant: {e:?}"),
        }
    }

    // 6. unknown_platform_returns_error_or_passthrough
    //
    // This smoke-tests that the resolver doesn't panic on unusual/unknown
    // platforms. We simulate by calling with default requirements — the actual
    // platform is whatever the test runner is on.
    #[tokio::test]
    async fn unknown_platform_smoke_does_not_panic() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");

        // Just call the resolver; verify it returns something without panicking.
        let requirements = SandboxRequirements::with_tier(SandboxRequiredTier::None);
        let result = resolve_adapter(&requirements, &[]).await;
        // Any result is fine — we're asserting "no panic".
        let _ = result;
    }

    // Extra: verify Debug impl doesn't panic for all variants.
    #[tokio::test]
    async fn sandbox_adapter_debug_impl_all_variants() {
        let _g = MOCK_ENV_LOCK.lock().await;
        std::env::set_var("TAU_TESTING_ALLOW_MOCK_SANDBOX", "1");
        let mock = resolve_adapter(&SandboxRequirements::default(), &[])
            .await
            .unwrap();
        std::env::remove_var("TAU_TESTING_ALLOW_MOCK_SANDBOX");
        let _ = format!("{mock:?}");

        // Passthrough can be constructed directly
        let pt = SandboxAdapter::Passthrough(PassthroughSandbox::new());
        let _ = format!("{pt:?}");
    }
}
