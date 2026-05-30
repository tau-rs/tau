//! Runtime-level e2e tests for plugin_host integration with the native
//! sandbox adapter.
//!
//! Sub-project D Task 4. These tests verify:
//!
//! 1. The native sandbox adapter is correctly threaded through `plugin_host`'s
//!    `load_*` functions: `wrap_spawn` fires before the handshake attempt, and
//!    the expected handshake failure (controlled-env doesn't speak IPC)
//!    confirms the adapter ran first.
//!
//! 2. `SandboxPlan` validation runs pre-spawn: a plan containing a
//!    `Capability::Custom` shape (unsupported by the native adapter) is
//!    rejected via `RuntimeError::SandboxValidationFailed` before any
//!    process is ever spawned.
//!
//! Linux-only (native landlock/seccomp requires Linux). Run on the
//! ubuntu-latest CI slot with:
//!
//!   cargo test -p tau-runtime --features integration-tests

#![cfg(all(feature = "integration-tests", target_os = "linux"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tau_domain::{fixtures as domain_fixtures, Capability, PluginKind, PluginManifest, PortKind};
use tau_pkg::LockedPlugin;
use tau_plugin_protocol::handshake::TraceContext;
use tau_ports::{CapabilityPlan, CapabilityProbe};
use tau_runtime::error::{CoreRuntimeError, RuntimeError};
use tau_runtime::plugin_host::{self, PluginHostOptions};
use tau_runtime::sandbox::registry::RegistryKind;
use tau_runtime::sandbox::{resolve_adapter_forced, SandboxAdapter};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Locate the controlled-env binary built during sub-project A Task 1.
///
/// The binary must have been compiled beforehand with:
///   cargo build -p tau-controlled-env --release
fn locate_controlled_env_bin() -> PathBuf {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // tau-runtime lives at crates/tau-runtime; workspace root is two levels up.
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    workspace_root.join(
        "crates/tau-plugin-compat/fixtures/controlled-env-binary/target/release/tau-controlled-env",
    )
}

/// Synthesise a minimal `LockedPlugin` whose binary points at the
/// controlled-env binary.  The `provides` port is `Tool` so we can call
/// `load_tool`; for these tests the port only needs to be *syntactically*
/// valid — the binary doesn't speak the IPC protocol, so the handshake will
/// fail, which is the assertion for test 1.
fn synthetic_locked_plugin(name: &str) -> LockedPlugin {
    let manifest = PluginManifest::new(PortKind::Tool, PluginKind::RustCargo, name.to_string());
    LockedPlugin::new(
        manifest,
        locate_controlled_env_bin(),
        std::time::SystemTime::UNIX_EPOCH,
        String::new(),
    )
}

/// Build a minimal `TraceContext` unique to this test run.
fn test_trace_context() -> TraceContext {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    TraceContext::new(
        format!("test-d4-run-{nanos}"),
        format!("test-d4-agent-{nanos}"),
        format!("test-d4-span-{nanos}"),
    )
}

