//! Shared test helpers for tau-mcp-tokio integration tests.
//!
//! - `mock_server_path()` — builds the in-tree fixture binary on demand
//!   and returns its path.
//! - `passthrough_gate()` — returns a `DynProcessCapabilityGate` impl
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

use tau_runtime_tokio::process_gate::DynProcessCapabilityGate;

static MOCK_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Path to the fixture binary. First call builds it; subsequent calls
/// return the cached path.
///
/// Uses `cargo build --message-format=json-render-diagnostics` and
/// parses the resulting `compiler-artifact` entries to find the
/// executable's exact path, rather than guessing where cargo will
/// place it from CARGO_TARGET_DIR. The guessing approach broke on
/// macos CI where the actual layout differed from the guess (PR #281
/// merge-queue check failed with `TokioSpawn("No such file or
/// directory")` on the constructed path).
pub fn mock_server_path() -> &'static PathBuf {
    MOCK_PATH.get_or_init(|| {
        let manifest = format!(
            "{}/tests/fixtures/mock-mcp-server/Cargo.toml",
            env!("CARGO_MANIFEST_DIR")
        );
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .unwrap_or_else(|_| format!("{}/../../target", env!("CARGO_MANIFEST_DIR")));
        let fixture_target = format!("{target_dir}/mock-mcp-server-build");
        let output = Command::new(env!("CARGO"))
            .args([
                "build",
                "--manifest-path",
                &manifest,
                "--message-format=json-render-diagnostics",
            ])
            .env("CARGO_TARGET_DIR", &fixture_target)
            .env("CARGO_INCREMENTAL", "0")
            .output()
            .expect("invoke cargo build");
        assert!(
            output.status.success(),
            "fixture build failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        for line in std::str::from_utf8(&output.stdout)
            .expect("cargo json output is utf8")
            .lines()
        {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
                continue;
            }
            if let Some(exe) = value.get("executable").and_then(|e| e.as_str()) {
                return PathBuf::from(exe);
            }
        }
        panic!(
            "no compiler-artifact executable in cargo build output:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    })
}

/// A `DynProcessCapabilityGate` that allows everything. Reuses the
/// existing `PassthroughSandbox` from `tau-runtime-tokio::process_gate`.
pub fn passthrough_gate() -> Arc<dyn DynProcessCapabilityGate> {
    use tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox;
    Arc::new(PassthroughSandbox::new())
}
