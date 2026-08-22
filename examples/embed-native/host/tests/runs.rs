//! EPIC 7.1: the Variant-B embedding runs to completion in CI.
use std::sync::Arc;

use embed_native_host::HostDispatcher;
use embed_native_workflow_lib::{run_ir, TAU_IR};
use tau_ir::from_canonical_bytes;
use tau_runtime_core::outcome::RunOutcome;

#[tokio::test]
async fn embedding_runs_to_completion() {
    let module = Arc::new(from_canonical_bytes(TAU_IR).expect("TAU_IR decodes"));
    let entry = module
        .workflow
        .agents
        .keys()
        .next()
        .expect("IR module has at least one agent")
        .clone();

    let outcome = run_ir(module, &entry, Arc::new(HostDispatcher::new()), Vec::new())
        .await
        .expect("run_ir returns an outcome");

    match outcome {
        RunOutcome::Completed {
            total_turns,
            final_message,
            ..
        } => {
            assert_eq!(total_turns, 2, "tool-call turn + final text turn");
            assert!(
                format!("{final_message:?}").contains("done"),
                "final assistant message should carry 'done': {final_message:?}"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}
