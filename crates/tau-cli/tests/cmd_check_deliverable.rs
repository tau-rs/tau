//! Integration test: `tau check config` surfaces `DeliverableNoProducer`
//! build-time errors end-to-end.

#[path = "check_common.rs"]
mod check_common;

use assert_cmd::Command;
use tempfile::TempDir;

/// A `tau.toml` with a deliverable whose path no agent declares in `produces`.
/// Validation should reject it with `DeliverableNoProducer`.
const TAU_TOML_DELIVERABLE_NO_PRODUCER: &str = r#"
[project]
name = "missing-producer"

[deliverables.report]
path = "/workspace/report.md"
must_satisfy = "The report covers all findings."
"#;

/// `tau check config` must exit 2 (Error severity) and emit the
/// "has no producer" message when a deliverable has no matching producer.
///
/// This is a characterization test: `ProjectConfig::from_path` calls
/// `validate()` → `validate_postconditions()` → `DeliverableNoProducer`, and
/// `run_config` translates any `ProjectConfigError` into a `Severity::Error`
/// finding (exit 2).  No wiring change was needed — the test confirms the
/// end-to-end surfacing.
#[test]
fn tau_check_reports_deliverable_with_no_producer() {
    check_common::ensure_tau_home();

    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    std::fs::write(proj.join("tau.toml"), TAU_TOML_DELIVERABLE_NO_PRODUCER).unwrap();

    let out = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "config"])
        .current_dir(&proj)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Validation error → Severity::Error → exit 2.
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for DeliverableNoProducer\nstdout: {stdout}\nstderr: {stderr}",
    );

    // The human renderer embeds the ProjectConfigError::to_string() message in
    // the finding summary, which contains "has no producer".
    assert!(
        stdout.contains("has no producer"),
        "expected 'has no producer' in stdout\nstdout: {stdout}\nstderr: {stderr}",
    );
}

/// `tau check config --json` must tag the finding with the typed
/// `structured.kind = "DeliverableNoProducer"`, not the `"Other"` catch-all
/// (issue #472). Guards the per-variant `variant_kind` mapping against
/// regressing back to the wildcard.
#[test]
fn tau_check_json_emits_typed_kind_for_deliverable_no_producer() {
    check_common::ensure_tau_home();

    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    std::fs::write(proj.join("tau.toml"), TAU_TOML_DELIVERABLE_NO_PRODUCER).unwrap();

    let out = Command::cargo_bin("tau")
        .unwrap()
        .args(["check", "config", "--json"])
        .current_dir(&proj)
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Find the finding's structured.kind across the JSONL stream.
    let kind = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("findings").and_then(|f| f.as_array().cloned()))
        .flatten()
        .find_map(|f| {
            f.get("structured")
                .and_then(|s| s.get("kind"))
                .and_then(|k| k.as_str())
                .map(str::to_owned)
        });

    assert_eq!(
        kind.as_deref(),
        Some("DeliverableNoProducer"),
        "expected typed kind, not \"Other\"\nstdout: {stdout}\nstderr: {stderr}",
    );
}
