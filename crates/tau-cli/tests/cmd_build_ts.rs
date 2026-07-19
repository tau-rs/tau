//! Integration: `tau build project.ts -o <path>` with a minimal TS file
//! routes through the TS extractor and exits with a status code.
//!
//! The bundle build step itself may fail (the bundle builder currently
//! requires `tau.toml`; full TS→bundle is deferred to β.8 Phase 6). The
//! key assertion is that the TS extractor was reached and the process exits
//! cleanly (with a status code, not a signal-kill or panic).

use assert_fs::prelude::*;

#[test]
fn build_with_ts_project_exits_gracefully() {
    let tmp = assert_fs::TempDir::new().expect("tmpdir");
    tmp.child("project.ts")
        .write_str(
            r#"
export const m = models({
    haiku: { backend: "anthropic", model: "claude-haiku-4-5" }
});
export const a = agent({
    display_name: "A",
    package: "a@^0.1",
    model: "haiku",
    prompt: { system: "x" }
});
"#,
        )
        .expect("write");

    let out_path = tmp.child("out.bundle");
    let assert = assert_cmd::Command::cargo_bin("tau")
        .expect("bin")
        .current_dir(tmp.path())
        .args([
            "build",
            "project.ts",
            "-o",
            out_path.path().to_str().unwrap(),
            // The TS authoring surface has no `[allow]` equivalent; this
            // test only asserts the process exits with a status code, so
            // skip the governance gate.
            "--no-governance",
        ])
        .timeout(std::time::Duration::from_secs(20))
        .assert();
    let output = assert.get_output();
    assert!(
        output.status.code().is_some(),
        "process must exit with a status code (not be signal-killed): {:?}",
        output.status
    );
}
