//! Bundle-mode runner: build a bundle from the fixture, decode its
//! embedded IR payload, then drive the v0 interpreter against the
//! deserialized `IrModule`.
//!
//! Symmetric with [`crate::dev_mode::DevMode`] by construction: both
//! modes ultimately call [`crate::dev_mode::drive_module`] with the
//! same `(module, entry, responses)` triple for the same fixture. The
//! only difference is the IR-module sourcing path:
//!
//! - DevMode lowers `workflow.toml` in-process via
//!   `tau_ir::lower::lower_project`.
//! - BundleMode lowers the same `workflow.toml` via
//!   `tau_pkg::bundle::build` (which calls the exact same `lower_project`
//!   internally via [`build_ir_payload`]), then decodes the bundle's
//!   `ir_payload.canonical_ir_bytes_hex` back to an `IrModule` via
//!   `tau_ir::from_canonical_bytes`. The round-trip is verified by
//!   asserting `module.canonical_hash() == ir_payload.canonical_ir_hash`.
//!
//! The conformance suite is in-process: BundleMode does NOT spawn any
//! real plugin processes. Tool invocations are recorded by the same
//! `RecordingDispatcher` DevMode uses (canned `{"ok": true}` response).

use std::path::Path;

use async_trait::async_trait;

use tau_ir::lower::{lower_project, Caches};
use tau_ir::IrModule;
use tau_pkg::bundle::{build as build_bundle, BuildOptions, BundleManifest, IrPayload};
use tau_pkg::project::ProjectConfig;
use tau_ports::target::registry as target_registry;

use crate::dev_mode::{drive_module, drive_pipeline, parse_mock_llm, CONFORMANCE_PIPELINE_INPUT};
use crate::{ConformanceReport, ExecutionMode};

/// Bundle-mode runner: builds a bundle from the fixture, decodes the
/// embedded IR, and drives the interpreter through the shared
/// dispatcher.
///
/// See the module-level docs for the round-trip invariant.
pub struct BundleMode;

