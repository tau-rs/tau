//! TOML↔TS conformance: goals / deliverables / produces must produce
//! byte-equal canonical IR regardless of which authoring surface is used.

use std::path::Path;

#[test]
fn toml_and_ts_produce_byte_equal_canonical_ir_with_goals_and_deliverables() {
    let fixture_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deliverables_goals_conformance");

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
    let caches = tau_ir::lower::Caches {
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
        tau_ir::lower::lower_project(&toml_project, &target, &caches).expect("lower TOML to IR");
    let ts_ir =
        tau_ir::lower::lower_project(&ts_project, &target, &caches).expect("lower TS to IR");

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
