//! Confirms `host_lifecycle::open` propagates a sandbox refusal as
//! `LifecycleError::StdioSpawn(StdioSpawnError::SandboxRefused)`.
//!
//! `DynProcessCapabilityGate` is a blanket impl over any
//! `T: ProcessCapabilityGate + 'static`, so `AlwaysRefuseGate` implements
//! the `CapabilityGate` + `ProcessCapabilityGate` sub-traits directly.

mod common;

use std::sync::Arc;

use tau_ports::{
    CapabilityError, CapabilityGate, CapabilityHandle, CapabilityPlan, CapabilityProbe,
    CapabilityShapeSet, CapabilityTier, ProcessCapabilityGate,
};

use tau_mcp_tokio::{open, LifecycleError, McpClientOptions, StdioSpawnError};

use crate::common::mock_server_path;

/// A gate that always refuses `wrap_spawn` with a fixed error.
struct AlwaysRefuseGate;

impl CapabilityGate for AlwaysRefuseGate {
    fn name(&self) -> &str {
        "always-refuse"
    }

    async fn probe(&self) -> CapabilityProbe {
        CapabilityProbe::Available {
            tier: CapabilityTier::None,
            details: "test gate".into(),
        }
    }

    fn supported_shapes(&self) -> CapabilityShapeSet {
        CapabilityShapeSet::default()
    }

    fn validate_plan(&self, _plan: &CapabilityPlan) -> Result<(), CapabilityError> {
        Ok(())
    }
}

impl ProcessCapabilityGate for AlwaysRefuseGate {
    async fn wrap_spawn(
        &self,
        _plan: &CapabilityPlan,
        _cmd: &mut std::process::Command,
    ) -> Result<CapabilityHandle, CapabilityError> {
        Err(CapabilityError::WrapFailed {
            message: "test gate refuses every plan".into(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_refusal_propagates() {
    let url = format!("stdio:{}", mock_server_path().to_string_lossy());
    let gate: Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> =
        Arc::new(AlwaysRefuseGate);

    let result = open(
        &url,
        &CapabilityPlan::new(vec![], None, None),
        gate,
        McpClientOptions::default(),
    )
    .await;
    let err = match result {
        Ok(_) => panic!("sandbox_refusal_propagates: expected Err but got Ok"),
        Err(e) => e,
    };
    match err {
        LifecycleError::StdioSpawn(StdioSpawnError::SandboxRefused(_)) => {}
        other => panic!("expected SandboxRefused, got {other:?}"),
    }
}
