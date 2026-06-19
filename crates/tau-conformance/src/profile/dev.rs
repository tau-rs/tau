//! Interpreted dev profile: drive `run_ir_streaming` and normalize its
//! single typed `RunEvent` stream into `ConformanceEvent`s (ADR-0049,
//! supersedes ADR-0048's dual-channel interleave). No tracing subscriber
//! is installed — the engine emits every gate event as a typed variant,
//! which is exactly what a no_std wasm guest produces across the
//! component boundary.

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt as _;

use crate::dispatcher::ConformanceDispatcher;
use crate::event::ConformanceEvent;
use crate::normalize::{map_runevent, NormState};
use crate::profile::{Profile, ProfileError};
use crate::scenario::Scenario;
use crate::sequenced_llm::SequencedLlm;

/// The interpreted-runtime conformance profile. See module docs.
pub struct DevProfile;

#[async_trait::async_trait(?Send)]
impl Profile for DevProfile {
    fn name(&self) -> &str {
        "dev"
    }

    async fn run(&self, scenario: &Scenario) -> Result<Vec<ConformanceEvent>, ProfileError> {
        let backend = Arc::new(SequencedLlm::new("mock-llm", scenario.responses.clone()));

        // Open the cassette-backed MCP weather client ONLY if the cassette
        // file exists (native-only scenarios have none).
        let dispatcher = if scenario.weather_cassette.exists() {
            let client = open_weather_client(&scenario.weather_cassette)
                .await
                .map_err(|e| ProfileError(format!("open weather cassette: {e}")))?;
            Arc::new(ConformanceDispatcher::new(backend, client))
        } else {
            Arc::new(ConformanceDispatcher::new_native_only(backend))
        };

        let stream = tau_runtime_core::interpreter::run_ir_streaming(
            scenario.module.clone(),
            &scenario.entry,
            dispatcher,
            Vec::new(),
        )
        .await
        .map_err(|e| ProfileError(format!("run_ir_streaming: {e}")))?;
        let mut stream = Box::pin(stream);

        // Single-channel: every conformance event is sourced from the typed
        // `RunEvent` stream. No tracing capture, no interleave.
        let mut out: Vec<ConformanceEvent> = Vec::new();
        let mut st = NormState::default();
        while let Some(ev) = stream.next().await {
            if let Some(ce) = map_runevent(ev, &mut st) {
                out.push(ce);
            }
        }
        Ok(out)
    }
}

/// Open the cassette-backed `weather` MCP client for the dev profile.
///
/// Uses the `cassette:` URL scheme (replay, no real process spawn), a
/// passthrough process gate, and an empty capability plan — the cassette
/// transport needs no real capabilities.
async fn open_weather_client(
    path: &Path,
) -> Result<Arc<tau_mcp_tokio::host_lifecycle::client::McpClient>, String> {
    use tau_mcp_tokio::host_lifecycle::{open, McpClientOptions};
    let url = format!("cassette:{}", path.display());
    let gate: Arc<dyn tau_runtime_tokio::process_gate::DynProcessCapabilityGate> =
        Arc::new(tau_runtime_tokio::process_gate::passthrough::PassthroughSandbox::new());
    let plan = tau_ports::CapabilityPlan::new(Vec::new(), None, None);
    let client = open(&url, &plan, gate, McpClientOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ConformanceEvent;
    use std::io::Write;

    fn write_native_fixture(dir: &std::path::Path) {
        let toml = r#"
packages = ["mock-llm"]

[project]
name = "mini"

[models.mock-1]
backend = "mock-llm"
model = "mock-1"

[agents.solo]
display_name = "Solo"
package      = "solo@^0.1"
model        = "mock-1"
tool_refs    = ["read_temp"]
max_turns    = 3

[tools.read_temp]
native      = "ReadTemp"
description = "Read temp."
capabilities = []
"#;
        std::fs::write(dir.join("tau.toml"), toml).unwrap();
        let mut f = std::fs::File::create(dir.join("mock_llm.jsonl")).unwrap();
        writeln!(f, "{{\"response\": {{\"tool_uses\": [{{\"id\": \"t0\", \"name\": \"read_temp\", \"input\": {{}}}}], \"stop_reason\": \"tool_use\"}}}}").unwrap();
        writeln!(
            f,
            "{{\"response\": {{\"text\": \"done\", \"stop_reason\": \"end_turn\"}}}}"
        )
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dev_profile_native_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        write_native_fixture(tmp.path());
        let s = crate::scenario::Scenario::load(tmp.path().to_path_buf()).unwrap();
        let events = DevProfile.run(&s).await.expect("dev runs");
        assert!(
            matches!(events.first(), Some(ConformanceEvent::RunStarted)),
            "first={:?}",
            events.first()
        );
        assert!(
            matches!(events.last(), Some(ConformanceEvent::RunCompleted { .. })),
            "last={:?}",
            events.last()
        );
        assert!(
            events.iter().any(
                |e| matches!(e, ConformanceEvent::ToolCallStarted{name, ..} if name=="read_temp")
            ),
            "expected read_temp ToolCallStarted; got {events:#?}"
        );
        assert!(events.iter().any(
            |e| matches!(e, ConformanceEvent::ToolCallCompleted{name, ..} if name=="read_temp")
        ));
    }
}