#[async_trait(?Send)]
impl ExecutionMode for BundleMode {
    async fn run(&self, fixture_dir: &Path) -> ConformanceReport {
        // All non-`Send` setup (the `Caches` closures, tempdir handles)
        // is confined to this synchronous block so the async future
        // remains `Send`-friendly (the `?Send` on ExecutionMode is for
        // run_ir's RefCell internals later — see lib.rs).
        let prepared: Result<(IrModule, Vec<tau_ports::llm::CompletionResponse>, String), String> = {
            // 1. Load workflow.toml + mock_llm.jsonl. Missing files are a
            //    fixture misconfiguration, not a build refusal — panic so
            //    the failure is loud and obvious in test output.
            let workflow_toml = std::fs::read_to_string(fixture_dir.join("workflow.toml"))
                .expect("fixture must contain workflow.toml");
            let mock_llm_jsonl = std::fs::read_to_string(fixture_dir.join("mock_llm.jsonl"))
                .expect("fixture must contain mock_llm.jsonl");
            let responses = parse_mock_llm(&mock_llm_jsonl);

            // 2. Stage a scratch project root: tau.toml = the fixture's
            //    workflow.toml verbatim + a synthesised empty tau.lock so
            //    `tau_pkg::bundle::build`'s install-state check passes.
            //    Conformance fixtures don't ship installed packages, so
            //    the lockfile is empty and the agent-home-package check
            //    (which only fires for capability-override agents) stays
            //    quiet.
            let scratch = match tempfile::tempdir() {
                Ok(t) => t,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: tempdir create failed: {e}"
                    ));
                }
            };
            if let Err(e) = std::fs::write(scratch.path().join("tau.toml"), &workflow_toml) {
                return ConformanceReport::build_refused(format!(
                    "bundle mode: writing scratch tau.toml failed: {e}"
                ));
            }
            // Empty lockfile (schema_version 6 — the current version
            // tau_pkg::lockfile::LockFile expects).
            let empty_lock = "schema_version = 6\n\
                              generated_by_tau_version = \"0.1.0\"\n\
                              generated_at = \"2024-01-01T00:00:00Z\"\n";
            if let Err(e) = std::fs::write(scratch.path().join("tau.lock"), empty_lock) {
                return ConformanceReport::build_refused(format!(
                    "bundle mode: writing scratch tau.lock failed: {e}"
                ));
            }

            // 3. Pre-lower the IR in-process (same SHA-256-of-name cache
            //    DevMode uses, mirrored from `tau-cli::cmd::build::lower_ir`)
            //    so the IR payload is populated. Errors here are the build-
            //    refused signal: report symmetric with DevMode.
            let target = target_registry::list_available()
                .next()
                .expect("at least one target triple available")
                .triple;
            let ir_payload = match build_ir_payload(&workflow_toml, &target) {
                Ok(payload) => Some(payload),
                Err(refusal) => return ConformanceReport::build_refused(refusal),
            };

            // 4. Build the bundle. The build path validates the project
            //    config + lockfile + install state and stamps the IR
            //    payload into the manifest. v0 fixtures never have
            //    capability overrides, so step-5 home-package resolution
            //    is skipped — keeping the scratch dir's empty lockfile
            //    enough to succeed.
            let opts = BuildOptions {
                project_root: scratch.path().to_path_buf(),
                target,
                output_path: None,
                agent_filter: None,
                ir_payload,
            };
            let artifact = match build_bundle(opts) {
                Ok(a) => a,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: tau_pkg::bundle::build failed: {e}"
                    ));
                }
            };

            // 5. Parse the bundle back and pull out the IR payload. A v2
            //    bundle MUST carry an `ir_payload`; if it doesn't, the
            //    build path silently dropped it (would mean we passed
            //    `None` above — already a bug) — surface as a build
            //    refusal so the test fails informatively.
            let bundle_str = match std::fs::read_to_string(&artifact.path) {
                Ok(s) => s,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: reading built bundle failed: {e}"
                    ));
                }
            };
            let manifest = match BundleManifest::parse_str(&bundle_str) {
                Ok(m) => m,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: parsing built bundle failed: {e}"
                    ));
                }
            };
            let ir_payload = match manifest.ir_payload.as_ref() {
                Some(p) => p,
                None => {
                    return ConformanceReport::build_refused(
                        "bundle mode: built bundle has no ir_payload".to_string(),
                    );
                }
            };

            // 6. Decode the canonical IR bytes back to an IrModule and
            //    verify the round-trip hash. A mismatch here means
            //    canonical-encoding drift between the build path and
            //    `tau_ir::from_canonical_bytes` — a serious bug, surface
            //    as a build refusal so the test fails informatively.
            let ir_bytes = match ir_payload.canonical_ir_bytes() {
                Ok(b) => b,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: hex decoding canonical_ir_bytes_hex failed: {e}"
                    ));
                }
            };
            let module = match tau_ir::from_canonical_bytes(&ir_bytes) {
                Ok(m) => m,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: decoding canonical IR bytes failed: {e}"
                    ));
                }
            };
            let recomputed_hash = tau_ir::compute_hash(&module);
            let recorded_hash = match ir_payload.canonical_ir_hash_bytes() {
                Ok(h) => h,
                Err(e) => {
                    return ConformanceReport::build_refused(format!(
                        "bundle mode: hex decoding canonical_ir_hash failed: {e}"
                    ));
                }
            };
            if recomputed_hash != recorded_hash {
                return ConformanceReport::build_refused(format!(
                    "bundle mode: IR canonical-hash round-trip mismatch: recorded={}, recomputed={}",
                    hex_lower(&recorded_hash),
                    hex_lower(&recomputed_hash),
                ));
            }

            // 7. Pick the entry agent (first BTreeMap key — alphabetical
            //    order), same convention as DevMode + the CLI dispatcher.
            let entry = module
                .workflow
                .agents
                .keys()
                .next()
                .expect("decoded IR module must declare at least one agent")
                .clone();

            // `scratch` and `caches` closures drop here. The IR bytes
            // have already been round-tripped through the bundle, so the
            // tempdir's lifetime doesn't matter past this point.
            Ok((module, responses, entry.0))
        };

        match prepared {
            Ok((module, responses, entry_id)) => {
                let module = std::sync::Arc::new(module);
                // Mirror DevMode: pipeline workflows are engine-sequenced
                // (no single entry agent loop). Both modes drive the same
                // in-process `run_pipeline`, so the round-tripped bundle
                // module produces the same report as the in-process lower.
                if module.workflow.pipeline.is_some() {
                    drive_pipeline(module, CONFORMANCE_PIPELINE_INPUT.to_string(), responses).await
                } else {
                    let entry = tau_ir::AgentId(entry_id);
                    drive_module(module, &entry, responses).await
                }
            }
            Err(refusal) => ConformanceReport::build_refused(refusal),
        }
    }
}

