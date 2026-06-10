//! Integration tests for `host_lifecycle::cassette_dial::dial`.

mod common;

use tau_mcp_tokio::host_lifecycle::client::McpClientOptions;
use tau_mcp_tokio::host_lifecycle::open::open;
use tau_ports::CapabilityPlan;

use crate::common::passthrough_gate;

fn empty_plan() -> CapabilityPlan {
    CapabilityPlan::new(vec![], None, None)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_cassette_replays_handshake() {
    let cassette = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/weather_minimal_cassette.jsonl");
    let plan = empty_plan();
    let gate = passthrough_gate();
    let client = open(
        &format!("cassette:{cassette}"),
        &plan,
        gate,
        McpClientOptions::default(),
    )
    .await
    .expect("open cassette");
    assert!(
        !client.contract().tools.is_empty(),
        "handshake should yield at least one tool"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cassette_missing_file_errors() {
    let plan = empty_plan();
    let gate = passthrough_gate();
    let result = open(
        "cassette:/nonexistent/file.jsonl",
        &plan,
        gate,
        McpClientOptions::default(),
    )
    .await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected Err but got Ok"),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("nonexistent") || msg.contains("file.jsonl"),
        "expected path in error, got: {msg}"
    );
}
