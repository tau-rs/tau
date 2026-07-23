//! Shared test helpers for tau-mcp-tokio integration tests.
//!
//! - `mock_server_path()` — builds the in-tree fixture binary on demand
//!   and returns its path.
//! - `passthrough_gate()` — returns a `DynProcessGate` impl
//!   that doesn't enforce anything (for tests that aren't exercising
//!   sandbox refusal).
//!
//! `#![allow(dead_code)]` because not every integration-test binary uses
//! every helper (e.g. `stdio_sandbox.rs` brings its own gate, so it never
//! calls `passthrough_gate`), and each test binary that does `mod common;`
//! gets its own dead-code analysis.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use tau_ports::DynProcessGate;

static MOCK_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Path to the fixture binary. First call builds it; subsequent calls
/// return the cached path.
pub fn mock_server_path() -> &'static PathBuf {
    MOCK_PATH.get_or_init(|| {
        let manifest = format!(
            "{}/tests/fixtures/mock-mcp-server/Cargo.toml",
            env!("CARGO_MANIFEST_DIR")
        );
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .unwrap_or_else(|_| format!("{}/../../target", env!("CARGO_MANIFEST_DIR")));
        let fixture_target = format!("{target_dir}/mock-mcp-server-build");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--manifest-path", &manifest])
            .env("CARGO_TARGET_DIR", &fixture_target)
            .env("CARGO_INCREMENTAL", "0")
            .status()
            .expect("build fixture");
        assert!(status.success(), "fixture build failed");
        PathBuf::from(format!("{fixture_target}/debug/tau-mcp-mock-server"))
    })
}

/// A `DynProcessGate` that allows everything. Uses `tau_ports::PassthroughGate`.
pub fn passthrough_gate() -> Arc<dyn DynProcessGate> {
    Arc::new(tau_ports::PassthroughGate::new())
}
