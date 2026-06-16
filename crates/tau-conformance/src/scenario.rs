//! Fixture loader: `tau.toml` -> [`IrModule`], plus scripted LLM responses
//! and the weather cassette path.
//!
//! Mirrors the proven lowering path in
//! `tau-ir-conformance::dev_mode`: parse `tau.toml` into a
//! [`ProjectConfig`], pick the first available target triple, build the
//! SHA-256-of-name native-tool [`Caches`], and call [`lower_project`]. The
//! native-tool cache key is byte-identical to `tau-cli::cmd::build` and
//! `tau-ir-conformance`, so every profile hashes the same IR bytes for the
//! same source workflow.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tau_ir_lower::{lower_project, Caches};
use tau_ir::{AgentId, IrModule};
use tau_pkg::project::ProjectConfig;
use tau_ports::llm::CompletionResponse;
use tau_ports::target::registry as target_registry;

use crate::sequenced_llm::parse_mock_llm;

/// A loaded conformance scenario: the lowered IR module, its entry agent,
/// the scripted LLM responses, and the path to the weather cassette.
pub struct Scenario {
    /// The lowered IR module (shared so the interpreter can clone cheaply).
    pub module: Arc<IrModule>,
    /// Entry agent: first agent in the module's `BTreeMap` (alphabetical).
    pub entry: AgentId,
    /// Scripted LLM responses replayed in order from `mock_llm.jsonl`.
    pub responses: Vec<CompletionResponse>,
    /// Path to `weather.cassette.jsonl`. Not opened at load time — the
    /// profile opens it when the run actually invokes the MCP transport.
    pub weather_cassette: PathBuf,
    /// The fixture directory the scenario was loaded from.
    pub dir: PathBuf,
}

impl Scenario {
    /// Resolve a fixture dir under `crates/tau-conformance/fixtures/<name>`.
    // used by tests in Task 12
    #[allow(dead_code)]
    pub fn fixture_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name)
    }

    /// Load a scenario from `dir`: parse `tau.toml`, lower to an
    /// [`IrModule`], and decode `mock_llm.jsonl` into scripted responses.
    ///
    /// Returns `Err(String)` on any failure (read error, parse error, or
    /// lowering refusal) — error stringification mirrors
    /// `tau-ir-conformance::dev_mode`.
    pub fn load(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();

        // 1. Read + parse tau.toml. Project-config validation errors surface
        //    here as Err, before lowering.
        let toml = std::fs::read_to_string(dir.join("tau.toml"))
            .map_err(|e| format!("read tau.toml: {e}"))?;
        let config = ProjectConfig::parse_str(&toml).map_err(|e| format!("{e}"))?;

        // 2. Lower to IrModule. The Caches closures are stack-only and dropped
        //    at the end of this block. SHA-256-of-name native-tool cache is
        //    symmetric with tau-ir-conformance and tau-cli::cmd::build.
        let target = target_registry::list_available()
            .next()
            .ok_or("no target triple available")?
            .triple;
        let module = {
            let caches = Caches {
                native_tool: &|name: &str| Some(sha256_name(name)),
                mcp_contract: &|_| None,
                skill: &|_| None,
            };
            lower_project(&config, &target, &caches).map_err(|e| format!("{e}"))?
        };

        // 3. Entry agent: first in BTreeMap (alphabetical) order.
        let entry = module
            .workflow
            .agents
            .keys()
            .next()
            .ok_or("fixture must declare at least one agent")?
            .clone();

        // 4. Decode mock_llm.jsonl into scripted responses.
        let mock_llm_jsonl = std::fs::read_to_string(dir.join("mock_llm.jsonl"))
            .map_err(|e| format!("read mock_llm.jsonl: {e}"))?;
        let responses = parse_mock_llm(&mock_llm_jsonl);

        let weather_cassette = dir.join("weather.cassette.jsonl");

        Ok(Self {
            module: Arc::new(module),
            entry,
            responses,
            weather_cassette,
            dir,
        })
    }
}

/// SHA-256-of-name native-tool cache key, symmetric with
/// `tau-ir-conformance::sha256_name` and `tau-cli::cmd::build::sha256_name`
/// so every profile hashes identical IR bytes for the same source workflow.
fn sha256_name(name: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_min_fixture(dir: &std::path::Path) {
        let toml = r#"
[project]
name = "mini"

[agents.solo]
display_name = "Solo"
package      = "solo@^0.1"
llm_backend  = "mock-llm"
model        = "mock-1"
tool_refs    = ["read_temp"]
max_turns    = 2

[tools.read_temp]
native      = "ReadTemp"
description = "Read temp."
capabilities = []
"#;
        std::fs::write(dir.join("tau.toml"), toml).unwrap();
        let mut f = std::fs::File::create(dir.join("mock_llm.jsonl")).unwrap();
        writeln!(
            f,
            "{{\"response\": {{\"text\": \"ok\", \"stop_reason\": \"end_turn\"}}}}"
        )
        .unwrap();
    }

    #[test]
    fn loads_minimal_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        write_min_fixture(tmp.path());
        let s = Scenario::load(tmp.path().to_path_buf()).expect("loads");
        assert_eq!(s.entry.0, "solo");
        assert_eq!(s.responses.len(), 1);
        assert!(s.weather_cassette.ends_with("weather.cassette.jsonl"));
    }
}
