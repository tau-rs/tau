//! TOML↔TS conformance: an agent's `durable` block (ADR-0053) must produce
//! byte-equal canonical IR regardless of which authoring surface is used.
//!
//! Two fixtures: `durable_conformance` (explicit per-turn form) and
//! `durable_intent_conformance` (EPIC 6.1 intent form `survive-restarts`).

use std::path::Path;

#[test]
fn toml_and_ts_produce_byte_equal_canonical_ir_with_agent_durable() {
    let fixture_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/durable_conformance");

    // ── TOML path ────────────────────────────────────────────────────────────
    let toml_str = std::fs::read_to_string(fixture_dir.join("tau.toml")).expect("read tau.toml");
    let toml_project =
        tau_pkg::project::project::ProjectConfig::parse_str(&toml_str).expect("parse tau.toml");

    // ── TS path ──────────────────────────────────────────────────────────────
    let ts_src = std::fs::read_to_string(fixture_dir.join("project.ts")).expect("read project.ts");
    let ts_project = tau_ts_extract::extract_project(&ts_src, &fixture_dir.join("project.ts"))
        .expect("extract project.ts");

    // ── Lower both to IR ─────────────────────────────────────────────────────
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let caches = tau_ir_lower::Caches {
        native_tool: &|fn_name| {
            let seed = fn_name.as_bytes().first().copied().unwrap_or(1);
            let mut h = [0u8; 32];
            for b in h.iter_mut() {
                *b = seed;
            }
            Some(h)
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
    };

    let toml_ir =
        tau_ir_lower::lower_project(&toml_project, &target, &caches).expect("lower TOML to IR");
    let ts_ir = tau_ir_lower::lower_project(&ts_project, &target, &caches).expect("lower TS to IR");

    // Sanity: the durable block actually lowered (not silently dropped on
    // either path) — otherwise byte-equality could be trivially satisfied by
    // both producing a durable-less agent.
    let fan = toml_ir
        .workflow
        .agents
        .get(&tau_ir::AgentId("fan".into()))
        .expect("fan agent present");
    assert!(
        fan.durable.is_some(),
        "TOML path must lower the durable block onto the agent"
    );

    // ── Canonical-encode and compare bytes ───────────────────────────────────
    let toml_bytes = tau_ir::canonical::to_canonical_bytes(&toml_ir);
    let ts_bytes = tau_ir::canonical::to_canonical_bytes(&ts_ir);

    if toml_bytes != ts_bytes {
        let toml_str = String::from_utf8_lossy(&toml_bytes);
        let ts_str = String::from_utf8_lossy(&ts_bytes);
        panic!(
            "TOML↔TS canonical IRs differ:\n--- TOML ---\n{}\n--- TS ---\n{}\n",
            toml_str, ts_str
        );
    }
}

#[test]
fn toml_and_ts_produce_byte_equal_canonical_ir_with_durable_intent() {
    let fixture_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/durable_intent_conformance");

    // ── TOML path ────────────────────────────────────────────────────────────
    let toml_str = std::fs::read_to_string(fixture_dir.join("tau.toml")).expect("read tau.toml");
    let toml_project =
        tau_pkg::project::project::ProjectConfig::parse_str(&toml_str).expect("parse tau.toml");

    // ── TS path ──────────────────────────────────────────────────────────────
    let ts_src = std::fs::read_to_string(fixture_dir.join("project.ts")).expect("read project.ts");
    let ts_project = tau_ts_extract::extract_project(&ts_src, &fixture_dir.join("project.ts"))
        .expect("extract project.ts");

    // ── Lower both to IR ─────────────────────────────────────────────────────
    let target = tau_ports::target::TargetTriple::PASSTHROUGH;
    let caches = tau_ir_lower::Caches {
        native_tool: &|fn_name| {
            let seed = fn_name.as_bytes().first().copied().unwrap_or(1);
            let mut h = [0u8; 32];
            for b in h.iter_mut() {
                *b = seed;
            }
            Some(h)
        },
        mcp_contract: &|_| None,
        skill: &|_| None,
    };

    let toml_ir =
        tau_ir_lower::lower_project(&toml_project, &target, &caches).expect("lower TOML to IR");
    let ts_ir = tau_ir_lower::lower_project(&ts_project, &target, &caches).expect("lower TS to IR");

    // Sanity: the intent form actually lowered to
    // `Durability::Intent(DurabilityIntent::SurviveRestarts)` and was not
    // silently dropped on either path — otherwise byte-equality could be
    // trivially satisfied by both producing a durable-less agent.
    let fan = toml_ir
        .workflow
        .agents
        .get(&tau_ir::AgentId("fan".into()))
        .expect("fan agent present");
    assert!(
        fan.durable
            == Some(tau_ir::durable::Durability::Intent(
                tau_ir::durable::DurabilityIntent::SurviveRestarts
            )),
        "TOML path must lower durable intent to Durability::Intent(SurviveRestarts); got: {:?}",
        fan.durable
    );

    // ── Canonical-encode and compare bytes ───────────────────────────────────
    let toml_bytes = tau_ir::canonical::to_canonical_bytes(&toml_ir);
    let ts_bytes = tau_ir::canonical::to_canonical_bytes(&ts_ir);

    if toml_bytes != ts_bytes {
        let toml_str = String::from_utf8_lossy(&toml_bytes);
        let ts_str = String::from_utf8_lossy(&ts_bytes);
        panic!(
            "TOML↔TS canonical IRs differ (intent form):\n--- TOML ---\n{}\n--- TS ---\n{}\n",
            toml_str, ts_str
        );
    }
}