/// Lower a workflow.toml to an `IrPayload`, mirroring the cache shape
/// `tau-cli::cmd::build::lower_ir` uses. Errors surface as the human-
/// readable `Display` form of `IrError` so DevMode and BundleMode emit
/// byte-identical build-refused diagnostics for the same fixture.
fn build_ir_payload(
    workflow_toml: &str,
    target: &tau_ports::target::TargetTriple,
) -> Result<IrPayload, String> {
    // Project-config validation errors (e.g. `DeliverableNoProducer`) are
    // surfaced here. The error string is formatted WITHOUT a "bundle mode:"
    // prefix so both DevMode and BundleMode emit the same diagnostic and
    // `assert_conform` can compare them byte-for-byte (D-3b refusal symmetry).
    let config = ProjectConfig::parse_str(workflow_toml).map_err(|e| format!("{e}"))?;

    let caches = Caches {
        native_tool: &|name: &str| Some(crate::sha256_name(name)),
        mcp_contract: &|_| None,
        skill: &|_| None,
    };
    let module = lower_project(&config, target, &caches).map_err(|e| format!("{e}"))?;
    let bytes = tau_ir::to_canonical_bytes(&module);
    let hash_bytes = tau_ir::compute_hash(&module);
    Ok(IrPayload {
        ir_format: module.ir_format.0.clone(),
        canonical_ir_hash: hex_lower(&hash_bytes),
        canonical_ir_bytes_hex: hex_lower(&bytes),
    })
}

/// Encode a byte slice as lowercase hex — symmetric with
/// `tau-cli::cmd::build::hex_lower` so DevMode-built and BundleMode-
/// built IR payloads encode their bytes identically.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture whose tool declares an `agent.spawn` capability — that
    /// shape is NOT in `linux-native-strict`'s `required_shapes`
    /// (`fs_rw_exec_net`), so `tau_ir::lower::capability_fit::check`
    /// refuses lowering with `IrError::CapabilityFitFailed`. BundleMode
    /// must surface this as a build-refused report instead of crashing.
    ///
    /// This is the D-3b symmetry test: any fixture lowering refuses MUST
    /// produce a `build_refused`-shaped report in BOTH modes — DevMode
    /// is covered by its own panic-replacement in `dev_mode.rs`.
    #[tokio::test(flavor = "current_thread")]
    async fn bundle_mode_reports_build_refused_on_capability_fit_failure() {
        let scratch = tempfile::tempdir().unwrap();
        // workflow.toml: one agent + one tool declaring an agent.spawn
        // capability. The first available target (linux-native-strict)
        // supports fs.read/fs.write/process.exec/net.http only, so the
        // capability_fit step refuses.
        std::fs::write(
            scratch.path().join("workflow.toml"),
            r#"
[project]
name = "refused"

[agents.solo]
display_name = "Solo"
package      = "p@^0.1"
llm_backend  = "mock-llm"
tool_refs    = ["spawner"]
max_turns    = 1

[tools.spawner]
native      = "Spawner"
description = "spawns a sub-agent"
[[tools.spawner.capabilities]]
kind = "agent.spawn"
allowed_kinds = ["critic"]
"#,
        )
        .unwrap();
        // mock_llm.jsonl unused (build refuses before drive_module).
        std::fs::write(scratch.path().join("mock_llm.jsonl"), "").unwrap();

        let report = BundleMode.run(scratch.path()).await;
        let refused = report
            .build_refused
            .as_ref()
            .expect("expected build_refused report; got an executed-run report");
        // The diagnostic must reference the IrError surface so any
        // assert_conform asymmetry would name the actual refusal cause.
        assert!(
            refused.to_lowercase().contains("capability")
                || refused.to_lowercase().contains("agentspawn")
                || refused.contains("AgentSpawn"),
            "diagnostic should name the capability-fit refusal; got: {refused}"
        );
        // The multiset fields stay empty when build_refused is Some.
        assert!(report.tool_calls.is_empty());
        assert!(report.message_added.is_empty());
    }
}