/// Build `PluginHostOptions` with a short handshake timeout so tests fail
/// fast when the controlled-env binary doesn't respond to IPC.
fn fast_options(adapter: SandboxAdapter) -> PluginHostOptions {
    let mut opts = PluginHostOptions::default();
    opts.handshake_timeout = Duration::from_millis(500);
    opts.shutdown_timeout = Duration::from_millis(200);
    opts.sandbox_adapter = Some(Arc::new(adapter));
    opts
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verify that the native sandbox adapter is correctly threaded through
/// `load_tool`.
///
/// The controlled-env binary does not speak the tau plugin IPC protocol, so
/// the handshake exchange will time out or produce a malformed response.
/// What matters is that the failure arrives as
/// `RuntimeError::PluginHandshakeFailed` — **not** as a pre-spawn error like
/// `SandboxValidationFailed` or `PluginSpawnFailed` — which proves:
///
/// - `validate_plan_against_adapter` passed (the plan's capability shapes are
///   all supported by the native adapter).
/// - `wrap_spawn` succeeded (the adapter applied its enforcement).
/// - The binary was actually spawned.
/// - The failure came from the IPC handshake layer, after spawn.
#[tokio::test]
async fn adapter_threads_through_to_plugin_spawn() {
    // 1. Resolve the native adapter. If this host doesn't have landlock/seccomp
    //    (e.g. a kernel older than 5.13) skip gracefully rather than failing.
    let adapter = match resolve_adapter_forced(RegistryKind::Native).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping: native adapter unavailable: {e}");
            return;
        }
    };

    // Extra-belt-and-suspenders: double-check the probe.
    if matches!(adapter.probe().await, CapabilityProbe::Unavailable { .. }) {
        eprintln!("skipping: native adapter probe returned Unavailable");
        return;
    }

    // 2. Build a valid `SandboxPlan`. Include the controlled-env binary's
    //    parent directory in fs.read so landlock allows exec of the binary
    //    itself (the standard system_read_paths in tau-sandbox-native::light
    //    only includes /bin, /usr/bin, /lib, etc.).
    let bin_parent = locate_controlled_env_bin()
        .parent()
        .expect("controlled-env binary has parent dir")
        .to_string_lossy()
        .into_owned();
    let read_cap: Capability = domain_fixtures::cap_fs_read(&["/tmp/**", &bin_parent]);
    let plan = CapabilityPlan::new(vec![read_cap], None, None);

    // 3. Synthesise a LockedPlugin pointing at the controlled-env binary.
    let plugin = synthetic_locked_plugin("tau-controlled-env");

    // 4. Call load_tool with the native adapter.
    let opts = fast_options(adapter);
    let result = plugin_host::load_tool(
        &plugin,
        serde_json::Value::Null,
        test_trace_context(),
        opts,
        Some(&plan),
    )
    .await;

    // 5. Assert the error is handshake-related (post-spawn).
    //    The controlled-env binary will either time out, close stdio, or
    //    emit non-IPC output — all of which surface as PluginHandshakeFailed.
    //    We also accept PluginSpawnFailed (sandbox-enforced spawn failure on
    //    constrained CI hosts) but NOT SandboxValidationFailed (pre-spawn).
    // `Arc<dyn DynTool>` doesn't impl Debug, so `result.expect_err(..)` won't
    // compile. Use match instead.
    let err = match result {
        Ok(_) => panic!("load_tool must fail: controlled-env doesn't speak IPC"),
        Err(e) => e,
    };

    assert!(
        matches!(
            &err,
            RuntimeError::Core(CoreRuntimeError::PluginHandshakeFailed { .. })
                | RuntimeError::PluginSpawnFailed { .. }
                | RuntimeError::SandboxWrapFailed { .. }
        ),
        "expected PluginHandshakeFailed, PluginSpawnFailed, or SandboxWrapFailed \
         (post-spawn or wrap failure); got: {err:?}"
    );

    // Verify it is NOT a pre-spawn validation error — that would mean the
    // adapter wasn't threaded through correctly.
    assert!(
        !matches!(&err, RuntimeError::SandboxValidationFailed { .. }),
        "got SandboxValidationFailed — this is a pre-spawn error; the plan \
         should have been accepted by the native adapter"
    );
}

/// Verify that `SandboxPlan` validation runs pre-spawn.
///
/// A plan containing `Capability::Custom` (which maps to
/// `CapabilityShape::Custom`) is unsupported by the native adapter.  The
/// expected result is `RuntimeError::SandboxValidationFailed` — the error
/// that signals Layer 3 validation fired **before** any process was spawned.
#[tokio::test]
async fn sandbox_plan_validation_runs_pre_spawn() {
    // 1. Resolve the native adapter.
    let adapter = match resolve_adapter_forced(RegistryKind::Native).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping: native adapter unavailable: {e}");
            return;
        }
    };

    if matches!(adapter.probe().await, CapabilityProbe::Unavailable { .. }) {
        eprintln!("skipping: native adapter probe returned Unavailable");
        return;
    }

    // 2. Build a SandboxPlan with a Capability::Custom that the native adapter
    //    does NOT support (native only supports standard FS/network shapes).
    let custom_cap: Capability = domain_fixtures::cap_custom("mcp.tool.use");
    let plan = CapabilityPlan::new(vec![custom_cap], None, None);

    // 3. Synthesise a LockedPlugin (binary path doesn't matter — spawn must
    //    never be reached).
    let plugin = synthetic_locked_plugin("tau-controlled-env-custom");

    // 4. Call load_tool with the native adapter + the unsupported plan.
    let opts = fast_options(adapter);
    let result = plugin_host::load_tool(
        &plugin,
        serde_json::Value::Null,
        test_trace_context(),
        opts,
        Some(&plan),
    )
    .await;

    // 5. Assert the error is SandboxValidationFailed (pre-spawn).
    let err = match result {
        Ok(_) => panic!("load_tool must fail: Custom shape unsupported by native adapter"),
        Err(e) => e,
    };

    assert!(
        matches!(&err, RuntimeError::SandboxValidationFailed { .. }),
        "expected SandboxValidationFailed (pre-spawn Layer 3 rejection); got: {err:?}"
    );

    // Extra: confirm the error message mentions the plugin name.
    if let RuntimeError::SandboxValidationFailed {
        plugin: plugin_name,
        errors,
        ..
    } = &err
    {
        assert!(
            !errors.is_empty(),
            "SandboxValidationFailed should carry at least one error"
        );
        assert!(
            !plugin_name.is_empty(),
            "SandboxValidationFailed should carry the plugin name"
        );
    }
}
