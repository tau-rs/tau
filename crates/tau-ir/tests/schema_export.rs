//! Generates the published IR JSON Schema and guards it against drift.
//! Regenerate after an intended IR change with: UPDATE_SCHEMA=1 cargo test -p tau-ir --features schema --test schema_export
#![cfg(feature = "schema")]

use std::path::PathBuf;
use tau_ir::module::{IrFormatVersion, IrModule};

fn schema_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/tau-ir ; the repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/ir/tau-ir.v2.3.0.schema.json")
}

/// Single source of the schema bytes — used by both the writer and the drift check.
fn generate_ir_schema() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<IrModule>();
    let mut v = serde_json::to_value(&schema).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.insert(
        "$id".into(),
        format!(
            "https://lebocqtitouan.github.io/tau/schemas/ir/{}/tau-ir.schema.json",
            IrFormatVersion::CURRENT
        )
        .into(),
    );
    obj.insert(
        "title".into(),
        format!("tau IR module (ir_format {})", IrFormatVersion::CURRENT).into(),
    );
    obj.insert("x-tau-ir-format".into(), IrFormatVersion::CURRENT.into());
    v
}

fn pretty(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[test]
fn schema_matches_checked_in_file() {
    let generated = pretty(&generate_ir_schema());
    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::create_dir_all(schema_path().parent().unwrap()).unwrap();
        std::fs::write(schema_path(), &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(schema_path())
        .expect("schemas/ir/tau-ir.v2.3.0.schema.json missing — run with UPDATE_SCHEMA=1");
    assert_eq!(
        generated, on_disk,
        "published IR schema drifted from the serde types; regenerate with UPDATE_SCHEMA=1"
    );
}
