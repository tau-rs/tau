//! Drift gate for the published IR JSON schema + samples (EPIC 2.2).
//! `TAU_BLESS=1 cargo test -p tau-ir --features schema --test schema_drift`
//! (or `cargo xtask gen-ir-schema`) regenerates the committed files.
#![cfg(feature = "schema")]

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crates/tau-ir → repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn render(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("pretty");
    s.push('\n');
    s
}

#[test]
fn schema_and_samples_match_committed() {
    let root = repo_root();
    let bless = std::env::var_os("TAU_BLESS").is_some();

    // Schema.
    let schema = render(&tau_ir::schema_gen::ir_json_schema());
    let schema_path = root.join("schema/ir/tau-ir.schema.json");
    check_or_bless(&schema_path, &schema, bless);

    // Samples.
    for (name, module) in tau_ir::schema_gen::sample_modules() {
        let body = render(&serde_json::to_value(&module).expect("module to json"));
        let p = root.join(format!("schema/ir/samples/{name}.json"));
        check_or_bless(&p, &body, bless);
    }

    // Version embedded in the schema matches the source of truth.
    let v = tau_ir::module::IrFormatVersion::CURRENT;
    assert!(schema.contains(v), "schema must embed ir_format {v}");
}

fn check_or_bless(path: &std::path::Path, want: &str, bless: bool) {
    if bless {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, want).unwrap();
        return;
    }
    let got = std::fs::read_to_string(path).unwrap_or_else(|_| {
        panic!(
            "{} missing — run `cargo xtask gen-ir-schema`",
            path.display()
        )
    });
    assert_eq!(
        got,
        *want,
        "{} is stale — run `cargo xtask gen-ir-schema`",
        path.display()
    );
}
