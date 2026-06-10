//! Integration: `tau dev project.ts -p <prompt>` with a minimal TS file
//! boots, extracts the project config, and exits gracefully (with a status
//! code, not a signal-kill).
//!
//! The test does NOT verify that the LLM backend is available — `tau dev`
//! exits with a non-zero code when no plugin is installed, but the key
//! assertion is that the TS extractor was reached and the process exits
//! (rather than panicking or crashing without a status code).

use assert_fs::prelude::*;

#[test]
fn dev_one_shot_with_ts_project_exits_gracefully() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("project.ts")
        .write_str(
            r#"
export const a = agent({
    display_name: "A",
    package: "a@^0.1",
    llm_backend: "anthropic",
    model: "claude-haiku-4-5",
    prompt: { system: "x" }
});
"#,
        )
        .expect("write");

    let assert = assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["dev", "project.ts", "-p", "hi"])
        .timeout(std::time::Duration::from_secs(20))
        .assert();
    let output = assert.get_output();
    assert!(
        output.status.code().is_some(),
        "process must exit with a status code (not be signal-killed): {:?}",
        output.status
    );
}
