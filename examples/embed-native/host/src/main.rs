//! Runnable Variant-B embedding: `cargo run -p embed-native-host`.
use std::sync::Arc;

use embed_native_host::HostDispatcher;
use embed_native_workflow_lib::{run_ir, TAU_IR};
use tau_ir::from_canonical_bytes;
use tau_runtime_core::outcome::RunOutcome;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = Arc::new(from_canonical_bytes(TAU_IR)?);
    let entry = module
        .workflow
        .agents
        .keys()
        .next()
        .expect("IR module has at least one agent")
        .clone();

    let outcome = run_ir(module, &entry, Arc::new(HostDispatcher::new()), Vec::new()).await?;
    println!("{outcome:#?}");

    if matches!(outcome, RunOutcome::Failed { .. }) {
        std::process::exit(1);
    }
    Ok(())
}
