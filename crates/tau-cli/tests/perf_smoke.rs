//! Performance smoke test (spec §10.7).
//!
//! Asserts that spawning the real `echo-llm` plugin, completing the
//! `meta.handshake` round-trip, and shutting it down again finishes
//! comfortably under one second on a release build.
//!
//! The bound is intentionally loose — a pure regression net for
//! "something pathologically wrong" rather than a tight latency
//! budget. Slow CI runners commonly hit ~200 ms for a plain process
//! spawn + handshake + reap; a one-second ceiling leaves 5x headroom
//! before we fail loud.

mod common;

use std::time::{Duration, Instant, SystemTime};

use tau_domain::{PluginKind, PluginManifest, PortKind};
use tau_pkg::LockedPlugin;
use tau_plugin_protocol::handshake::TraceContext;
use tau_runtime_tokio::plugin_host::{self, PluginHostOptions};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_completes_under_one_second_in_release() {
    let echo_llm = common::echo_plugins::echo_llm_binary().clone();

    // Build a `LockedPlugin` straight against the pre-built binary —
    // no install, no scope, just enough to drive `describe_plugin`.
    let manifest = PluginManifest::new(
        PortKind::LlmBackend,
        PluginKind::RustCargo,
        "echo-llm".to_string(),
    );
    let plugin = LockedPlugin::new(manifest, echo_llm, SystemTime::now(), String::new());

    let trace_context = TraceContext::new(
        "perf-smoke".to_string(),
        "perf-smoke-agent".to_string(),
        "root".to_string(),
    );

    let options = PluginHostOptions::default();

    let start = Instant::now();
    let response = plugin_host::describe_plugin(&plugin, trace_context, options)
        .await
        .expect("describe_plugin against echo-llm should succeed");
    let elapsed = start.elapsed();

    assert_eq!(response.plugin_name, "echo-llm");
    assert_eq!(response.provides, PortKind::LlmBackend);
    assert!(
        elapsed < Duration::from_secs(1),
        "echo-llm handshake too slow on release: {elapsed:?} (budget = 1s)"
    );
}
