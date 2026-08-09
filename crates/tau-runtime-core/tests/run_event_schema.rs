//! Freeze test: the committed RunEvent JSON schema must equal fresh schemars
//! output. Regenerate with:
//!   UPDATE_SCHEMA=1 cargo test -p tau-runtime-core --features schema --test run_event_schema
#![cfg(feature = "schema")]

use std::path::PathBuf;

use tau_runtime_core::stream::{RunEvent, RUN_EVENT_SCHEMA_VERSION};

fn schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas/run-event/run-event.v1.schema.json")
}

fn generate() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::draft2020_12();
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<RunEvent>();
    let mut v = serde_json::to_value(&schema).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.insert(
        "$id".into(),
        format!(
            "https://lebocqtitouan.github.io/tau/schemas/run-event/{}/run-event.schema.json",
            RUN_EVENT_SCHEMA_VERSION
        )
        .into(),
    );
    obj.insert(
        "title".into(),
        format!("tau RunEvent ({})", RUN_EVENT_SCHEMA_VERSION).into(),
    );
    v
}

fn pretty(v: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap();
    s.push('\n');
    s
}

#[test]
fn run_event_schema_matches_checked_in_file() {
    let generated = pretty(&generate());
    if std::env::var("UPDATE_SCHEMA").is_ok() {
        std::fs::create_dir_all(schema_path().parent().unwrap()).unwrap();
        std::fs::write(schema_path(), &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(schema_path())
        .expect("schemas/run-event/run-event.v1.schema.json missing — run with UPDATE_SCHEMA=1");
    assert_eq!(
        generated, on_disk,
        "RunEvent schema drifted from serde types; regenerate with UPDATE_SCHEMA=1"
    );
}
